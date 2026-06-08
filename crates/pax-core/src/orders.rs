//! Working-order replication: scale the source's resting limit/stop orders to the client,
//! diff them against the client's existing resting orders, and fold *entry* orders into the
//! effective position used by the reconciliation safety net.

use std::collections::BTreeMap;

use crate::model::{OrderKind, Position, Side, WorkingOrder};
use crate::sizing::{round_qty, SizingParams};

/// Scale the master's working orders into the client's desired working orders.
///
/// Quantities are scaled by `client_balance / master_balance × multiplier` (rounded,
/// min one share when the master has a non-zero order). In long-only mode, entry sells
/// (which would open a short) are dropped, and protective sells are capped to the
/// client's current long in that symbol so they only ever close.
pub fn desired_working_orders(
    master: &[WorkingOrder],
    sizing: &SizingParams,
    long_only: bool,
    client_positions: &[Position],
) -> Vec<WorkingOrder> {
    let ratio = match sizing.ratio() {
        Some(r) => r * sizing.multiplier,
        None => return Vec::new(),
    };

    let mut client_long: BTreeMap<&str, f64> = BTreeMap::new();
    for p in client_positions {
        client_long.insert(p.symbol.as_str(), p.net_qty.max(0.0));
    }

    let mut out = Vec::new();
    for mo in master {
        let mut qty = round_qty(mo.quantity * ratio).abs();
        if qty < 1.0 && mo.quantity != 0.0 && sizing.force_min_one {
            qty = 1.0;
        }
        if qty <= 0.0 {
            continue;
        }

        if long_only && mo.side == Side::Sell {
            if mo.is_entry {
                continue; // an entry sell would open a short — forbidden in long-only
            }
            let long = client_long.get(mo.symbol.as_str()).copied().unwrap_or(0.0);
            if long <= 0.0 {
                continue;
            }
            qty = qty.min(long); // protective sell: never exceed the long it protects
            if qty <= 0.0 {
                continue;
            }
        }

        out.push(WorkingOrder {
            symbol: mo.symbol.clone(),
            currency: mo.currency.clone(),
            exchange: mo.exchange.clone(),
            side: mo.side,
            quantity: qty,
            kind: mo.kind,
            limit_price: mo.limit_price,
            aux_price: mo.aux_price,
            is_entry: mo.is_entry,
        });
    }
    out
}

/// The difference between the desired client working orders and the ones currently live.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkingDiff {
    pub to_place: Vec<WorkingOrder>,
    /// Keys of currently-live client orders that should be cancelled (caller maps key →
    /// order id).
    pub to_cancel: Vec<String>,
}

/// The price field that actually matters for an order of this kind.
fn relevant_price(w: &WorkingOrder) -> f64 {
    match w.kind {
        OrderKind::Limit | OrderKind::StopLimit => w.limit_price,
        OrderKind::Stop => w.aux_price,
        OrderKind::Market => 0.0,
    }
}

/// Two orders are "the same resting order" if they share symbol/side/kind and their
/// quantity and price are within tolerance. The tolerance absorbs the unavoidable churn
/// sources — proportional-sizing quantity drift as balances tick, and IBKR price
/// rounding/capping on read-back — so a matched book is NOT cancelled-and-replaced on
/// every restart or balance wobble. A *material* change (real master re-size or re-price)
/// still falls outside tolerance and triggers a proper replace.
fn same_working_order(a: &WorkingOrder, b: &WorkingOrder) -> bool {
    if a.symbol != b.symbol || a.side != b.side || a.kind != b.kind {
        return false;
    }
    // Quantity: within 3% (or 1 share, whichever is larger).
    let qmax = a.quantity.abs().max(b.quantity.abs());
    let qty_tol = (qmax * 0.03).max(1.0);
    if (a.quantity - b.quantity).abs() > qty_tol {
        return false;
    }
    // Price: within 0.5% (or 1 cent, whichever is larger).
    let (pa, pb) = (relevant_price(a), relevant_price(b));
    let price_tol = (pb.abs() * 0.005).max(0.01);
    (pa - pb).abs() <= price_tol
}

/// Diff desired vs. currently-live client working orders with TOLERANT matching.
///
/// An existing live order that already corresponds to a desired one (same symbol/side/kind,
/// quantity and price within tolerance — see [`same_working_order`]) is left in place. Only
/// desired orders with no live counterpart are placed, and only live orders with no desired
/// counterpart are cancelled. This stops the restart/drift churn where exact-key matching
/// cancelled and re-placed the entire resting book every cycle.
pub fn diff_working_orders(desired: &[WorkingOrder], current: &[WorkingOrder]) -> WorkingDiff {
    let mut used = vec![false; current.len()];
    let mut to_place = Vec::new();

    for d in desired {
        // Find an as-yet-unmatched live order equivalent to this desired one.
        match current.iter().enumerate().find(|(i, c)| !used[*i] && same_working_order(c, d)) {
            Some((i, _)) => used[i] = true, // already satisfied — leave it untouched
            None => to_place.push(d.clone()),
        }
    }

    let to_cancel = current
        .iter()
        .enumerate()
        .filter(|(i, _)| !used[*i])
        .map(|(_, c)| c.key())
        .collect();

    WorkingDiff { to_place, to_cancel }
}

/// Fold *entry* working orders into positions to produce the effective exposure the
/// reconciliation safety net should target. Protective (non-entry) orders are ignored
/// here — they guard an existing position rather than changing intended exposure.
pub fn effective_positions(positions: &[Position], working: &[WorkingOrder]) -> Vec<Position> {
    let mut map: BTreeMap<String, Position> = positions
        .iter()
        .map(|p| (p.symbol.clone(), p.clone()))
        .collect();

    for w in working {
        if !w.is_entry {
            continue;
        }
        let entry = map
            .entry(w.symbol.clone())
            .or_insert_with(|| Position::new(w.symbol.clone(), 0.0));
        entry.net_qty += w.signed_qty();
        entry.currency = w.currency.clone();
        entry.exchange = w.exchange.clone();
    }

    map.into_values().filter(|p| p.net_qty.abs() >= 0.5).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{OrderKind, Position};

    fn equal_balances() -> SizingParams {
        SizingParams {
            master_balance: 100_000.0,
            client_balance: 100_000.0,
            ..Default::default()
        }
    }

    fn wo(symbol: &str, side: Side, qty: f64, is_entry: bool) -> WorkingOrder {
        WorkingOrder {
            symbol: symbol.into(),
            currency: "USD".into(),
            exchange: "SMART".into(),
            side,
            quantity: qty,
            kind: OrderKind::Limit,
            limit_price: 100.0,
            aux_price: 0.0,
            is_entry,
        }
    }

    #[test]
    fn scales_quantity_proportionally() {
        let sizing = SizingParams {
            master_balance: 25_000.0,
            client_balance: 100_000.0,
            ..Default::default()
        };
        let d = desired_working_orders(&[wo("AAPL", Side::Buy, 100.0, true)], &sizing, false, &[]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].quantity, 400.0);
        assert_eq!(d[0].limit_price, 100.0);
    }

    #[test]
    fn long_only_drops_entry_sells() {
        let d = desired_working_orders(
            &[wo("X", Side::Sell, 10.0, true)],
            &equal_balances(),
            true,
            &[],
        );
        assert!(d.is_empty(), "entry sell must be dropped in long-only");
    }

    #[test]
    fn long_only_caps_protective_sell_to_long() {
        // Master protective sell 100, but client only holds 40 long -> cap to 40.
        let d = desired_working_orders(
            &[wo("X", Side::Sell, 100.0, false)],
            &equal_balances(),
            true,
            &[Position::new("X", 40.0)],
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].quantity, 40.0);
    }

    #[test]
    fn diff_places_new_and_cancels_stale() {
        let desired = vec![wo("AAPL", Side::Buy, 10.0, true)];
        let current = vec![wo("TSLA", Side::Sell, 5.0, false)];
        let diff = diff_working_orders(&desired, &current);
        assert_eq!(diff.to_place.len(), 1);
        assert_eq!(diff.to_place[0].symbol, "AAPL");
        assert_eq!(diff.to_cancel.len(), 1);
    }

    #[test]
    fn diff_is_noop_when_matched() {
        let desired = vec![wo("AAPL", Side::Buy, 10.0, true)];
        let current = vec![wo("AAPL", Side::Buy, 10.0, true)];
        let diff = diff_working_orders(&desired, &current);
        assert!(diff.to_place.is_empty());
        assert!(diff.to_cancel.is_empty());
    }

    /// Small quantity drift (balance wobble) must NOT cancel+replace — the cause of the
    /// restart order flood. 1000 vs 1005 shares (0.5%) is within tolerance.
    #[test]
    fn diff_tolerates_small_qty_drift() {
        let desired = vec![wo("AAPL", Side::Buy, 1005.0, true)];
        let current = vec![wo("AAPL", Side::Buy, 1000.0, true)];
        let diff = diff_working_orders(&desired, &current);
        assert!(diff.to_place.is_empty(), "tiny qty drift must not re-place");
        assert!(diff.to_cancel.is_empty(), "tiny qty drift must not cancel");
    }

    /// Tiny price drift (IBKR rounding/capping) must NOT churn. 100.00 vs 100.02 limit.
    #[test]
    fn diff_tolerates_tiny_price_drift() {
        let mut d = wo("AAPL", Side::Buy, 100.0, true);
        d.limit_price = 100.02;
        let c = wo("AAPL", Side::Buy, 100.0, true); // limit_price 100.0
        let diff = diff_working_orders(&[d], &[c]);
        assert!(diff.to_place.is_empty());
        assert!(diff.to_cancel.is_empty());
    }

    /// A MATERIAL change (real master re-size) still replaces: 1000 -> 2000 shares.
    #[test]
    fn diff_replaces_on_material_change() {
        let desired = vec![wo("AAPL", Side::Buy, 2000.0, true)];
        let current = vec![wo("AAPL", Side::Buy, 1000.0, true)];
        let diff = diff_working_orders(&desired, &current);
        assert_eq!(diff.to_place.len(), 1, "material qty change must place new");
        assert_eq!(diff.to_cancel.len(), 1, "material qty change must cancel old");
    }

    #[test]
    fn entry_orders_add_to_effective_exposure() {
        // Flat position + a pending entry buy 100 -> effective long 100.
        let eff = effective_positions(&[], &[wo("AAPL", Side::Buy, 100.0, true)]);
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].net_qty, 100.0);
    }

    #[test]
    fn protective_orders_do_not_change_effective_exposure() {
        // Long 100 with a protective stop-sell 100 -> effective stays long 100.
        let eff = effective_positions(
            &[Position::new("AAPL", 100.0)],
            &[wo("AAPL", Side::Sell, 100.0, false)],
        );
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].net_qty, 100.0, "protective sell must not reduce target exposure");
    }
}
