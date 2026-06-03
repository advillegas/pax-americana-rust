//! IB Gateway / TWS worker thread for the master.
//!
//! Owns its own blocking `ibapi` client (one-client-per-thread model). It holds a
//! **persistent streaming position subscription** so position changes in TWS propagate
//! into the shared snapshot the instant they happen, and refreshes NetLiquidation on a
//! light timer. All IB calls happen here; the GUI and HTTP server only read the snapshot.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ibapi::accounts::types::AccountGroup;
use ibapi::accounts::{AccountSummaryResult, PositionUpdate};
use ibapi::client::blocking::Client;
use ibapi::orders::{Action, Orders};
use pax_core::{MasterSnapshot, OrderKind, Position, WorkingOrder, PROTOCOL_SCHEMA};

use crate::config::MasterConfig;
use crate::state::{now_ms, ConnParams, IbMode, LogLevel, SharedState};

/// How often to refresh NetLiquidation (positions stream in real time independently).
const BALANCE_REFRESH_SECS: u64 = 5;
/// How often to poll working (resting) orders.
const ORDERS_REFRESH_MS: u64 = 1000;
/// How often to republish the snapshot (keeps `generated_at_ms` fresh for liveness).
const REPUBLISH_MS: u64 = 200;

/// Spawn the master IB worker. Returns a handle; set the flag to stop.
pub fn spawn(cfg: MasterConfig, state: Arc<SharedState>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_ret = stop.clone();
    thread::spawn(move || worker_loop(cfg, state, stop));
    stop_ret
}

fn worker_loop(cfg: MasterConfig, state: Arc<SharedState>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        // Params are read fresh each attempt so GUI edits (host/port/mode) take effect on
        // (re)connect. `gen` lets us notice an Apply/toggle and reconnect.
        let gen = state.reconnect_gen();
        let params = state.conn.lock().clone();

        // Probe IB endpoints until one accepts the API handshake. We rotate both the port
        // (selected mode first, then the other configured port + TWS/Gateway defaults) and
        // the clientId. The master is READ-ONLY (it only streams positions/orders and never
        // places trades), so connecting to whichever IB endpoint is actually open is safe
        // and frees the operator from guessing Live/Paper or TWS/Gateway port numbers.
        // Rotating clientIds also recovers from a stale/duplicate session (common on
        // clientId 0) where the socket connects but the handshake fails.
        let client = match connect_with_fallback(&params, cfg.ib_client_id, &state, &stop) {
            Some(c) => c,
            None => {
                mark_disconnected(&state);
                sleep_interruptible(10, &stop);
                continue;
            }
        };

        let account = client
            .managed_accounts()
            .ok()
            .and_then(|a| a.into_iter().next())
            .unwrap_or_default();
        state.log(LogLevel::Ok, format!("IB connected ✓ account={account} — streaming positions"));

        // Open the persistent streaming position subscription: full replay first, then
        // live incremental updates as positions change in TWS.
        let pos_sub = match client.positions() {
            Ok(s) => s,
            Err(e) => {
                state.log(LogLevel::Warn, format!("Position stream failed: {e} — reconnecting"));
                mark_disconnected(&state);
                sleep_interruptible(5, &stop);
                continue;
            }
        };

        let mut book: BTreeMap<String, Position> = BTreeMap::new();
        let mut balance = read_balance(&client).unwrap_or(0.0);
        let mut working: Vec<WorkingOrder> = Vec::new();
        let mut last_balance = Instant::now();
        let mut last_orders = Instant::now()
            .checked_sub(Duration::from_millis(ORDERS_REFRESH_MS))
            .unwrap_or_else(Instant::now);

        // Steady-state event loop.
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }

            // Operator changed connection params (host/port/mode) and applied — drop
            // this connection and reconnect with the new params.
            if state.reconnect_gen() != gen {
                state.log(LogLevel::Warn, "Connection settings changed — reconnecting…");
                break;
            }

            // Drain all pending position updates without blocking.
            while let Some(update) = pos_sub.try_next() {
                match update {
                    PositionUpdate::Position(p) => {
                        let sym = p.contract.symbol.to_string();
                        if p.position == 0.0 {
                            book.remove(&sym);
                        } else {
                            book.insert(
                                sym.clone(),
                                Position {
                                    symbol: sym,
                                    currency: nonempty(p.contract.currency.to_string(), "USD"),
                                    exchange: nonempty(p.contract.exchange.to_string(), "SMART"),
                                    net_qty: p.position,
                                    avg_cost: p.average_cost,
                                    order_kind: OrderKind::Market,
                                    limit_price: 0.0,
                                    aux_price: 0.0,
                                },
                            );
                        }
                    }
                    PositionUpdate::PositionEnd => {} // end of initial replay; updates follow
                }
            }

            // Stream died (e.g. TWS dropped) → break out to reconnect.
            if let Some(e) = pos_sub.error() {
                state.log(LogLevel::Warn, format!("Position stream error: {e} — reconnecting"));
                break;
            }

            // Light periodic balance refresh.
            if last_balance.elapsed() >= Duration::from_secs(BALANCE_REFRESH_SECS) {
                if let Ok(b) = read_balance(&client) {
                    balance = b;
                }
                last_balance = Instant::now();
            }

            // Periodic working-order capture.
            if last_orders.elapsed() >= Duration::from_millis(ORDERS_REFRESH_MS) {
                if let Ok(w) = read_working_orders(&client, &book) {
                    working = w;
                }
                last_orders = Instant::now();
            }

            // Republish the snapshot (cheap; keeps liveness timestamp fresh).
            {
                let mut snap = state.snapshot.lock();
                *snap = MasterSnapshot {
                    schema: PROTOCOL_SCHEMA,
                    connected: true,
                    account: account.clone(),
                    balance,
                    positions: book.values().cloned().collect(),
                    working_orders: working.clone(),
                    generated_at_ms: now_ms(),
                };
            }

            thread::sleep(Duration::from_millis(REPUBLISH_MS));
        }

        mark_disconnected(&state);
        sleep_interruptible(5, &stop);
    }
}

fn mark_disconnected(state: &SharedState) {
    let mut snap = state.snapshot.lock();
    snap.connected = false;
    snap.generated_at_ms = now_ms();
}

/// Read NetLiquidation (USD) via an account-summary request. The tag is passed as a
/// plain string so we don't depend on a specific associated-constant name.
fn read_balance(client: &Client) -> Result<f64, String> {
    let group = AccountGroup("All".to_string());
    let sub = client
        .account_summary(&group, &["NetLiquidation"])
        .map_err(|e| e.to_string())?;
    let mut balance = 0.0_f64;
    for item in &sub {
        match item {
            AccountSummaryResult::Summary(s) => {
                if s.tag == "NetLiquidation" {
                    if let Ok(v) = s.value.parse::<f64>() {
                        balance = v;
                    }
                }
            }
            AccountSummaryResult::End => break,
        }
    }
    Ok(balance)
}

/// Snapshot the master's resting limit/stop/stop-limit orders, tagging each as an entry
/// (opens/adds exposure) or protective (reduces an existing position) using `book`.
fn read_working_orders(
    client: &Client,
    book: &BTreeMap<String, Position>,
) -> Result<Vec<WorkingOrder>, String> {
    let sub = client.all_open_orders().map_err(|e| e.to_string())?;
    let mut out: Vec<WorkingOrder> = Vec::new();
    for item in &sub {
        if let Orders::OrderData(d) = item {
            let kind = OrderKind::from_ib(&d.order.order_type);
            // Only resting order types are worth mirroring; skip market/other.
            if matches!(kind, OrderKind::Market) {
                continue;
            }
            let side = match d.order.action {
                Action::Buy => pax_core::Side::Buy,
                _ => pax_core::Side::Sell,
            };
            let qty = d.order.total_quantity.abs();
            if qty == 0.0 {
                continue;
            }
            let signed = match side {
                pax_core::Side::Buy => qty,
                pax_core::Side::Sell => -qty,
            };
            let pos = book.get(&d.contract.symbol.to_string()).map(|p| p.net_qty).unwrap_or(0.0);
            let is_entry = pos == 0.0 || pos.signum() == signed.signum();

            out.push(WorkingOrder {
                symbol: d.contract.symbol.to_string(),
                currency: nonempty(d.contract.currency.to_string(), "USD"),
                exchange: nonempty(d.contract.exchange.to_string(), "SMART"),
                side,
                quantity: qty,
                kind,
                limit_price: d.order.limit_price.unwrap_or(0.0),
                aux_price: d.order.aux_price.unwrap_or(0.0),
                is_entry,
            });
        }
    }
    Ok(out)
}

/// Sleep up to `secs`, waking early (every 250ms) if the stop flag is set.
fn sleep_interruptible(secs: u64, stop: &AtomicBool) {
    let mut remaining = secs * 4;
    while remaining > 0 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(Duration::from_millis(250));
        remaining -= 1;
    }
}

/// Candidate clientIds to try, in order: the configured one first, then low alternates.
/// (Master alternates stay low; clients hash into 21..=999, so they never collide here.)
fn client_id_candidates(configured: i32) -> Vec<i32> {
    let mut ids = vec![configured];
    for c in [1, 2, 3, 4, 5, 10] {
        if !ids.contains(&c) {
            ids.push(c);
        }
    }
    ids
}

/// Ports to probe, in order: the selected mode's port first, then the other configured
/// port, then the standard TWS/Gateway defaults. Deduplicated, blanks skipped.
fn port_candidates(params: &ConnParams) -> Vec<(u16, &'static str)> {
    let mut out: Vec<(u16, &'static str)> = Vec::new();
    let mut push = |p: u16, label: &'static str| {
        if p != 0 && !out.iter().any(|(q, _)| *q == p) {
            out.push((p, label));
        }
    };
    match params.mode {
        IbMode::Live => {
            push(params.port_live, "live");
            push(params.port_paper, "paper");
        }
        IbMode::Paper => {
            push(params.port_paper, "paper");
            push(params.port_live, "live");
        }
    }
    push(7496, "TWS live");
    push(7497, "TWS paper");
    push(4001, "Gateway live");
    push(4002, "Gateway paper");
    out
}

/// Probe IB endpoints (port × clientId) until one accepts the API handshake. A refused
/// port is abandoned immediately (nothing is listening); a handshake failure rotates the
/// clientId on the same port. Returns `None` if nothing reachable or a stop was requested.
fn connect_with_fallback(
    params: &ConnParams,
    configured_id: i32,
    state: &SharedState,
    stop: &AtomicBool,
) -> Option<Client> {
    let ids = client_id_candidates(configured_id);
    let id_last = ids.len().saturating_sub(1);
    for (port, label) in port_candidates(params) {
        let endpoint = format!("{}:{}", params.host, port);
        for (i, &cid) in ids.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            state.log(
                LogLevel::Info,
                format!("Connecting to IB at {endpoint} [{label}] (clientId={cid})…"),
            );
            match Client::connect(&endpoint, cid) {
                Ok(c) => {
                    state.log(
                        LogLevel::Ok,
                        format!("IB connected on {endpoint} [{label}] clientId={cid}"),
                    );
                    return Some(c);
                }
                Err(e) => {
                    let msg = e.to_string();
                    // Connection refused → nothing on this port; stop trying ids here.
                    if msg.contains("refused") || msg.contains("10061") {
                        state.log(
                            LogLevel::Warn,
                            format!("No IB listener on {endpoint} — trying next port"),
                        );
                        break;
                    }
                    state.log(
                        LogLevel::Err,
                        format!("IB connect failed on {endpoint} clientId={cid}: {e}"),
                    );
                    // Let TWS settle before the next id (skip the wait after the last).
                    if i < id_last {
                        sleep_interruptible(1, stop);
                    }
                }
            }
        }
    }
    state.log(
        LogLevel::Err,
        "No reachable IB endpoint — check TWS/Gateway is running with API enabled. Retrying in 10s…"
            .to_string(),
    );
    None
}

fn nonempty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}
