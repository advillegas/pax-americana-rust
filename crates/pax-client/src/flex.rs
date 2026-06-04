//! IBKR Flex Web Service client + background perf worker.
//!
//! Fetches historical trades and NAV data via IBKR's Flex Web Service HTTPS/XML API,
//! then triggers analytics and chart rendering. Also handles NAV/returns recomputation
//! and PDF export on request.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::state::{LogLevel, SharedState};

const SEND_URL: &str =
    "https://gdcdyn.interactivebrokers.com/Universal/servlet/FlexStatementService.SendRequest";
const GET_URL: &str =
    "https://gdcdyn.interactivebrokers.com/Universal/servlet/FlexStatementService.GetStatement";

pub fn spawn(state: Arc<SharedState>) {
    thread::spawn(move || worker_loop(state));
}

const AUTO_REFRESH_SECS: u64 = 3600; // 1 hour

fn worker_loop(state: Arc<SharedState>) {
    let mut last_fetch = Instant::now();

    loop {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Manual fetch request from the GUI.
            if state.flex_request.swap(false, Ordering::Relaxed) {
                if try_fetch(&state) {
                    last_fetch = Instant::now();
                }
            }

            // Delayed retry after a transient IBKR error (5-minute backoff).
            let retry_at = state.flex_retry_at.load(Ordering::Relaxed);
            if retry_at > 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if now >= retry_at {
                    state.flex_retry_at.store(0, Ordering::Relaxed);
                    state.log(LogLevel::Info, "Flex: auto-retrying after transient error…");
                    if try_fetch(&state) {
                        last_fetch = Instant::now();
                    }
                }
            }

            // Hourly auto-refresh (only if token + query are configured).
            if last_fetch.elapsed() >= Duration::from_secs(AUTO_REFRESH_SECS) {
                let has_config = {
                    let cfg = state.flex_config.lock();
                    !cfg.token.is_empty() && !cfg.query_id.is_empty()
                };
                if has_config {
                    state.log(LogLevel::Info, "Flex: hourly auto-refresh…");
                    if try_fetch(&state) {
                        last_fetch = Instant::now();
                    }
                }
                last_fetch = Instant::now(); // reset even on failure to avoid tight retry
            }

            if state.perf_recompute.swap(false, Ordering::Relaxed) {
                recompute(&state);
            }

            if state.export_pdf.swap(false, Ordering::Relaxed) {
                export_pdf(&state);
            }
        }));

        if result.is_err() {
            state.log(LogLevel::Err, "Perf worker recovered from an internal error.");
        }

        thread::sleep(Duration::from_millis(300));
    }
}

/// Attempt a fetch using the currently configured token + query ID. Returns true on success.
fn try_fetch(state: &SharedState) -> bool {
    let (token, query_id) = {
        let cfg = state.flex_config.lock();
        (cfg.token.clone(), cfg.query_id.clone())
    };
    if token.is_empty() || query_id.is_empty() {
        *state.flex_status.lock() = "Set Flex token and Query ID first.".into();
        return false;
    }
    fetch_and_process(state, &token, &query_id)
}

fn fetch_and_process(state: &SharedState, token: &str, query_id: &str) -> bool {
    state.log(LogLevel::Info, format!("Flex: SendRequest (token={}…, query={query_id})", &token[..token.len().min(8)]));
    *state.flex_status.lock() = "Sending request to IBKR…".into();

    // Step 1: SendRequest → get reference code.
    let url = format!("{SEND_URL}?t={token}&q={query_id}&v=3");
    let body = match http_get(&url) {
        Ok(b) => b,
        Err(e) => {
            set_err(state, &format!("SendRequest failed: {e}"));
            return false;
        }
    };

    // Log the raw response (truncated) for diagnostics.
    let snippet: String = body.chars().take(300).collect();
    state.log(LogLevel::Info, format!("Flex SendRequest response: {snippet}"));

    let ref_code = match extract_tag(&body, "ReferenceCode") {
        Some(c) => c,
        None => {
            let code = extract_tag(&body, "ErrorCode").unwrap_or_default();
            let msg = extract_tag(&body, "ErrorMessage")
                .unwrap_or_else(|| format!("Unexpected response (no ReferenceCode). Raw: {snippet}"));
            let transient = code == "1001" || code == "1003" || code == "1009"
                || msg.contains("try again") || msg.contains("could not be generated");
            if transient {
                *state.flex_status.lock() = format!("IBKR busy (error {code}) — will auto-retry in 5 min.");
                state.log(LogLevel::Warn, format!("Flex: IBKR error {code}, will retry in 5 min."));
                // Schedule a quiet retry via the auto-refresh mechanism.
                state.flex_retry_at.store(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                        + 300,
                    Ordering::Relaxed,
                );
                return false;
            }
            set_err(state, &format!("IBKR error {code}: {msg}"));
            return false;
        }
    };

    // Step 2: Poll GetStatement until the statement is ready.
    *state.flex_status.lock() = format!("Statement generating (ref {ref_code})…");
    state.log(LogLevel::Info, format!("Flex: polling ref {ref_code}…"));

    let mut attempts = 0;
    let xml = loop {
        attempts += 1;
        if attempts > 30 {
            set_err(state, "Flex: statement timed out after 60s of polling.");
            return false;
        }
        thread::sleep(Duration::from_secs(2));

        let url = format!("{GET_URL}?t={token}&q={ref_code}&v=3");
        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => {
                set_err(state, &format!("GetStatement failed: {e}"));
                return false;
            }
        };

        if body.contains("<FlexStatements") || body.contains("<FlexQueryResponse") {
            break body;
        }
        if body.contains("Statement generation in progress") || body.contains("1019") {
            *state.flex_status.lock() = format!("Generating… (attempt {attempts})");
            continue;
        }
        let code = extract_tag(&body, "ErrorCode").unwrap_or_default();
        let msg = extract_tag(&body, "ErrorMessage").unwrap_or_default();
        if !msg.is_empty() {
            set_err(state, &format!("IBKR poll error {code}: {msg}"));
            return false;
        }
    };

    *state.flex_status.lock() = "Parsing statement…".into();

    match crate::flexparse::parse(&xml) {
        Ok((trades, nav, cashflows)) => {
            let nt = trades.len();
            let nn = nav.len();
            save_cache(&trades, &nav, &cashflows);
            *state.flex_trades.lock() = trades;
            *state.nav_history.lock() = nav;
            *state.cashflows.lock() = cashflows;
            *state.flex_status.lock() = format!("Loaded: {nt} trades, {nn} NAV points.");
            state.log(LogLevel::Ok, format!("Flex: {nt} trades, {nn} NAV points."));
            recompute(state);
            true
        }
        Err(e) => {
            set_err(state, &format!("Parse error: {e}"));
            false
        }
    }
}

pub fn recompute(state: &SharedState) {
    let trades = state.flex_trades.lock().clone();
    let nav = state.nav_history.lock().clone();
    let cashflows = state.cashflows.lock().clone();
    let sectors = state.sectors.lock().clone();
    let show_returns = state.perf_curve_mode.load(Ordering::Relaxed) == 1;

    if trades.is_empty() && nav.is_empty() {
        return;
    }

    // Step 1: compute analytics (pure math, can't fail).
    let rts = crate::analytics::build_round_trips(&trades, &sectors);
    let metrics = crate::analytics::compute_metrics(&rts, &nav, &cashflows);

    // Store analytics immediately so the UI gets trade list + metrics
    // even if chart rendering fails below.
    state.log(
        LogLevel::Ok,
        format!("Analytics: {} round trips, {:.2}% return", rts.len(), metrics.total_return),
    );
    *state.round_trips.lock() = rts.clone();
    *state.metrics.lock() = Some(metrics.clone());
    state.perf_gen.fetch_add(1, Ordering::Relaxed);

    // Step 2: render charts (plotters — may panic on systems without fonts).
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::charts::render_all(&nav, &rts, &metrics, show_returns)
    })) {
        Ok(charts) => {
            *state.perf_charts.lock() = charts;
            state.perf_gen.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            state.log(LogLevel::Warn, "Chart rendering failed (font or graphics issue on this system).");
        }
    }
}

fn export_pdf(state: &SharedState) {
    let metrics = state.metrics.lock().clone();
    let rts = state.round_trips.lock().clone();
    let charts = state.perf_charts.lock().clone();
    let cfg = state.flex_config.lock().clone();

    let dir = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    let path = std::path::PathBuf::from(dir)
        .join("Downloads")
        .join(format!("pax-report-{}.pdf", crate::state::now_hms().replace(':', "")));

    *state.export_status.lock() = "Generating PDF…".into();
    let sections = crate::report::ReportSections {
        equity: cfg.show_equity,
        drawdown: cfg.show_drawdown,
        histogram: cfg.show_histogram,
        pies: cfg.show_pies,
        symbol_bar: cfg.show_symbol_bar,
        monthly: cfg.show_monthly,
        trade_list: true,
    };
    match crate::report::export(&path, &metrics, &rts, &charts, &sections) {
        Ok(()) => {
            let msg = format!("Saved to {}", path.display());
            *state.export_status.lock() = msg.clone();
            state.log(LogLevel::Ok, msg);
        }
        Err(e) => {
            *state.export_status.lock() = format!("Export failed: {e}");
            state.log(LogLevel::Err, format!("PDF export failed: {e}"));
        }
    }
}

fn set_err(state: &SharedState, msg: &str) {
    *state.flex_status.lock() = msg.to_string();
    state.log(LogLevel::Err, format!("Flex: {msg}"));
}

/// HTTP GET that returns the response body for BOTH success and error status codes.
/// ureq 2.x treats 4xx/5xx as Err, but IBKR returns XML error bodies we need to parse.
fn http_get(url: &str) -> Result<String, String> {
    match ureq::get(url).call() {
        Ok(resp) => resp.into_string().map_err(|e| format!("body read: {e}")),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            if body.contains('<') {
                Ok(body) // XML error response — let the caller parse it
            } else {
                Err(format!("HTTP {code}: {body}"))
            }
        }
        Err(ureq::Error::Transport(t)) => Err(format!("connection: {t}")),
    }
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)?;
    Some(xml[start..start + end].trim().to_string())
}

// ── Flex data cache (survives restarts) ──────────────────────────────────────

use serde::{Deserialize, Serialize};

const CACHE_FILE: &str = "fd.dat";

#[derive(Serialize, Deserialize)]
struct FlexCache {
    trades: Vec<CachedTrade>,
    nav: Vec<CachedNav>,
    cashflows: Vec<CachedCashflow>,
}

#[derive(Serialize, Deserialize)]
struct CachedTrade {
    dt: String,
    sym: String,
    side: String,
    qty: f64,
    px: f64,
    proceeds: f64,
    comm: f64,
    rpnl: f64,
    cat: String,
    ccy: String,
    desc: String,
}

#[derive(Serialize, Deserialize)]
struct CachedNav {
    date: String,
    nav: f64,
}

#[derive(Serialize, Deserialize)]
struct CachedCashflow {
    date: String,
    amount: f64,
}

fn save_cache(
    trades: &[crate::state::FlexTrade],
    nav: &[crate::state::NavPoint],
    cashflows: &[crate::state::Cashflow],
) {
    let c = FlexCache {
        trades: trades
            .iter()
            .map(|t| CachedTrade {
                dt: t.date_time.clone(),
                sym: t.symbol.clone(),
                side: t.side.clone(),
                qty: t.quantity,
                px: t.price,
                proceeds: t.proceeds,
                comm: t.commission,
                rpnl: t.realized_pnl,
                cat: t.asset_category.clone(),
                ccy: t.currency.clone(),
                desc: t.description.clone(),
            })
            .collect(),
        nav: nav.iter().map(|n| CachedNav { date: n.date.clone(), nav: n.nav }).collect(),
        cashflows: cashflows
            .iter()
            .map(|c| CachedCashflow { date: c.date.clone(), amount: c.amount })
            .collect(),
    };
    if let Ok(bytes) = serde_json::to_vec(&c) {
        crate::appdata::write(CACHE_FILE, bytes);
    }
}

/// Load cached Flex data from disk (if any) and run analytics. Called once at startup
/// so the Trades + Perf tabs are populated immediately without re-fetching from IBKR.
pub fn load_cache_into(state: &SharedState) {
    let bytes = match crate::appdata::read(CACHE_FILE) {
        Some(b) => b,
        None => return,
    };
    let c: FlexCache = match serde_json::from_slice(&bytes) {
        Ok(c) => c,
        Err(_) => return,
    };

    let trades: Vec<crate::state::FlexTrade> = c
        .trades
        .into_iter()
        .map(|t| crate::state::FlexTrade {
            date_time: t.dt,
            symbol: t.sym,
            side: t.side,
            quantity: t.qty,
            price: t.px,
            proceeds: t.proceeds,
            commission: t.comm,
            realized_pnl: t.rpnl,
            asset_category: t.cat,
            currency: t.ccy,
            description: t.desc,
            sector: String::new(),
        })
        .collect();
    let nav: Vec<crate::state::NavPoint> =
        c.nav.into_iter().map(|n| crate::state::NavPoint { date: n.date, nav: n.nav }).collect();
    let cashflows: Vec<crate::state::Cashflow> = c
        .cashflows
        .into_iter()
        .map(|c| crate::state::Cashflow { date: c.date, amount: c.amount })
        .collect();

    let nt = trades.len();
    let nn = nav.len();
    if nt == 0 && nn == 0 {
        return;
    }

    *state.flex_trades.lock() = trades;
    *state.nav_history.lock() = nav;
    *state.cashflows.lock() = cashflows;
    *state.flex_status.lock() = format!("Cached: {nt} trades, {nn} NAV points. Click FETCH to refresh.");
    state.log(LogLevel::Info, format!("Loaded cached Flex data: {nt} trades, {nn} NAV pts."));
    recompute(state);
}
