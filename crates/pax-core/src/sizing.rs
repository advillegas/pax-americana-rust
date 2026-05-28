//! Proportional position sizing and per-instrument risk clamps.

/// Inputs that scale a master net quantity into a client target net quantity.
#[derive(Debug, Clone, Copy)]
pub struct SizingParams {
    /// User multiplier (e.g. 0.1x .. 5.0x).
    pub multiplier: f64,
    /// Master NetLiquidation (USD).
    pub master_balance: f64,
    /// Client NetLiquidation (USD).
    pub client_balance: f64,
    /// Cap on absolute notional per instrument (USD). `0` disables.
    pub max_position_notional: f64,
    /// Hard cap on absolute share count per instrument. `0` disables.
    pub max_position_qty: f64,
    /// When the master holds a non-zero position that scales below one share,
    /// still take a one-share position in the master's direction.
    pub force_min_one: bool,
}

impl Default for SizingParams {
    fn default() -> Self {
        SizingParams {
            multiplier: 1.0,
            master_balance: 0.0,
            client_balance: 0.0,
            max_position_notional: 0.0,
            max_position_qty: 0.0,
            force_min_one: true,
        }
    }
}

impl SizingParams {
    /// Proportional ratio `client_balance / master_balance`, or `None` if balances
    /// are not yet known (in which case the caller must not trade).
    pub fn ratio(&self) -> Option<f64> {
        if self.master_balance <= 0.0 || self.client_balance <= 0.0 {
            None
        } else {
            Some(self.client_balance / self.master_balance)
        }
    }
}

/// Round to whole shares, half away from zero, preserving sign.
pub fn round_qty(q: f64) -> f64 {
    if q >= 0.0 {
        (q + 0.5).floor()
    } else {
        (q - 0.5).ceil()
    }
}

/// Compute the client's **target signed net quantity** for one instrument.
///
/// Returns `None` if balances are unknown (caller must skip trading). The result is
/// rounded to whole shares and clamped by the configured risk limits. `price` (the
/// master avg cost or a reference price) is used for the notional clamp; pass `0.0`
/// to disable notional-based clamping for this call.
pub fn target_net_qty(master_net: f64, price: f64, p: &SizingParams) -> Option<f64> {
    let ratio = p.ratio()?;
    let scaled = master_net * ratio * p.multiplier;
    let mut target = round_qty(scaled);

    // Keep participation when the master is in the market but scaling rounds to zero.
    if target == 0.0 && master_net != 0.0 && p.force_min_one {
        target = master_net.signum();
    }

    // Absolute share cap.
    if p.max_position_qty > 0.0 && target.abs() > p.max_position_qty {
        target = p.max_position_qty * target.signum();
    }

    // Notional cap.
    if p.max_position_notional > 0.0 && price > 0.0 {
        let max_by_notional = (p.max_position_notional / price).floor();
        if target.abs() > max_by_notional {
            target = max_by_notional * target.signum();
        }
    }

    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_balances_block() {
        let p = SizingParams::default();
        assert_eq!(target_net_qty(100.0, 10.0, &p), None);
    }

    #[test]
    fn proportional_scaling() {
        // Client is 4x the master -> 100 shares scales to 400.
        let p = SizingParams {
            master_balance: 25_000.0,
            client_balance: 100_000.0,
            ..Default::default()
        };
        assert_eq!(target_net_qty(100.0, 0.0, &p), Some(400.0));
    }

    #[test]
    fn multiplier_applies() {
        let p = SizingParams {
            master_balance: 100_000.0,
            client_balance: 100_000.0,
            multiplier: 2.5,
            ..Default::default()
        };
        assert_eq!(target_net_qty(10.0, 0.0, &p), Some(25.0));
    }

    #[test]
    fn short_target_preserves_sign() {
        let p = SizingParams {
            master_balance: 50_000.0,
            client_balance: 50_000.0,
            ..Default::default()
        };
        assert_eq!(target_net_qty(-30.0, 0.0, &p), Some(-30.0));
    }

    #[test]
    fn notional_clamp() {
        let p = SizingParams {
            master_balance: 10_000.0,
            client_balance: 1_000_000.0, // 100x
            max_position_notional: 5_000.0,
            ..Default::default()
        };
        // 10 * 100 = 1000 shares, but $5000 / $50 = 100 share cap.
        assert_eq!(target_net_qty(10.0, 50.0, &p), Some(100.0));
    }
}
