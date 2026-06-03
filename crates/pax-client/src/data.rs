//! Read-only market/account data connection for the Portfolio and Charts tabs.
//!
//! This runs on its own thread with its OWN IB connection (a different clientId from the
//! trading engine) so portfolio valuation and chart requests never interfere with the
//! reconcile loop. It streams account/portfolio updates and serves on-demand historical
//! bars. It places no orders.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ibapi::accounts::types::AccountId;
use ibapi::accounts::AccountUpdate;
use ibapi::client::blocking::Client;
use ibapi::contracts::Contract;
use ibapi::market_data::historical::{BarSize, Duration as HDuration, WhatToShow};
use ibapi::market_data::TradingHours;

use crate::config::{stable_client_id, ClientConfig};
use crate::state::{AccountMode, Candle, ChartView, LogLevel, PortfolioRow, RawBar, SharedState};

pub fn spawn(cfg: ClientConfig, state: Arc<SharedState>) {
    thread::spawn(move || data_main(cfg, state));
}

fn data_main(_cfg: ClientConfig, state: Arc<SharedState>) {
    loop {
        // Connect with the same host/port as the engine but a distinct clientId.
        let (mode, host, live, paper, want_account) = {
            let c = state.controls.lock();
            (c.account_mode, c.ib_host.clone(), c.ib_port_live, c.ib_port_paper, c.ib_account.trim().to_string())
        };
        let port = match mode {
            AccountMode::Live => live,
            AccountMode::Paper => paper,
        };
        let endpoint = format!("{host}:{port}");
        let cid = stable_client_id().wrapping_add(2);

        let client = match Client::connect(&endpoint, cid) {
            Ok(c) => c,
            Err(_) => {
                state.data_connected.store(false, Ordering::Relaxed);
                sleep(10);
                continue;
            }
        };

        // Resolve the account to view: the configured one if present, else the sole/first.
        let accounts = client.managed_accounts().unwrap_or_default();
        let account = if !want_account.is_empty() && accounts.iter().any(|a| a == &want_account) {
            want_account.clone()
        } else {
            accounts.into_iter().next().unwrap_or_default()
        };
        if account.trim().is_empty() {
            state.data_connected.store(false, Ordering::Relaxed);
            sleep(10);
            continue;
        }

        let sub = match client.account_updates(&AccountId(account.clone())) {
            Ok(s) => s,
            Err(_) => {
                state.data_connected.store(false, Ordering::Relaxed);
                sleep(5);
                continue;
            }
        };
        state.data_connected.store(true, Ordering::Relaxed);
        state.log(LogLevel::Info, format!("Portfolio data connected (account={account})."));

        let mut book: BTreeMap<String, PortfolioRow> = BTreeMap::new();
        let mut last_publish = Instant::now();
        let mut errored = false;

        loop {
            // Reconnect if the operator changed the connection target.
            let changed = {
                let c = state.controls.lock();
                c.account_mode != mode || c.ib_host != host
            };
            if changed {
                break;
            }

            while let Some(u) = sub.try_next() {
                if let AccountUpdate::PortfolioValue(p) = u {
                    let sym = p.contract.symbol.to_string();
                    if p.position == 0.0 {
                        book.remove(&sym);
                    } else {
                        book.insert(
                            sym.clone(),
                            PortfolioRow {
                                symbol: sym,
                                position: p.position,
                                market_price: p.market_price,
                                market_value: p.market_value,
                                avg_cost: p.average_cost,
                                unrealized_pnl: p.unrealized_pnl,
                            },
                        );
                    }
                }
            }
            if sub.error().is_some() {
                errored = true;
            }

            // Publish a sorted snapshot a few times a second.
            if last_publish.elapsed() >= Duration::from_millis(500) {
                let mut v: Vec<PortfolioRow> = book.values().cloned().collect();
                v.sort_by(|a, b| a.symbol.cmp(&b.symbol));
                *state.portfolio.lock() = v;
                last_publish = Instant::now();
            }

            // Serve a chart request (on-demand historical bars).
            if state.chart_request.swap(false, Ordering::Relaxed) {
                let symbol = state.chart_symbol.lock().clone();
                let tf = state.chart_tf.load(Ordering::Relaxed);
                let sym = symbol.trim().to_string();
                if !sym.is_empty() {
                    // Chart the open-position average cost if we hold the symbol.
                    let avg = book.get(&sym).map(|r| r.avg_cost).filter(|c| *c > 0.0);
                    state.chart.lock().status = format!("Loading {sym}…");
                    load_into_state(&client, &state, &sym, tf, avg);
                }
            }

            if errored {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }

        state.data_connected.store(false, Ordering::Relaxed);
        sleep(3);
    }
}

/// Timeframe → (bar size, lookback duration, label).
fn timeframe(tf: u8) -> (BarSize, HDuration, &'static str) {
    match tf {
        0 => (BarSize::Min5, HDuration::days(1), "1D"),
        1 => (BarSize::Min30, HDuration::days(5), "1W"),
        2 => (BarSize::Day, HDuration::months(1), "1M"),
        4 => (BarSize::Day, HDuration::years(1), "1Y"),
        _ => (BarSize::Day, HDuration::months(6), "6M"),
    }
}

/// Default number of bars shown on a fresh load (the initial zoom level).
const DEFAULT_VISIBLE: usize = 90;
/// Tightest zoom (fewest bars) and the minimum slice we will render.
const MIN_VISIBLE: usize = 8;

/// Fetch bars for `symbol`, store the full set in shared state, reset the view window to
/// the most-recent `DEFAULT_VISIBLE` bars, then render. `avg` (open-position average cost)
/// is charted as a horizontal line and folded into the price range.
fn load_into_state(client: &Client, state: &SharedState, symbol: &str, tf: u8, avg: Option<f64>) {
    let (bar_size, duration, label) = timeframe(tf);
    let contract = Contract::stock(symbol).on_exchange("SMART").in_currency("USD").build();

    let hd = match client.historical_data(&contract, None, duration, bar_size, WhatToShow::Trades, TradingHours::Regular) {
        Ok(h) => h,
        Err(e) => {
            *state.chart_bars.lock() = Vec::new();
            *state.chart.lock() = ChartView {
                symbol: symbol.to_string(),
                status: format!("{symbol}: chart unavailable ({e})"),
                ..Default::default()
            };
            state.chart_gen.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    if hd.bars.len() < 2 {
        *state.chart_bars.lock() = Vec::new();
        *state.chart.lock() = ChartView {
            symbol: symbol.to_string(),
            status: format!("{symbol}: no data (market-data permissions?)"),
            ..Default::default()
        };
        state.chart_gen.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let raw: Vec<RawBar> = hd
        .bars
        .iter()
        .map(|b| RawBar { o: b.open as f32, h: b.high as f32, l: b.low as f32, c: b.close as f32 })
        .collect();
    let len = raw.len();
    let count = len.min(DEFAULT_VISIBLE).max(MIN_VISIBLE.min(len));

    *state.chart_bars.lock() = raw;
    *state.chart_avg.lock() = avg;
    *state.chart_symbol.lock() = symbol.to_string();
    *state.chart_label.lock() = label.to_string();
    state.chart_count.store(count, Ordering::Relaxed);
    state.chart_start.store(len - count, Ordering::Relaxed);
    rerender(state);
}

/// Re-window the stored bars using the current `chart_start` / `chart_count` and publish a
/// fresh `ChartView`. Called both after a load (data thread) and on every pan/zoom (GUI
/// thread) — it never touches IB, so interaction stays snappy.
pub fn rerender(state: &SharedState) {
    let bars = state.chart_bars.lock();
    let len = bars.len();
    if len == 0 {
        return;
    }
    // Clamp the window to the available data and write the clamped values back.
    let count = state.chart_count.load(Ordering::Relaxed).clamp(MIN_VISIBLE.min(len).max(1), len);
    let start = state.chart_start.load(Ordering::Relaxed).min(len - count);
    state.chart_count.store(count, Ordering::Relaxed);
    state.chart_start.store(start, Ordering::Relaxed);

    let avg = *state.chart_avg.lock();
    let symbol = state.chart_symbol.lock().clone();
    let label = state.chart_label.lock().clone();
    let view = render_window(&bars[start..start + count], &symbol, &label, len, avg);
    drop(bars);

    *state.chart.lock() = view;
    state.chart_gen.fetch_add(1, Ordering::Relaxed);
}

/// Build a `ChartView` for a slice of bars. The price range is taken from the *visible*
/// slice (so zooming/panning rescales the y-axis), plus the avg line when present.
fn render_window(slice: &[RawBar], symbol: &str, label: &str, total: usize, avg: Option<f64>) -> ChartView {
    if slice.is_empty() {
        return ChartView { symbol: symbol.to_string(), status: format!("{symbol}: no data"), ..Default::default() };
    }

    let mut lo = slice.iter().map(|b| b.l).fold(f32::INFINITY, f32::min);
    let mut hi = slice.iter().map(|b| b.h).fold(f32::NEG_INFINITY, f32::max);
    if let Some(a) = avg {
        lo = lo.min(a as f32);
        hi = hi.max(a as f32);
    }
    let span = (hi - lo).max(1e-9);
    let y_of = |p: f32| 100.0 - (p - lo) / span * 100.0;

    let n = slice.len();
    let slot = 100.0 / n as f32;
    let bw = slot * 0.7;
    let candles: Vec<Candle> = slice
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let top = b.o.max(b.c); // higher price → smaller y
            let bot = b.o.min(b.c);
            Candle {
                cx: (i as f32 + 0.5) * slot,
                bw,
                high_y: y_of(b.h),
                low_y: y_of(b.l),
                top_y: y_of(top),
                bot_y: y_of(bot),
                up: b.c >= b.o,
            }
        })
        .collect();

    let first = slice[0].c;
    let last = slice[n - 1].c;
    let chg_pct = if first.abs() > 1e-9 { (last - first) / first * 100.0 } else { 0.0 };

    ChartView {
        symbol: symbol.to_string(),
        status: format!("{symbol} · {label} · {n}/{total} bars  (drag/scroll to pan, =/- to zoom)"),
        candles,
        min_val: lo,
        max_val: hi,
        min_label: format!("{lo:.2}"),
        max_label: format!("{hi:.2}"),
        last_label: format!("{last:.2}  ({chg_pct:+.2}%)"),
        up: last >= first,
        avg_present: avg.is_some(),
        avg_y: avg.map(|a| y_of(a as f32)).unwrap_or(0.0),
        avg_label: avg.map(|a| format!("avg {a:.2}")).unwrap_or_default(),
    }
}

fn sleep(secs: u64) {
    thread::sleep(Duration::from_secs(secs));
}
