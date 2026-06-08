//! The reconciliation engine — the safety core of Pax Americana.
//!
//! Given the master's net positions and the client's net positions, it produces a set
//! of [`TradeIntent`]s that move the client toward a proportionally-scaled copy of the
//! master's *structure*. It never emits an order derived from a raw action verb, so it
//! cannot accidentally flip a flat/long book into a short when the master simply closes.
//!
//! Key guarantees:
//! * If the master is flat in a symbol, the client target is `0` → the client only ever
//!   trades *toward flat* for that symbol and stops there.
//! * Long-only mode clamps every target to `>= 0`, so no short can ever be opened.
//! * Zero-crossing reconciliations (e.g. client long, master genuinely short) are split
//!   into an explicit flatten leg followed by an open leg, so a single fill can never be
//!   misread as "go short by selling".
//! * Safety guards refuse to act when the master is disconnected or reports an
//!   implausibly empty book while the client holds many positions.

use std::collections::BTreeMap;

use crate::model::{OrderKind, Position, Side};
use crate::sizing::{target_net_qty, SizingParams};

/// Why an intent was generated — surfaced in the UI log for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentReason {
    /// Master holds it, client is flat — open toward the target.
    OpenMissing,
    /// Increase an existing same-direction position toward the target.
    IncreaseToTarget,
    /// Reduce an existing position toward a smaller (same-sign) target.
    ReduceToTarget,
    /// Client holds it, master is flat — close the orphan to flat.
    CloseOrphan,
    /// First leg of a direction flip: close the current position to flat.
    FlattenLeg,
    /// Second leg of a direction flip: open the new-direction position.
    OpenLeg,
}

impl IntentReason {
    pub fn label(self) -> &'static str {
        match self {
            IntentReason::OpenMissing => "open missing",
            IntentReason::IncreaseToTarget => "increase",
            IntentReason::ReduceToTarget => "reduce",
            IntentReason::CloseOrphan => "close",
            IntentReason::FlattenLeg => "flatten",
            IntentReason::OpenLeg => "open",
        }
    }
}

/// A concrete, side-and-quantity order the client should place. `qty` is always
/// strictly positive; direction lives in `side`.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeIntent {
    pub symbol: String,
    pub currency: String,
    pub exchange: String,
    pub side: Side,
    pub qty: f64,
    pub kind: OrderKind,
    pub limit_price: f64,
    pub aux_price: f64,
    pub reason: IntentReason,
}

/// A symbol the engine deliberately did not act on, with a human reason.
#[derive(Debug, Clone, PartialEq)]
pub struct SkipNote {
    pub symbol: String,
    pub reason: String,
}

/// Result of a reconciliation pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReconcileResult {
    pub intents: Vec<TradeIntent>,
    pub skipped: Vec<SkipNote>,
    /// When true, the engine refused to act at all (a global safety guard tripped).
    pub blocked: bool,
    pub blocked_reason: Option<String>,
}

/// Everything the engine needs for one pass.
pub struct ReconcileInput<'a> {
    pub master: &'a [Position],
    pub client: &'a [Position],
    pub master_connected: bool,
    pub sizing: SizingParams,
    pub long_only: bool,
    /// Split direction-flipping reconciliations into flatten + open legs.
    pub split_zero_cross: bool,
    /// If the master reports an empty book while the client holds more than this many
    /// positions, treat it as a likely connectivity glitch and refuse to act.
    pub empty_master_guard: usize,
    /// Optional per-symbol signed target overrides. When `Some`, the engine uses these
    /// fixed targets instead of recomputing them from `sizing` on every pass. This lets
    /// the caller LOCK targets (computed proportionally at the moment the master's ledger
    /// changes) so a matched book isn't resized on every balance/price tick — only when
    /// the master itself adjusts. A symbol absent from the map targets `0` (close).
    pub targets: Option<&'a BTreeMap<String, f64>>,
}

impl<'a> ReconcileInput<'a> {
    pub fn new(master: &'a [Position], client: &'a [Position]) -> Self {
        ReconcileInput {
            master,
            client,
            master_connected: true,
            sizing: SizingParams::default(),
            long_only: false,
            split_zero_cross: true,
            empty_master_guard: 2,
            targets: None,
        }
    }
}

const EPS: f64 = 1e-9;

/// Run one reconciliation pass.
pub fn reconcile(input: &ReconcileInput) -> ReconcileResult {
    let mut result = ReconcileResult::default();

    // ── Global safety guards ────────────────────────────────────────────────
    if !input.master_connected {
        result.blocked = true;
        result.blocked_reason = Some("standing by — sync paused".to_string());
        return result;
    }
    if input.sizing.ratio().is_none() {
        result.blocked = true;
        result.blocked_reason =
            Some("balances not yet known — sync skipped (sizing undefined)".to_string());
        return result;
    }
    if input.master.is_empty() && input.client.len() > input.empty_master_guard {
        result.blocked = true;
        result.blocked_reason = Some(format!(
            "0 target positions but local book holds {} — holding (safety)",
            input.client.len()
        ));
        return result;
    }

    // ── Build lookup maps over the union of symbols ──────────────────────────
    let mut master_map: BTreeMap<&str, &Position> = BTreeMap::new();
    for p in input.master {
        master_map.insert(p.symbol.as_str(), p);
    }
    let mut client_map: BTreeMap<&str, &Position> = BTreeMap::new();
    for p in input.client {
        client_map.insert(p.symbol.as_str(), p);
    }

    let mut symbols: Vec<&str> = master_map.keys().copied().collect();
    for s in client_map.keys() {
        if !master_map.contains_key(s) {
            symbols.push(s);
        }
    }
    symbols.sort_unstable();

    for sym in symbols {
        let master_pos = master_map.get(sym).copied();
        let client_pos = client_map.get(sym).copied();

        let master_net = master_pos.map(|p| p.net_qty).unwrap_or(0.0);
        let current = client_pos.map(|p| p.net_qty).unwrap_or(0.0);

        // Reference price for the notional clamp: prefer master avg cost.
        let price = master_pos.map(|p| p.avg_cost).unwrap_or(0.0);

        // Target net quantity (signed). Prefer a locked override when supplied (so we only
        // resize when the master's ledger changes); otherwise compute it proportionally.
        let mut target = match input.targets {
            Some(map) => map.get(sym).copied().unwrap_or(0.0),
            None => match target_net_qty(master_net, price, &input.sizing) {
                Some(t) => t,
                None => continue, // balances vanished mid-pass; skip safely
            },
        };

        // Long-only clamps every target to non-negative: a short can never be opened,
        // and an existing long is closed (not flipped) when the master goes short.
        if input.long_only && target < 0.0 {
            target = 0.0;
        }

        let delta = target - current;
        if delta.abs() < 0.5 {
            // Within rounding tolerance of the target — nothing to do.
            continue;
        }

        // Carry the master's order-type hint for opening/increasing legs.
        let (kind, limit_price, aux_price) = match master_pos {
            Some(p) => (p.order_kind, p.limit_price, p.aux_price),
            None => (OrderKind::Market, 0.0, 0.0),
        };
        let currency = master_pos
            .or(client_pos)
            .map(|p| p.currency.clone())
            .unwrap_or_else(|| "USD".to_string());
        let exchange = master_pos
            .or(client_pos)
            .map(|p| p.exchange.clone())
            .unwrap_or_else(|| "SMART".to_string());

        let crosses_zero = current > EPS && target < -EPS || current < -EPS && target > EPS;

        if crosses_zero && input.split_zero_cross {
            // Leg 1: flatten the current book to zero with a guaranteed market order.
            result.intents.push(TradeIntent {
                symbol: sym.to_string(),
                currency: currency.clone(),
                exchange: exchange.clone(),
                side: Side::closing(current),
                qty: current.abs(),
                kind: OrderKind::Market,
                limit_price: 0.0,
                aux_price: 0.0,
                reason: IntentReason::FlattenLeg,
            });
            // Leg 2: open the new-direction target.
            result.intents.push(TradeIntent {
                symbol: sym.to_string(),
                currency,
                exchange,
                side: Side::from_delta(target),
                qty: target.abs(),
                kind,
                limit_price,
                aux_price,
                reason: IntentReason::OpenLeg,
            });
            continue;
        }

        // Single-leg move toward the target (no zero crossing, or splitting disabled).
        let side = Side::from_delta(delta);
        let reason = classify(current, target);
        // Reducing/closing always uses a market order to guarantee the exit fills;
        // opening/increasing carries the master's order-type hint.
        let (eff_kind, eff_lmt, eff_aux) = match reason {
            IntentReason::ReduceToTarget | IntentReason::CloseOrphan => {
                (OrderKind::Market, 0.0, 0.0)
            }
            _ => (kind, limit_price, aux_price),
        };

        result.intents.push(TradeIntent {
            symbol: sym.to_string(),
            currency,
            exchange,
            side,
            qty: delta.abs(),
            kind: eff_kind,
            limit_price: eff_lmt,
            aux_price: eff_aux,
            reason,
        });
    }

    result
}

fn classify(current: f64, target: f64) -> IntentReason {
    if current.abs() < EPS {
        IntentReason::OpenMissing
    } else if target.abs() < EPS {
        IntentReason::CloseOrphan
    } else if target.abs() > current.abs() {
        IntentReason::IncreaseToTarget
    } else {
        IntentReason::ReduceToTarget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equal_balances() -> SizingParams {
        SizingParams {
            master_balance: 100_000.0,
            client_balance: 100_000.0,
            ..Default::default()
        }
    }

    fn pos(sym: &str, qty: f64) -> Position {
        Position::new(sym, qty)
    }

    /// THE headline guarantee: the master closes a long by selling, going flat.
    /// The client (also flat after copying) must do NOTHING — never open a short.
    #[test]
    fn master_closes_long_client_does_not_short() {
        let master: Vec<Position> = vec![]; // master flat after closing
        let client: Vec<Position> = vec![]; // client already flat
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        let r = reconcile(&input);
        assert!(r.intents.is_empty(), "must not trade when both are flat");
    }

    /// Master closes a long; client still holds the (now orphaned) long.
    /// Client must SELL exactly its holding to reach flat — and not a share more.
    #[test]
    fn orphan_long_is_closed_to_flat_not_shorted() {
        let master: Vec<Position> = vec![];
        let client = vec![pos("TSLA", 400.0)];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        let r = reconcile(&input);
        assert_eq!(r.intents.len(), 1);
        let i = &r.intents[0];
        assert_eq!(i.side, Side::Sell);
        assert_eq!(i.qty, 400.0, "sells exactly the holding — no overshoot to short");
        assert_eq!(i.reason, IntentReason::CloseOrphan);
    }

    /// Master holds a long, client is flat -> open it, scaled.
    #[test]
    fn missing_long_is_opened_scaled() {
        let master = vec![pos("AAPL", 100.0)];
        let client: Vec<Position> = vec![];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = SizingParams {
            master_balance: 25_000.0,
            client_balance: 100_000.0,
            ..Default::default()
        };
        let r = reconcile(&input);
        assert_eq!(r.intents.len(), 1);
        assert_eq!(r.intents[0].side, Side::Buy);
        assert_eq!(r.intents[0].qty, 400.0);
        assert_eq!(r.intents[0].reason, IntentReason::OpenMissing);
    }

    /// Genuine direction flip (master is really net short): split into flatten + open.
    #[test]
    fn genuine_flip_is_split_into_two_legs() {
        let master = vec![pos("NVDA", -50.0)]; // master genuinely short
        let client = vec![pos("NVDA", 50.0)]; // client currently long
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        let r = reconcile(&input);
        assert_eq!(r.intents.len(), 2);
        assert_eq!(r.intents[0].reason, IntentReason::FlattenLeg);
        assert_eq!(r.intents[0].side, Side::Sell);
        assert_eq!(r.intents[0].qty, 50.0);
        assert_eq!(r.intents[1].reason, IntentReason::OpenLeg);
        assert_eq!(r.intents[1].side, Side::Sell);
        assert_eq!(r.intents[1].qty, 50.0);
    }

    /// Long-only must never produce a short, even when the master is short.
    #[test]
    fn long_only_never_shorts() {
        let master = vec![pos("SPY", -100.0)];
        let client = vec![pos("SPY", 100.0)];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        input.long_only = true;
        let r = reconcile(&input);
        // Target clamps to 0 -> just close the long to flat, one leg.
        assert_eq!(r.intents.len(), 1);
        assert_eq!(r.intents[0].side, Side::Sell);
        assert_eq!(r.intents[0].qty, 100.0);
        assert_eq!(r.intents[0].reason, IntentReason::CloseOrphan);
    }

    #[test]
    fn increase_existing_long() {
        let master = vec![pos("MSFT", 200.0)];
        let client = vec![pos("MSFT", 50.0)];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        let r = reconcile(&input);
        assert_eq!(r.intents.len(), 1);
        assert_eq!(r.intents[0].side, Side::Buy);
        assert_eq!(r.intents[0].qty, 150.0);
        assert_eq!(r.intents[0].reason, IntentReason::IncreaseToTarget);
    }

    #[test]
    fn reduce_existing_long_uses_market() {
        let master = vec![pos("QQQ", 30.0)];
        let client = vec![pos("QQQ", 100.0)];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        let r = reconcile(&input);
        assert_eq!(r.intents.len(), 1);
        assert_eq!(r.intents[0].side, Side::Sell);
        assert_eq!(r.intents[0].qty, 70.0);
        assert_eq!(r.intents[0].kind, OrderKind::Market);
        assert_eq!(r.intents[0].reason, IntentReason::ReduceToTarget);
    }

    #[test]
    fn blocks_when_master_disconnected() {
        let master: Vec<Position> = vec![];
        let client = vec![pos("X", 10.0)];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        input.master_connected = false;
        let r = reconcile(&input);
        assert!(r.blocked);
        assert!(r.intents.is_empty());
    }

    #[test]
    fn blocks_on_empty_master_with_many_client_positions() {
        let master: Vec<Position> = vec![];
        let client = vec![pos("A", 1.0), pos("B", 2.0), pos("C", 3.0)];
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        let r = reconcile(&input);
        assert!(r.blocked, "empty master + many client positions must block");
    }

    /// With a LOCKED target equal to the current holding, the engine does nothing — even
    /// though live sizing would say 100. This is the anti-churn guarantee: a matched book
    /// is not resized by balance drift, only by a change to the (locked) target.
    #[test]
    fn locked_target_suppresses_balance_drift_resize() {
        let master = vec![pos("AAPL", 100.0)];
        let client = vec![pos("AAPL", 50.0)];
        let mut targets = BTreeMap::new();
        targets.insert("AAPL".to_string(), 50.0);
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances(); // would otherwise target 100 -> +50
        input.targets = Some(&targets);
        let r = reconcile(&input);
        assert!(r.intents.is_empty(), "locked target == holding -> no resize");
    }

    /// When the locked target changes (because the master adjusted), the client resizes
    /// toward exactly the new target.
    #[test]
    fn locked_target_change_drives_resize() {
        let master = vec![pos("AAPL", 100.0)];
        let client = vec![pos("AAPL", 50.0)];
        let mut targets = BTreeMap::new();
        targets.insert("AAPL".to_string(), 120.0);
        let mut input = ReconcileInput::new(&master, &client);
        input.sizing = equal_balances();
        input.targets = Some(&targets);
        let r = reconcile(&input);
        assert_eq!(r.intents.len(), 1);
        assert_eq!(r.intents[0].side, Side::Buy);
        assert_eq!(r.intents[0].qty, 70.0);
        assert_eq!(r.intents[0].reason, IntentReason::IncreaseToTarget);
    }

    #[test]
    fn blocks_when_balances_unknown() {
        let master = vec![pos("A", 10.0)];
        let client: Vec<Position> = vec![];
        let input = ReconcileInput::new(&master, &client); // default sizing has 0 balances
        let r = reconcile(&input);
        assert!(r.blocked);
    }
}
