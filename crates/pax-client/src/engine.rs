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
use ibapi::orders::OrderUpdate;
use pax_core::{
    desired_working_orders, diff_working_orders, effective_positions, reconcile, IntentReason,
    ReconcileInput, Side, SizingParams,
};

use crate::config::{stable_client_id, ClientConfig};
use crate::ib;
use crate::license::{self, LicenseStatus};
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
    let api = MasterApi::new(&master_url, &cfg.master_api_key);
    state.log(LogLevel::Info, format!("Server: {master_url}"));
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

    // Resolve EXACTLY ONE account to operate on. If the login manages several and none is
    // configured, refuse to trade — never act across accounts (which could touch/flatten
    // positions in an account the operator didn't intend).
    let accounts = client.managed_accounts().unwrap_or_default();
    let configured = state.controls.lock().ib_account.trim().to_string();
    let account = if !configured.is_empty() {
        if accounts.iter().any(|a| a == &configured) {
            configured
        } else {
            let avail = if accounts.is_empty() { "none".to_string() } else { accounts.join(", ") };
            state.log(LogLevel::Err, format!("Account '{configured}' not on this login (available: {avail}). Stopping."));
            state.running.store(false, Ordering::Relaxed);
            return;
        }
    } else {
        match accounts.len() {
            1 => accounts[0].clone(),
            0 => {
                state.log(LogLevel::Err, "No account available on this login. Stopping.");
                state.running.store(false, Ordering::Relaxed);
                return;
            }
            _ => {
                state.log(
                    LogLevel::Err,
                    format!(
                        "This login manages multiple accounts ({}). Set the Account field to choose one — refusing to trade across accounts.",
                        accounts.join(", ")
                    ),
                );
                state.running.store(false, Ordering::Relaxed);
                return;
            }
        }
    };
    let baseline = ib::read_margin(&client, &account).map(|m| m.net_liq).unwrap_or(0.0);
    state.log(LogLevel::Ok, format!("IB connected ✓ account={account}  balance=${baseline:.2}"));
    state.with_status(|s| {
        s.connected = true;
        s.account = account.clone();
        s.client_balance = baseline;
        s.drawdown_hit = false;
    });

    // ── license gate ─────────────────────────────────────────────────────────
    match license::check(&account) {
        LicenseStatus::Authorized => state.log(LogLevel::Ok, "License verified ✓"),
        LicenseStatus::Denied => {
            state.log(
                LogLevel::Err,
                format!("Account {account} is not licensed — contact support@neroai.com. Stopping."),
            );
            state.running.store(false, Ordering::Relaxed);
            return;
        }
        LicenseStatus::Unknown => {
            state.log(LogLevel::Err, "Could not verify license (endpoint unreachable). Stopping.");
            state.running.store(false, Ordering::Relaxed);
            return;
        }
    }
    let mut last_license = Instant::now();

    // Order lifecycle stream (fills, rejections, IBKR notices).
    let order_stream = client.order_update_stream().ok();

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
        // ── periodic license re-check (revocation stops trading) ──────────────
        if last_license.elapsed() >= Duration::from_secs(60) {
            last_license = Instant::now();
            if license::check(&account) == LicenseStatus::Denied {
                state.log(LogLevel::Err, "License revoked — stopping engine.");
                state.stop_engine();
                break;
            }
        }

        // ── drain order-status updates (fills / rejections / IBKR notices) ────
        if let Some(os) = &order_stream {
            while let Some(upd) = os.try_next() {
                match upd {
                    OrderUpdate::ExecutionData(d) => {
                        let lvl = if d.execution.side == "SLD" { LogLevel::Sell } else { LogLevel::Buy };
                        state.log(
                            lvl,
                            format!(
                                "FILL {} {} {:.0} @ {:.2}",
                                d.execution.side,
                                d.contract.symbol,
                                d.execution.shares,
                                d.execution.price
                            ),
                        );
                    }
                    OrderUpdate::OrderStatus(s) => {
                        if s.status == "Inactive" {
                            state.log(
                                LogLevel::Err,
                                format!("Order {} rejected/inactive (filled {:.0}, rem {:.0})", s.order_id, s.filled, s.remaining),
                            );
                            state.with_status(|st| st.orders_failed += 1);
                        }
                    }
                    OrderUpdate::Message(n) => {
                        if !is_notice_noise(n.code) {
                            state.log(LogLevel::Warn, format!("IBKR [{}] {}", n.code, n.message));
                        }
                    }
                    _ => {}
                }
            }
        }

        // ── one-shot Close All request ───────────────────────────────────────
        if state.close_all.swap(false, Ordering::Relaxed) {
            do_close_all(&client, state, &account);
        }

        // ── balance + margin/SMA + drawdown guards ───────────────────────────
        let margin = ib::read_margin(&client, &account).unwrap_or_default();
        let client_balance = margin.net_liq;
        let max_dd = state.controls.lock().max_drawdown_pct;
        // SMA < 0 is a Reg-T / Fed call — block ALL opening regardless of buying power.
        // Otherwise each opening order is gated by an exact what-if margin check below.
        let sma_call = margin.sma < 0.0;
        // Live buying power for THIS cycle. Each opening order decrements it by its
        // what-if initial-margin requirement, so cumulative opens can't over-commit.
        let mut projected_available = margin.available_funds;
        if sma_call {
            let warn = last_margin_warn.map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(true);
            if warn {
                state.log(
                    LogLevel::Err,
                    format!("SMA negative (${:.0}) — Reg-T call; opens blocked, closes allowed", margin.sma),
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

        // ── read OUR OWN positions (independent of the server) ────────────────
        let client_positions = match ib::read_positions(&client, &account) {
            Ok(p) => p,
            Err(e) => {
                state.log(LogLevel::Warn, format!("Position read failed: {e} — reconnecting"));
                break; // outer loop reconnects to IB
            }
        };

        // Always reflect our own IB state in the GUI, whether or not the server is up.
        state.with_status(|s| {
            s.client_balance = client_balance;
            s.client_positions = client_positions.len();
            s.excess_liquidity = margin.excess_liquidity;
            s.cushion = margin.cushion;
            s.sma = margin.sma;
            s.margin_blocks_opens = sma_call;
        });

        if drawdown_hit {
            sleep_running(cfg.sync_interval_secs, state);
            continue;
        }

        // ── fetch the server snapshot (entirely separate from the IB link) ─────
        let snap = match api.snapshot() {
            Ok(s) => {
                state.with_status(|st| {
                    st.master_balance = s.balance;
                    st.master_connected = s.connected;
                    st.master_positions = s.positions.len();
                    st.last_sync = crate::state::now_hms();
                });
                s
            }
            Err(e) => {
                let warn = last_unreachable_warn
                    .map(|t| t.elapsed() > Duration::from_secs(30))
                    .unwrap_or(true);
                if warn {
                    state.log(LogLevel::Warn, format!("Server sync skipped: {e} (client stays connected to IB)"));
                    last_unreachable_warn = Some(Instant::now());
                }
                state.with_status(|s| s.master_connected = false);
                sleep_running(cfg.sync_interval_secs, state);
                continue;
            }
        };

        // Capture the server's starting structure once per session for New-Only mode.
        if baseline_symbols.is_none() {
            baseline_symbols = Some(snap.positions.iter().map(|p| p.symbol.clone()).collect());
        }

        let controls = state.controls.lock().clone();

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
        let current_working = ib::read_open_orders(&client, &account).unwrap_or_default();
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
            // Margin: gate entry (opening) orders against live buying power via what-if.
            // Protective orders (stops/limits that reduce risk) still go through.
            if w.is_entry
                && !open_allowed(
                    &client, state, &account, &w.symbol, &w.currency, &w.exchange, w.side, w.quantity, w.kind,
                    w.limit_price, w.aux_price, sma_call, &mut projected_available,
                )
            {
                continue;
            }
            if let Some(t) = wo_cooldown.get(&w.key()) {
                if t.elapsed() < cooldown_dur {
                    continue;
                }
            }
            match ib::place_order(
                &client, &account, &w.symbol, &w.currency, &w.exchange, w.side, w.quantity, w.kind, w.limit_price, w.aux_price,
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
                // Margin: gate opening/increasing via what-if buying-power check; always
                // allow reduces/closes/flattens (they free margin).
                if is_opening(intent.reason)
                    && !open_allowed(
                        &client, state, &account, &intent.symbol, &intent.currency, &intent.exchange,
                        intent.side, intent.qty, intent.kind, intent.limit_price, intent.aux_price,
                        sma_call, &mut projected_available,
                    )
                {
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
                    &account,
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

fn do_close_all(client: &Client, state: &Arc<SharedState>, account: &str) {
    state.log(LogLevel::Warn, "CLOSE ALL — cancelling working orders and flattening positions…");
    // Cancel only THIS account's working orders (never a blanket global cancel that could
    // touch other accounts on the login).
    for (id, w) in ib::read_open_orders(client, account).unwrap_or_default() {
        if let Err(e) = ib::cancel_order(client, id) {
            state.log(LogLevel::Warn, format!("Cancel warning {}: {e}", w.symbol));
        }
    }
    let positions = ib::read_positions(client, account).unwrap_or_default();
    if positions.is_empty() {
        state.log(LogLevel::Ok, "Close all — no open positions.");
        return;
    }
    for p in &positions {
        let side = Side::closing(p.net_qty);
        let qty = p.net_qty.abs();
        match ib::place_order(
            client,
            account,
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

/// IBKR system notices that are routine status/connectivity chatter (and cancel
/// confirmations), suppressed from the order feed to avoid noise.
fn is_notice_noise(code: i32) -> bool {
    matches!(code, 202 | 2100 | 2103 | 2104 | 2105 | 2106 | 2107 | 2108 | 2119 | 2150 | 2158)
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

/// Decide whether an opening order may be placed, using an exact IBKR what-if margin
/// check. `projected_available` is this cycle's running buying power; on approval it is
/// decremented by the order's initial-margin requirement so cumulative opens stay within
/// buying power. SMA calls block all opens. If the what-if can't be evaluated, we defer
/// to IBKR's own real-time rejection rather than falsely halting.
#[allow(clippy::too_many_arguments)]
fn open_allowed(
    client: &Client,
    state: &Arc<SharedState>,
    account: &str,
    symbol: &str,
    currency: &str,
    exchange: &str,
    side: Side,
    qty: f64,
    kind: pax_core::OrderKind,
    limit_price: f64,
    aux_price: f64,
    sma_call: bool,
    projected_available: &mut f64,
) -> bool {
    if sma_call {
        return false;
    }
    match ib::what_if_init_margin(client, account, symbol, currency, exchange, side, qty, kind, limit_price, aux_price) {
        Ok(init_margin) => {
            if *projected_available - init_margin < 0.0 {
                state.log(
                    LogLevel::Warn,
                    format!(
                        "Margin: skip {} {} qty={:.0} — needs ${:.0} init margin, ${:.0} buying power left",
                        side.as_ib(),
                        symbol,
                        qty,
                        init_margin,
                        projected_available.max(0.0)
                    ),
                );
                false
            } else {
                *projected_available -= init_margin;
                true
            }
        }
        Err(_) => true,
    }
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
