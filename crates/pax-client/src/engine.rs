//! The client trading engine.
//!
//! Runs on its own thread for the whole app lifetime, doing work only while the operator
//! has pressed START. Each pass fetches the master snapshot, reads the client's own net
//! positions, reconciles them into delta orders via [`pax_core::reconcile`], and submits
//! those orders — with a per-symbol cooldown so in-flight fills are never double-ordered.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ibapi::client::blocking::Client;
use ibapi::orders::OrderUpdate;
use pax_core::{
    reconcile, target_net_qty, IntentReason, ReconcileInput, Side, SizingParams, WorkingOrder,
};

use crate::config::{stable_client_id, ClientConfig};
use crate::ib;
use crate::license::{self, LicenseStatus};
use crate::master_api::MasterApi;
use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

/// After IBKR rejects an order, pause re-submitting that contract+side for this long so a
/// rejected order isn't hammered every cycle (the cause of 201 order floods).
const REJECT_BACKOFF_SECS: u64 = 60;
/// Throttle repeated identical IBKR notice codes to at most once per this interval.
const NOTICE_THROTTLE_SECS: u64 = 30;

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
    state.log(LogLevel::Info, "Connecting…".to_string());
    let port = match account_mode {
        AccountMode::Live => port_live,
        AccountMode::Paper => port_paper,
    };
    let cid = stable_client_id();
    let endpoint = format!("{}:{}", host, port);
    state.log(LogLevel::Info, format!("Connecting to IB {endpoint} clientId={cid}…"));

    // Auto-reconnect: keep retrying the IB connection while the operator has the engine
    // running, rather than stopping on the first failure. A hands-off client should resume
    // on its own when the Gateway/TWS comes back — and staying "running but disconnected"
    // is what the disconnect-alert monitor watches for.
    let client = loop {
        if !state.is_running() {
            return;
        }
        match Client::connect(&endpoint, cid) {
            Ok(c) => break c,
            Err(e) => {
                state.with_status(|s| s.connected = false);
                state.log(
                    LogLevel::Warn,
                    format!("IB connection failed: {e}. Retrying in 15s (check Gateway/TWS API)."),
                );
                // Interruptible wait so STOP stays responsive.
                for _ in 0..150 {
                    if !state.is_running() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
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
    // Sync our order-id sequence with the server's next valid id. The ibapi connect seed
    // can lag the gateway's real sequence, causing submissions with ids BELOW it ("OrderId
    // N is < M") which IBKR rejects outright — so nothing fills and the engine retries
    // forever. An explicit reqIds round-trip pins the sequence to the server's value.
    match client.next_valid_order_id() {
        Ok(id) => state.log(LogLevel::Ok, format!("Order id sequence synced (next={id})")),
        Err(e) => state.log(
            LogLevel::Warn,
            format!("Could not sync order id sequence: {e} — proceeding with local sequence"),
        ),
    }

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
    // Order id → (symbol, side) for orders we placed, so async status updates (fills /
    // rejections) can be attributed back to a contract and side.
    let mut placed_orders: HashMap<i32, (String, Side)> = HashMap::new();
    // (symbol, side) → time of last rejection; re-submits for that side pause until the
    // backoff window elapses.
    let mut reject_backoff: HashMap<(String, Side), Instant> = HashMap::new();
    // Order ids already logged inactive/rejected — dedupe repeated status callbacks.
    let mut logged_inactive: HashSet<i32> = HashSet::new();
    // IBKR notice code → last log time, to throttle repeated identical notices.
    let mut notice_log: HashMap<i32, Instant> = HashMap::new();
    let mut drawdown_hit = false;
    // M1: running peak equity (high-water mark) for a true max-drawdown control.
    let mut peak_balance = baseline;
    let mut last_unreachable_warn: Option<Instant> = None;
    let mut last_margin_warn: Option<Instant> = None;
    let mut last_offhours_log: Option<Instant> = None;
    let cooldown_dur = Duration::from_secs(cfg.order_cooldown_secs);
    // Symbols the master already held at session start — used by "New Only" mode to
    // suppress opening pre-existing master positions. Captured on first snapshot.
    let mut baseline_symbols: Option<HashSet<String>> = None;
    // ── Master-change gate state ─────────────────────────────────────────────
    // The client re-syncs its structure ONLY when the master's ledger changes. Between
    // changes these locked targets/orders are held so balance/price drift never triggers
    // a resize (which would rack up commissions). Recomputed proportionally to the CURRENT
    // balances at the instant the master adjusts.
    // Resume from the persisted ledger (if it belongs to this account) so a restart does
    // NOT recompute/resize a book that already matches — only a real master change since
    // the ledger was written will trigger a resize.
    //
    // Gating is PER SYMBOL: `seen_master_net` is the master net qty we last locked a target
    // against, per symbol. A target is only recomputed (with the then-current balances) when
    // THAT symbol's master net changes — so a move in one name, or a re-priced master order,
    // never re-sizes a symbol the master left untouched. `last_wo_fp` gates the resting-order
    // replication independently.
    // `last_wo_fp` / `locked_desired` are retained only to round-trip older ledgers; this
    // positions-only build no longer replicates resting orders, so they are never updated.
    let (mut seen_master_net, mut locked_targets, last_wo_fp, locked_desired) =
        match crate::ledger::load(&account) {
            Some(l) => {
                state.log(
                    LogLevel::Ok,
                    format!("Resumed saved positions ({} symbols) — holding unless the strategy changes.", l.targets.len()),
                );
                (l.seen_master_net, l.targets, l.wo_fingerprint, l.desired)
            }
            None => (
                BTreeMap::<String, f64>::new(),
                BTreeMap::<String, f64>::new(),
                None::<String>,
                Vec::<WorkingOrder>::new(),
            ),
        };
    let mut last_ledger_save = Instant::now();

    // Positions-only mode: this build never places resting limit/stop orders. Cancel any
    // leftover resting orders (e.g. from an older order-replication build) so the account
    // holds positions only — nothing rests to churn or fill unexpectedly. One-time on
    // connect; the engine places only market orders afterward, so none reaccumulate.
    {
        let stale = ib::read_open_orders(&client, &account).unwrap_or_default();
        if !stale.is_empty() {
            state.log(LogLevel::Warn, format!("Clearing {} leftover resting order(s) (positions-only).", stale.len()));
            for (id, _w) in &stale {
                let _ = ib::cancel_order(&client, *id);
            }
        }
    }

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
                        // Include order id + cumulative qty so partial fills of ONE order
                        // (same id, rising cum) are distinguishable from repeated orders
                        // (different ids) at a glance.
                        state.log(
                            lvl,
                            format!(
                                "FILL {} {} {:.0} @ {:.2} (ord {}, cum {:.0})",
                                d.execution.side,
                                d.contract.symbol,
                                d.execution.shares,
                                d.execution.price,
                                d.execution.order_id,
                                d.execution.cumulative_quantity
                            ),
                        );
                    }
                    OrderUpdate::OrderStatus(s) => {
                        if s.status == "Inactive" {
                            // Back off this contract/side so a rejected order isn't
                            // resubmitted every cycle (root cause of the 201 floods).
                            if let Some((sym, side)) = placed_orders.get(&s.order_id) {
                                reject_backoff.insert((sym.clone(), *side), Instant::now());
                            }
                            // Log/count each rejection only once (IBKR re-sends the status).
                            if logged_inactive.insert(s.order_id) {
                                state.log(
                                    LogLevel::Err,
                                    format!("Order {} rejected/inactive (filled {:.0}, rem {:.0})", s.order_id, s.filled, s.remaining),
                                );
                                state.with_status(|st| st.orders_failed += 1);
                            }
                        }
                    }
                    OrderUpdate::Message(n) => {
                        if !is_notice_noise(n.code) {
                            let show = notice_log
                                .get(&n.code)
                                .map(|t| t.elapsed() > Duration::from_secs(NOTICE_THROTTLE_SECS))
                                .unwrap_or(true);
                            if show {
                                notice_log.insert(n.code, Instant::now());
                                state.log(LogLevel::Warn, format!("IBKR [{}] {}", n.code, n.message));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // ── one-shot Close All request ───────────────────────────────────────
        // Flatten the book, then HALT trading (armed) so the engine does not immediately
        // re-open to match the master. The operator presses START to clear the halt.
        if state.close_all.swap(false, Ordering::Relaxed) {
            do_close_all(&client, state, &account);
            state.halted.store(true, Ordering::Relaxed);
            state.with_status(|s| s.halted = true);
            state.log(LogLevel::Warn, "CLOSE ALL complete — trading HALTED. Press START to resume.");
        }

        // ── balance + margin + drawdown guards ───────────────────────────────
        // A failed margin/balance read must NOT proceed with zeroed values (that would
        // read as "balances unknown" and block with a misleading reason). Skip the cycle
        // with a clear, throttled message and retry.
        let margin = match ib::read_margin(&client, &account) {
            Ok(m) => m,
            Err(e) => {
                let warn = last_margin_warn.map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(true);
                if warn {
                    state.log(LogLevel::Warn, format!("Margin/balance read failed: {e} — skipping cycle, will retry."));
                    last_margin_warn = Some(Instant::now());
                }
                sleep_running(cfg.sync_interval_secs, state);
                continue;
            }
        };
        let client_balance = margin.net_liq;
        let max_dd = state.controls.lock().max_drawdown_pct;
        // Universal "no capacity to open" signal that works across Reg-T, Portfolio
        // Margin, cash, AND paper accounts: IBKR-computed AvailableFunds (= equity −
        // initial margin). Each opening order is additionally gated by an exact per-order
        // what-if margin check below. SMA is a Reg-T-specific value that is unreliable on
        // Portfolio Margin / paper accounts, so it is surfaced for information only and is
        // never used to gate trading.
        let mut projected_available = margin.available_funds;
        let opens_blocked = margin.available_funds <= 0.0;
        if opens_blocked {
            let warn = last_margin_warn.map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(true);
            if warn {
                state.log(
                    LogLevel::Warn,
                    format!(
                        "Insufficient margin (available funds ${:.0}) — opens blocked, closes allowed",
                        margin.available_funds
                    ),
                );
                last_margin_warn = Some(Instant::now());
            }
        }
        // Max drawdown is measured from the running PEAK equity (high-water mark), not the
        // session-start balance, so a profitable run doesn't loosen the guard.
        if client_balance > 0.0 {
            peak_balance = peak_balance.max(client_balance);
            let dd = if peak_balance > 0.0 { (peak_balance - client_balance) / peak_balance * 100.0 } else { 0.0 };
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
            s.margin_blocks_opens = opens_blocked;
            s.halted = state.halted.load(Ordering::Relaxed);
        });

        if drawdown_hit {
            sleep_running(cfg.sync_interval_secs, state);
            continue;
        }

        // CLOSE ALL halt: stay connected and keep status/positions live, but place no
        // orders until the operator presses START (which clears the halt).
        if state.halted.load(Ordering::Relaxed) {
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
                    state.log(LogLevel::Warn, format!("Sync skipped: {e} (still connected to IB)"));
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

        // Off-hours guard: when RTH-only is enabled, place no orders outside US equity
        // regular trading hours. We keep status/positions/master updated above, but skip
        // the trading channels AND the master-change gate, so a master move during the
        // close is synced when the session reopens (not lost, not acted on off-hours).
        if controls.rth_only && !crate::market_hours::is_us_equity_rth_now() {
            let warn = last_offhours_log
                .map(|t| t.elapsed() > Duration::from_secs(300))
                .unwrap_or(true);
            if warn {
                state.log(LogLevel::Info, "Outside regular trading hours — holding (RTH-only).".to_string());
                last_offhours_log = Some(Instant::now());
            }
            sleep_running(cfg.sync_interval_secs, state);
            continue;
        }

        let sizing = SizingParams {
            multiplier: controls.multiplier,
            master_balance: snap.balance,
            client_balance,
            force_min_one: true,
        };
        let long_only = controls.trade_mode == TradeMode::LongOnly;

        // ── Source-change gate ────────────────────────────────────────────────
        // Recompute a symbol's target net position ONLY when the source's net position in
        // that symbol actually changes. Targets are sized proportionally to the CURRENT
        // balances at that instant, then held — so a matched book is never resized by mere
        // balance/price drift (the source of commission bleed). This build copies POSITIONS
        // only; the source's resting limit/stop orders are intentionally ignored.
        if sizing.ratio().is_some() {
            let mut changed = false;

            // Position targets: recompute PER SYMBOL, only where the source net changed.
            let mut now_net: BTreeMap<String, (f64, f64)> = BTreeMap::new(); // sym -> (net, price)
            for p in &snap.positions {
                now_net.insert(p.symbol.clone(), (p.net_qty, p.avg_cost));
            }
            // Walk the union of previously-tracked and currently-held master symbols.
            let mut syms: BTreeSet<String> = seen_master_net.keys().cloned().collect();
            syms.extend(now_net.keys().cloned());
            for sym in syms {
                let now = now_net.get(&sym).map(|(n, _)| *n).unwrap_or(0.0);
                let symbol_changed = match seen_master_net.get(&sym).copied() {
                    Some(prev) => (prev - now).abs() >= 0.5,
                    None => true,
                };
                if !symbol_changed {
                    continue;
                }
                let price = now_net.get(&sym).map(|(_, p)| *p).unwrap_or(0.0);
                let mut t = if now.abs() < 0.5 {
                    0.0 // master flat in this symbol → target flat (close orphan)
                } else {
                    target_net_qty(now, price, &sizing).unwrap_or(0.0)
                };
                if long_only && t < 0.0 {
                    t = 0.0;
                }
                locked_targets.insert(sym.clone(), t);
                seen_master_net.insert(sym.clone(), now);
                changed = true;
            }

            if changed {
                state.log(LogLevel::Info, "Strategy updated — re-synced affected symbols.".to_string());
                // Persist immediately so a restart right after a change resumes the new state.
                crate::ledger::save(&account, &seen_master_net, &locked_targets, &last_wo_fp, &locked_desired);
                last_ledger_save = Instant::now();
            }
        }

        // Read our own in-flight orders so we never stack a second market order on a
        // contract/side that already has one working (IBKR error 201). This build places
        // ONLY market orders, so any resting orders found are cleaned up below.
        let live_orders = ib::read_live_orders(&client, &account).unwrap_or_default();
        let active_market_sides: HashSet<(String, Side)> = live_orders
            .iter()
            .filter(|o| matches!(o.kind, pax_core::OrderKind::Market))
            .map(|o| (o.symbol.clone(), o.side))
            .collect();
        // Self-heal: cancel any resting (non-market) orders that appear — this build never
        // wants them, so the account stays positions-only no matter what.
        for o in live_orders.iter().filter(|o| !matches!(o.kind, pax_core::OrderKind::Market)) {
            if ib::cancel_order(&client, o.id).is_ok() {
                state.log(LogLevel::Warn, format!("Cancel stray resting order {} {}", o.side.as_ib(), o.symbol));
            }
        }

        // ── Position safety net: match the source's NET positions via market orders ─────
        // Copy actual positions only — the source's pending resting orders are ignored, so
        // the client acts when a position genuinely changes (the source's order fills).
        let input = ReconcileInput {
            master: &snap.positions,
            client: &client_positions,
            master_connected: snap.connected,
            sizing,
            long_only,
            split_zero_cross: true,
            empty_master_guard: 2,
            // Locked, balance-stable targets: resize only when the source's net changes.
            targets: Some(&locked_targets),
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
                // Never stack a second market order on a contract/side that already has one
                // of ours in flight — this is exactly what trips IBKR error 201.
                if active_market_sides.contains(&(intent.symbol.clone(), intent.side)) {
                    continue;
                }
                // Pause a contract/side IBKR recently rejected (cool-off, not every cycle).
                if in_backoff(&reject_backoff, &intent.symbol, intent.side) {
                    continue;
                }
                // Margin: gate opening/increasing via what-if buying-power check; always
                // allow reduces/closes/flattens (they free margin).
                if is_opening(intent.reason)
                    && !open_allowed(
                        &client, state, &account, &intent.symbol, &intent.currency, &intent.exchange,
                        intent.side, intent.qty, intent.kind, intent.limit_price, intent.aux_price,
                        &mut projected_available,
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
                    Ok(id) => {
                        placed_orders.insert(id, (intent.symbol.clone(), intent.side));
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

        // Persist the ledger on a 15s cadence so the matched state survives a crash/restart.
        if last_ledger_save.elapsed() >= Duration::from_secs(15) {
            crate::ledger::save(&account, &seen_master_net, &locked_targets, &last_wo_fp, &locked_desired);
            last_ledger_save = Instant::now();
        }

        sleep_running(cfg.sync_interval_secs, state);
    }

    // Save once more on a clean stop so the latest matched state is on disk.
    crate::ledger::save(&account, &seen_master_net, &locked_targets, &last_wo_fp, &locked_desired);
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
    // 2161 = limit-order price-capped per IBKR's disruptive-order control (informational;
    // the order still rests and works). Routine noise on the working-order channel.
    matches!(code, 202 | 2100 | 2103 | 2104 | 2105 | 2106 | 2107 | 2108 | 2119 | 2150 | 2158 | 2161)
}

/// True while `(symbol, side)` is within its post-rejection cool-off window.
fn in_backoff(backoff: &HashMap<(String, Side), Instant>, symbol: &str, side: Side) -> bool {
    backoff
        .get(&(symbol.to_string(), side))
        .map(|t| t.elapsed() < Duration::from_secs(REJECT_BACKOFF_SECS))
        .unwrap_or(false)
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
/// check. `projected_available` is this cycle's running buying power (seeded from IBKR's
/// AvailableFunds); on approval it is decremented by the order's initial-margin
/// requirement so cumulative opens stay within buying power. This is account-type
/// agnostic (Reg-T, Portfolio Margin, cash). If the what-if can't be evaluated, we defer
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
    projected_available: &mut f64,
) -> bool {
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
