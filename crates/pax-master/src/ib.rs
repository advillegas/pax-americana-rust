//! IB Gateway / TWS worker thread for the master.
//!
//! Owns its own blocking `ibapi` client (one-client-per-thread model) and continuously
//! republishes the master's net positions and NetLiquidation balance into shared state.
//! All IB calls happen here; the GUI and HTTP server only read the shared snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ibapi::accounts::types::AccountGroup;
use ibapi::accounts::{AccountSummaryResult, PositionUpdate};
use ibapi::client::blocking::Client;
use pax_core::{MasterSnapshot, OrderKind, Position, PROTOCOL_SCHEMA};

use crate::config::MasterConfig;
use crate::state::{now_ms, LogLevel, SharedState};

/// Spawn the master IB worker. Returns a handle; set the flag to stop.
pub fn spawn(cfg: MasterConfig, state: Arc<SharedState>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_ret = stop.clone();
    thread::spawn(move || worker_loop(cfg, state, stop));
    stop_ret
}

fn worker_loop(cfg: MasterConfig, state: Arc<SharedState>, stop: Arc<AtomicBool>) {
    let endpoint = cfg.ib_endpoint();
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
        state.log(LogLevel::Ok, format!("IB connected ✓ account={account}"));

        // Steady-state refresh loop. Any error breaks out to reconnect.
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }

            let positions = match read_positions(&client) {
                Ok(p) => p,
                Err(e) => {
                    state.log(LogLevel::Warn, format!("Position read failed: {e} — reconnecting"));
                    break;
                }
            };
            let balance = read_balance(&client).unwrap_or(0.0);

            {
                let mut snap = state.snapshot.lock();
                *snap = MasterSnapshot {
                    schema: PROTOCOL_SCHEMA,
                    connected: true,
                    account: account.clone(),
                    balance,
                    positions,
                    generated_at_ms: now_ms(),
                };
            }

            sleep_interruptible(cfg.refresh_secs.max(1), &stop);
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

/// Collect a full net-position snapshot, draining the subscription until `PositionEnd`.
/// Sync subscriptions yield the value directly (errors surface via `.error()`).
fn read_positions(client: &Client) -> Result<Vec<Position>, String> {
    let sub = client.positions().map_err(|e| e.to_string())?;
    let mut out: Vec<Position> = Vec::new();
    for update in &sub {
        match update {
            PositionUpdate::Position(p) => {
                let qty = p.position;
                if qty == 0.0 {
                    continue;
                }
                out.push(Position {
                    symbol: p.contract.symbol.to_string(),
                    currency: nonempty(p.contract.currency.to_string(), "USD"),
                    exchange: nonempty(p.contract.exchange.to_string(), "SMART"),
                    net_qty: qty,
                    avg_cost: p.average_cost,
                    order_kind: OrderKind::Market,
                    limit_price: 0.0,
                    aux_price: 0.0,
                });
            }
            PositionUpdate::PositionEnd => break,
        }
    }
    if let Some(e) = sub.error() {
        return Err(e.to_string());
    }
    Ok(out)
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
