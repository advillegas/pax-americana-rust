//! IBKR Flex Web Service client + background perf worker.
//!
//! Fetches historical trades and NAV data via IBKR's Flex Web Service HTTPS/XML API,
//! then triggers analytics and chart rendering. Also handles NAV/returns recomputation
//! and PDF export on request.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::state::{LogLevel, SharedState};

const SEND_URL: &str =
    "https://gdcdyn.interactivebrokers.com/Universal/servlet/FlexStatementService.SendRequest";
const GET_URL: &str =
    "https://gdcdyn.interactivebrokers.com/Universal/servlet/FlexStatementService.GetStatement";

pub fn spawn(state: Arc<SharedState>) {
    thread::spawn(move || worker_loop(state));
}

fn worker_loop(state: Arc<SharedState>) {
    loop {
        // Guard: if any operation panics, log it and keep the loop alive.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if state.flex_request.swap(false, Ordering::Relaxed) {
                let (token, query_id) = {
                    let cfg = state.flex_config.lock();
                    (cfg.token.clone(), cfg.query_id.clone())
                };
                if token.is_empty() || query_id.is_empty() {
                    *state.flex_status.lock() = "Set Flex token and Query ID first.".into();
                } else {
                    fetch_and_process(&state, &token, &query_id);
                }
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

fn fetch_and_process(state: &SharedState, token: &str, query_id: &str) {
    *state.flex_status.lock() = "Sending request to IBKR…".into();
    state.log(LogLevel::Info, "Flex: sending request…");

    // SendRequest with auto-retry on transient IBKR errors (rate limit, busy).
    // IBKR limits to 10 requests/min/token and statement generation can take time.
    const RETRY_DELAYS: [u64; 6] = [0, 30, 60, 60, 90, 120];
    let ref_code = 'send: {
        for (retry, &delay) in RETRY_DELAYS.iter().enumerate() {
            if delay > 0 {
                *state.flex_status.lock() = format!("IBKR busy — waiting {delay}s then retry {retry}/{}…", RETRY_DELAYS.len() - 1);
                thread::sleep(Duration::from_secs(delay));
            }

            let url = format!("{SEND_URL}?t={token}&q={query_id}&v=3");
            let body = match ureq::get(&url).call() {
                Ok(resp) => match resp.into_string() {
                    Ok(s) => s,
                    Err(e) => {
                        set_err(state, &format!("Read error: {e}"));
                        return;
                    }
                },
                Err(e) => {
                    set_err(state, &format!("HTTP error: {e}"));
                    return;
                }
            };

            if let Some(c) = extract_tag(&body, "ReferenceCode") {
                break 'send c;
            }
            let msg = extract_tag(&body, "ErrorMessage").unwrap_or_default();
            let transient = msg.contains("try again") || msg.contains("could not be generated")
                || msg.contains("heavy load") || msg.contains("Too many");
            if !transient {
                set_err(state, &format!("Flex: {msg}"));
                return;
            }
        }
        set_err(state, "IBKR unavailable after retries. Wait 5 min, then FETCH again.");
        return;
    };

    *state.flex_status.lock() = format!("Statement generating (ref {ref_code})…");

    let mut attempts = 0;
    let xml = loop {
        attempts += 1;
        if attempts > 30 {
            set_err(state, "Flex statement timed out.");
            return;
        }
        thread::sleep(Duration::from_secs(2));

        let url = format!("{GET_URL}?t={token}&q={ref_code}&v=3");
        let body = match ureq::get(&url).call() {
            Ok(r) => match r.into_string() {
                Ok(s) => s,
                Err(e) => {
                    set_err(state, &format!("Read error: {e}"));
                    return;
                }
            },
            Err(e) => {
                set_err(state, &format!("HTTP error: {e}"));
                return;
            }
        };

        if body.contains("<FlexStatements") || body.contains("<FlexQueryResponse") {
            break body;
        }
        if body.contains("Statement generation in progress") {
            *state.flex_status.lock() = format!("Generating… (attempt {attempts})");
            continue;
        }
        if body.contains("<Status>Fail</Status>") {
            let msg = extract_tag(&body, "ErrorMessage").unwrap_or_else(|| "Unknown".into());
            set_err(state, &format!("Flex error: {msg}"));
            return;
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
        }
        Err(e) => set_err(state, &format!("Parse error: {e}")),
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
