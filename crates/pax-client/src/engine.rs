//! The client trading engine.
//!
//! Runs on its own thread for the whole app lifetime, doing work only while the operator
//! has pressed START. Each pass fetches the master snapshot, reads the client's own net
//! positions, reconciles them into delta orders via [`pax_core::reconcile`], and submits
//! those orders — with a per-symbol cooldown so in-flight fills are never double-ordered.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ibapi::client::blocking::Client;
use pax_core::{
    desired_working_orders, diff_working_orders, effective_positions, reconcile, IntentReason,
    ReconcileInput, Side, SizingParams,
};

use crate::config::{stable_client_id, ClientConfig};
use crate::ib;
use crate::master_api::MasterApi;
use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

pub fn spawn(cfg: ClientConfig, state: Arc<SharedState>) {
    thread::spawn(move || engine_main(cfg, state));
}

fn engine_main(cfg: ClientConfig, state: Arc<SharedState>) {
    loop {
        // Idle until the operator starts the engine.
        while !state.is_running() {
            thread::sleep(Duration::from_millis(200));
        }
        run_session(&cfg, &state);
        // Session ended; reflect disconnection and loop back to idle/reconnect.
        state.with_status(|s| {
            s.connected = false;
        });
    }
}

fn run_session(cfg: &ClientConfig, state: &Arc<SharedState>) {
    let (account_mode, host, port_live, port_paper, master_url) = {
        let c = state.controls.lock();
        (c.account_mode, c.ib_host.clone(), c.ib_port_live, c.ib_port_paper, c.master_url.clone())
    };
    // Master URL is read at session start from the GUI-editable controls.
    let api = MasterApi::new(&master_url, &cfg.master_api_key);
    state.log(LogLevel::Info, format!("Master: {master_url}"));
    let port = match account_mode {
        AccountMode::Live => port_live,
        AccountMode::Paper => port_paper,
    };
    let cid = stable_client_id();
    let endpoint = format!("{}:{}", host, port);
    state.log(LogLevel::Info, format!("Connecting to IB {endpoint} clientId={cid}…"));

    let client = match Client::connect(&endpoint, cid) {
        Ok(c) => c,
        Err(e) => {
            state.log(
                LogLevel::Err,
                format!("IB connection failed: {e}. Check Gateway/TWS is running with API enabled."),
            );
            state.running.store(false, Ordering::Relaxed);
            return;
        }
    };

    let account = client
        .managed_accounts()
        .ok()
        .and_then(|a| a.into_iter().next())
        .unwrap_or_default();
    let baseline = ib::read_margin(&client).map(|m| m.net_liq).unwrap_or(0.0);
    state.log(LogLevel::Ok, format!("IB connected ✓ account={account}  balance=${baseline:.2}"));
    state.with_status(|s| {
        s.connected = true;
        s.account = account.clone();
        s.client_balance = baseline;
        s.drawdown_hit = false;
    });

    let mut cooldown: HashMap<String, Instant> = HashMap::new();
    let mut wo_cooldown: HashMap<String, Instant> = HashMap::new();
    let mut drawdown_hit = false;
    let mut last_unreachable_warn: Option<Instant> = None;
    let mut last_margin_warn: Option<Instant> = None;
    let cooldown_dur = Duration::from_secs(cfg.order_cooldown_secs);
    // Symbols the master already held at session start — used by "New Only" mode to
    // suppress opening pre-existing master positions. Captured on first snapshot.
    let mut baseline_symbols: Option<HashSet<String>> = None;

    while state.is_running() {
        // ── one-shot Close All request ───────────────────────────────────────
        if state.close_all.swap(false, Ordering::Relaxed) {
            do_close_all(&client, state);
        }

        // ── balance + margin/SMA + drawdown guards ───────────────────────────
        let margin = ib::read_margin(&client).unwrap_or_default();
        let client_balance = margin.net_liq;
        let (max_dd, min_cushion_pct) = {
            let c = state.controls.lock();
            (c.max_drawdown_pct, c.min_cushion_pct)
        };
        // Block opening/adding when margin is tight: cushion below the floor (when
        // reported), no excess liquidity, or SMA negative (Reg-T/Fed call). De-risking
        // (closes/reduces) is always allowed.
        let opens_blocked = (margin.cushion > 0.0 && margin.cushion * 100.0 < min_cushion_pct)
            || (margin.net_liq > 0.0 && margin.excess_liquidity <= 0.0)
            || (margin.sma < 0.0);
        if opens_blocked {
            let warn = last_margin_warn.map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(true);
            if warn {
                state.log(
                    LogLevel::Warn,
                    format!(
                        "Margin guard: opens blocked (cushion {:.0}%, excess ${:.0}, SMA ${:.0}) — closes still allowed",
                        margin.cushion * 100.0,
                        margin.excess_liquidity,
                        margin.sma
                    ),
                );
                last_margin_warn = Some(Instant::now());
            }
        }
        if baseline > 0.0 && client_balance > 0.0 {
            let dd = (baseline - client_balance) / baseline * 100.0;
            if dd >= max_dd && !drawdown_hit {
                drawdown_hit = true;
                state.log(
                    LogLevel::Err,
                    format!("Max drawdown {max_dd:.1}% hit ({dd:.2}%) — trading halted. Press STOP/START to resume."),
                );
                state.with_status(|s| s.drawdown_hit = true);
            }
        }

        // ── fetch master snapshot ────────────────────────────────────────────
        let snap = match api.snapshot() {
            Ok(s) => s,
            Err(e) => {
                let warn = last_unreachable_warn
                    .map(|t| t.elapsed() > Duration::from_secs(30))
                    .unwrap_or(true);
                if warn {
                    state.log(LogLevel::Warn, format!("Master sync skipped: {e}"));
                    last_unreachable_warn = Some(Instant::now());
                }
                sleep_running(cfg.sync_interval_secs, state);
                continue;
            }
        };

        // ── read our own positions ───────────────────────────────────────────
        let client_positions = match ib::read_positions(&client) {
            Ok(p) => p,
            Err(e) => {
                state.log(LogLevel::Warn, format!("Position read failed: {e} — reconnecting"));
                break; // outer loop reconnects
            }
        };

        // Capture the master's starting structure once per session for New-Only mode.
        if baseline_symbols.is_none() {
            baseline_symbols = Some(snap.positions.iter().map(|p| p.symbol.clone()).collect());
        }

        let controls = state.controls.lock().clone();
        state.with_status(|s| {
            s.client_balance = client_balance;
            s.master_balance = snap.balance;
            s.master_connected = snap.connected;
            s.master_positions = snap.positions.len();
            s.client_positions = client_positions.len();
            s.last_sync = crate::state::now_hms();
            s.excess_liquidity = margin.excess_liquidity;
            s.cushion = margin.cushion;
            s.sma = margin.sma;
            s.margin_blocks_opens = opens_blocked;
        });

        if drawdown_hit {
            sleep_running(cfg.sync_interval_secs, state);
            continue;
        }

        let sizing = SizingParams {
            multiplier: controls.multiplier,
            master_balance: snap.balance,
            client_balance,
            max_position_notional: controls.max_position_notional,
            max_position_qty: controls.max_position_qty,
            force_min_one: true,
        };
        let long_only = controls.trade_mode == TradeMode::LongOnly;

        // ── Channel 1: mirror the master's resting limit/stop orders ───────────
        let desired = desired_working_orders(&snap.working_orders, &sizing, long_only, &client_positions);
        let current_working = ib::read_open_orders(&client).unwrap_or_default();
        let current_wo: Vec<pax_core::WorkingOrder> =
            current_working.iter().map(|(_, w)| w.clone()).collect();
        let wdiff = diff_working_orders(&desired, &current_wo);

        for key in &wdiff.to_cancel {
            if let Some((id, w)) = current_working.iter().find(|(_, w)| &w.key() == key) {
                match ib::cancel_order(&client, *id) {
                    Ok(()) => state.log(LogLevel::Warn, format!("Cancel mirror order {} {}", w.side.as_ib(), w.symbol)),
                    Err(e) => state.log(LogLevel::Err, format!("Cancel failed {}: {e}", w.symbol)),
                }
            }
        }
        for w in &wdiff.to_place {
            // Margin guard: don't place entry (opening) orders when margin is tight.
            // Protective orders (stops/limits that reduce risk) still go through.
            if opens_blocked && w.is_entry {
                continue;
            }
            if let Some(t) = wo_cooldown.get(&w.key()) {
                if t.elapsed() < cooldown_dur {
                    continue;
                }
            }
            match ib::place_order(
                &client, &w.symbol, &w.currency, &w.exchange, w.side, w.quantity, w.kind, w.limit_price, w.aux_price,
            ) {
                Ok(()) => {
                    wo_cooldown.insert(w.key(), Instant::now());
                    state.with_status(|s| s.orders_placed += 1);
                    let lvl = if w.side == Side::Buy { LogLevel::Buy } else { LogLevel::Sell };
                    let px = if w.kind == pax_core::OrderKind::Limit { w.limit_price } else { w.aux_price };
                    state.log(
                        lvl,
                        format!("{:<4} {:<6} qty={:.0} {} @ {:.2} [mirror]", w.side.as_ib(), w.symbol, w.quantity, w.kind.as_ib(), px),
                    );
                }
                Err(e) => {
                    state.with_status(|s| s.orders_failed += 1);
                    state.log(LogLevel::Err, format!("Mirror order failed {}: {e}", w.symbol));
                }
            }
            thread::sleep(Duration::from_millis(250));
        }

        // ── Channel 2: position safety net on EFFECTIVE exposure ───────────────
        // Effective exposure folds *entry* working orders into positions so the safety
        // net won't market-fill what a resting limit order will cover; protective orders
        // ride alongside their position. Market orders here only correct genuine drift.
        let master_eff = effective_positions(&snap.positions, &snap.working_orders);
        let client_eff = effective_positions(&client_positions, &desired);
        let input = ReconcileInput {
            master: &master_eff,
            client: &client_eff,
            master_connected: snap.connected,
            sizing,
            long_only,
            split_zero_cross: true,
            empty_master_guard: 2,
        };
        let result = reconcile(&input);

        if result.blocked {
            if let Some(reason) = result.blocked_reason {
                let warn = last_unreachable_warn
                    .map(|t| t.elapsed() > Duration::from_secs(30))
                    .unwrap_or(true);
                if warn {
                    state.log(LogLevel::Warn, format!("Sync blocked — {reason}"));
                    last_unreachable_warn = Some(Instant::now());
                }
            }
        } else {
            for intent in &result.intents {
                // Margin guard: skip opening/increasing when margin is tight; always
                // allow reduces/closes/flattens (they free margin).
                if opens_blocked && is_opening(intent.reason) {
                    continue;
                }
                // New-Only mode: skip opening positions the master already held at start.
                // Orphan closes and reduces always proceed regardless of mode.
                if controls.execution_mode == ExecutionMode::NewOnly
                    && intent.reason == IntentReason::OpenMissing
                {
                    if let Some(base) = &baseline_symbols {
                        if base.contains(&intent.symbol) {
                            continue;
                        }
                    }
                }
                // Cooldown: don't re-submit a symbol while its last order settles.
                if let Some(t) = cooldown.get(&intent.symbol) {
                    if t.elapsed() < cooldown_dur {
                        continue;
                    }
                }
                match ib::place_order(
                    &client,
                    &intent.symbol,
                    &intent.currency,
                    &intent.exchange,
                    intent.side,
                    intent.qty,
                    intent.kind,
                    intent.limit_price,
                    intent.aux_price,
                ) {
                    Ok(()) => {
                        cooldown.insert(intent.symbol.clone(), Instant::now());
                        let lvl = match intent.side {
                            Side::Buy => LogLevel::Buy,
                            Side::Sell => LogLevel::Sell,
                        };
                        state.log(
                            lvl,
                            format!(
                                "{:<4} {:<6} qty={:.0} {} [{}]",
                                intent.side.as_ib(),
                                intent.symbol,
                                intent.qty,
                                intent.kind.as_ib(),
                                intent.reason.label()
                            ),
                        );
                        if is_closing(intent.reason) {
                            state.with_status(|s| s.orders_closed += 1);
                        } else {
                            state.with_status(|s| s.orders_placed += 1);
                        }
                    }
                    Err(e) => {
                        state.with_status(|s| s.orders_failed += 1);
                        state.log(LogLevel::Err, format!("Order failed {}: {e}", intent.symbol));
                    }
                }
                thread::sleep(Duration::from_millis(300));
            }
        }

        sleep_running(cfg.sync_interval_secs, state);
    }

    state.log(LogLevel::Warn, "Engine session ended.");
}

fn do_close_all(client: &Client, state: &Arc<SharedState>) {
    state.log(LogLevel::Warn, "CLOSE ALL — cancelling working orders and flattening positions…");
    if let Err(e) = ib::cancel_all(client) {
        state.log(LogLevel::Warn, format!("Cancel-all warning: {e}"));
    }
    let positions = ib::read_positions(client).unwrap_or_default();
    if positions.is_empty() {
        state.log(LogLevel::Ok, "Close all — no open positions.");
        return;
    }
    for p in &positions {
        let side = Side::closing(p.net_qty);
        let qty = p.net_qty.abs();
        match ib::place_order(
            client,
            &p.symbol,
            &p.currency,
            &p.exchange,
            side,
            qty,
            pax_core::OrderKind::Market,
            0.0,
            0.0,
        ) {
            Ok(_) => {
                state.with_status(|s| s.orders_closed += 1);
                state.log(LogLevel::Warn, format!("Close all: {} {} qty={:.0} MKT", side.as_ib(), p.symbol, qty));
            }
            Err(e) => {
                state.with_status(|s| s.orders_failed += 1);
                state.log(LogLevel::Err, format!("Close all failed {}: {e}", p.symbol));
            }
        }
        thread::sleep(Duration::from_millis(300));
    }
    state.log(LogLevel::Ok, "Close all submitted.");
}

fn is_closing(reason: IntentReason) -> bool {
    matches!(
        reason,
        IntentReason::CloseOrphan | IntentReason::ReduceToTarget | IntentReason::FlattenLeg
    )
}

fn is_opening(reason: IntentReason) -> bool {
    matches!(
        reason,
        IntentReason::OpenMissing | IntentReason::IncreaseToTarget | IntentReason::OpenLeg
    )
}

/// Sleep `secs`, but wake promptly (250ms granularity) if the engine is stopped.
fn sleep_running(secs: u64, state: &Arc<SharedState>) {
    let mut remaining = secs * 4;
    while remaining > 0 {
        if !state.is_running() {
            return;
        }
        thread::sleep(Duration::from_millis(250));
        remaining -= 1;
    }
}
