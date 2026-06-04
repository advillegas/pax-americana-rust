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

use pax_core::OrderKind;

use crate::config::{stable_client_id, ClientConfig};
use crate::state::{AccountMode, Candle, ChartView, LogLevel, PortfolioRow, PositionOverlay, RawBar, SharedState};

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
                    let overlay = build_overlay(&client, &book, &account, &sym);
                    state.chart.lock().status = format!("Loading {sym}…");
                    load_into_state(&client, &state, &sym, tf, overlay);
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

/// Timeframe index → (bar size, lookback duration, label).
///
/// The lookback for each bar size is set to the maximum IB allows while
/// keeping the response size reasonable.
///   M1  = 1-min bars,  1 trading day
///   M5  = 5-min bars,  5 trading days
///   M15 = 15-min bars, 10 trading days
///   M30 = 30-min bars, 1 month
///   H1  = 1-hour bars, 2 months
///   H4  = 4-hour bars, 6 months
///   D1  = daily bars,  1 year
///   W1  = weekly bars, 5 years
///   MN  = monthly bars, 10 years
fn timeframe(tf: u8) -> (BarSize, HDuration, &'static str) {
    match tf {
        0 => (BarSize::Min,   HDuration::days(1),    "M1"),
        1 => (BarSize::Min5,  HDuration::days(5),    "M5"),
        2 => (BarSize::Min15, HDuration::days(10),   "M15"),
        3 => (BarSize::Min30, HDuration::months(1),  "M30"),
        4 => (BarSize::Hour,  HDuration::months(2),  "H1"),
        5 => (BarSize::Hour4, HDuration::months(6),  "H4"),
        6 => (BarSize::Day,   HDuration::years(1),   "D1"),
        7 => (BarSize::Week,  HDuration::years(5),   "W1"),
        8 => (BarSize::Month, HDuration::years(10),  "MN"),
        _ => (BarSize::Day,   HDuration::years(1),   "D1"),
    }
}

/// Default number of bars shown on a fresh load (the initial zoom level).
const DEFAULT_VISIBLE: usize = 90;
/// Tightest zoom (fewest bars) and the minimum slice we will render.
const MIN_VISIBLE: usize = 8;

/// Fetch bars for `symbol`, store the full set in shared state, reset the view window to
/// the most-recent `DEFAULT_VISIBLE` bars, then render. The `overlay` carries the open
/// position's avg cost, direction, stop loss, and take profit for chart annotation.
fn load_into_state(client: &Client, state: &SharedState, symbol: &str, tf: u8, overlay: PositionOverlay) {
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
    *state.chart_overlay.lock() = overlay;
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

    let overlay = state.chart_overlay.lock().clone();
    let symbol = state.chart_symbol.lock().clone();
    let label = state.chart_label.lock().clone();
    let markers = state.chart_markers.lock().clone();
    let mut view = render_window(&bars[start..start + count], &symbol, &label, len, &overlay);
    drop(bars);

    if let Some(m) = &markers {
        let lo = view.min_val;
        let hi = view.max_val;
        let span = (hi - lo).max(1e-9);
        let y_of = |p: f32| 100.0 - (p - lo) / span * 100.0;
        view.has_trade_markers = true;
        view.entry_marker_y = y_of(m.entry_price as f32);
        view.exit_marker_y = y_of(m.exit_price as f32);
        view.trade_entry_label = m.entry_label.clone();
        view.trade_exit_label = m.exit_label.clone();
    }

    *state.chart.lock() = view;
    state.chart_gen.fetch_add(1, Ordering::Relaxed);
}

/// Build a `ChartView` for a slice of bars. The price range is taken from the *visible*
/// slice (so zooming/panning rescales the y-axis), expanded to include the overlay prices.
fn render_window(slice: &[RawBar], symbol: &str, label: &str, total: usize, overlay: &PositionOverlay) -> ChartView {
    if slice.is_empty() {
        return ChartView { symbol: symbol.to_string(), status: format!("{symbol}: no data"), ..Default::default() };
    }

    let mut lo = slice.iter().map(|b| b.l).fold(f32::INFINITY, f32::min);
    let mut hi = slice.iter().map(|b| b.h).fold(f32::NEG_INFINITY, f32::max);
    // Expand price range to include overlay levels so they're always visible.
    if overlay.qty.abs() > 0.0 && overlay.avg_cost > 0.0 {
        lo = lo.min(overlay.avg_cost as f32);
        hi = hi.max(overlay.avg_cost as f32);
    }
    if let Some(sp) = overlay.stop_price {
        lo = lo.min(sp as f32);
        hi = hi.max(sp as f32);
    }
    if let Some(tp) = overlay.tp_price {
        lo = lo.min(tp as f32);
        hi = hi.max(tp as f32);
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
            let top = b.o.max(b.c);
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

    let has_pos = overlay.qty.abs() > 0.0 && overlay.avg_cost > 0.0;

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
        // Position overlay
        pos_present: has_pos,
        pos_label: if has_pos {
            let dir = if overlay.is_long { "LONG" } else { "SHORT" };
            format!("{dir} {:.0}", overlay.qty.abs())
        } else {
            String::new()
        },
        pos_long: overlay.is_long,
        entry_y: if has_pos { y_of(overlay.avg_cost as f32) } else { 0.0 },
        entry_label: if has_pos { format!("Entry {:.2}", overlay.avg_cost) } else { String::new() },
        sl_present: overlay.stop_price.is_some(),
        sl_y: overlay.stop_price.map(|p| y_of(p as f32)).unwrap_or(0.0),
        sl_label: overlay.stop_label.clone(),
        tp_present: overlay.tp_price.is_some(),
        tp_y: overlay.tp_price.map(|p| y_of(p as f32)).unwrap_or(0.0),
        tp_label: overlay.tp_label.clone(),
        // Trade markers (filled by rerender's caller)
        has_trade_markers: false,
        entry_marker_y: 0.0,
        exit_marker_y: 0.0,
        trade_entry_label: String::new(),
        trade_exit_label: String::new(),
    }
}

/// Build the position overlay for a symbol: avg cost, direction, and any resting
/// stop-loss / take-profit orders. A stop that would close a long position is a SL;
/// a limit that would close it is a TP. Vice versa for shorts.
fn build_overlay(
    client: &Client,
    book: &BTreeMap<String, PortfolioRow>,
    account: &str,
    symbol: &str,
) -> PositionOverlay {
    let row = match book.get(symbol) {
        Some(r) if r.position.abs() > 0.0 => r,
        _ => return PositionOverlay::default(),
    };

    let is_long = row.position > 0.0;
    let mut overlay = PositionOverlay {
        qty: row.position,
        avg_cost: row.avg_cost,
        is_long,
        ..Default::default()
    };

    // Look up resting orders on this symbol to identify SL and TP.
    if let Ok(orders) = crate::ib::read_open_orders(client, account) {
        for (_id, wo) in &orders {
            if wo.symbol != symbol {
                continue;
            }
            // A protective order closes the position: opposite side to the position.
            let closes = (is_long && wo.side == pax_core::Side::Sell)
                || (!is_long && wo.side == pax_core::Side::Buy);
            if !closes {
                continue;
            }
            match wo.kind {
                OrderKind::Stop | OrderKind::StopLimit => {
                    let price = if wo.aux_price > 0.0 { wo.aux_price } else { wo.limit_price };
                    if price > 0.0 {
                        overlay.stop_price = Some(price);
                        overlay.stop_label = format!("SL {price:.2}");
                    }
                }
                OrderKind::Limit => {
                    if wo.limit_price > 0.0 {
                        overlay.tp_price = Some(wo.limit_price);
                        overlay.tp_label = format!("TP {:.2}", wo.limit_price);
                    }
                }
                _ => {}
            }
        }
    }

    overlay
}

fn sleep(secs: u64) {
    thread::sleep(Duration::from_secs(secs));
}
