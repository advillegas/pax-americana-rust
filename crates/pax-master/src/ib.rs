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
use pax_core::{MasterSnapshot, OrderKind, Position, PROTOCOL_SCHEMA};

use crate::config::MasterConfig;
use crate::state::{now_ms, LogLevel, SharedState};

/// How often to refresh NetLiquidation (positions stream in real time independently).
const BALANCE_REFRESH_SECS: u64 = 5;
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
    let endpoint = cfg.ib_endpoint();
    let _ = cfg.refresh_secs; // balance cadence is fixed; positions are event-driven now

    while !stop.load(Ordering::Relaxed) {
        state.log(
            LogLevel::Info,
            format!("Connecting to IB at {endpoint} (clientId={})…", cfg.ib_client_id),
        );

        let client = match Client::connect(&endpoint, cfg.ib_client_id) {
            Ok(c) => c,
            Err(e) => {
                state.log(LogLevel::Err, format!("IB connect failed: {e}. Retrying in 10s…"));
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
        let mut last_balance = Instant::now();

        // Steady-state event loop.
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
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

            // Republish the snapshot (cheap; keeps liveness timestamp fresh).
            {
                let mut snap = state.snapshot.lock();
                *snap = MasterSnapshot {
                    schema: PROTOCOL_SCHEMA,
                    connected: true,
                    account: account.clone(),
                    balance,
                    positions: book.values().cloned().collect(),
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

fn nonempty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}
