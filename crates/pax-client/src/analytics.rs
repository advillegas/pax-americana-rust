//! Trade analytics: FIFO round-trip matching + full performance metrics suite.

use std::collections::{BTreeMap, VecDeque};

use crate::state::{Cashflow, FlexTrade, Metrics, NavPoint, RoundTrip};

// ── FIFO round-trip matching ──────────────────────────────────────────────────

pub fn build_round_trips(trades: &[FlexTrade], sectors: &BTreeMap<String, String>) -> Vec<RoundTrip> {
    let mut by_sym: BTreeMap<String, Vec<&FlexTrade>> = BTreeMap::new();
    for t in trades {
        by_sym.entry(t.symbol.clone()).or_default().push(t);
    }

    let mut result = Vec::new();

    for (symbol, sym_trades) in &by_sym {
        struct Lot {
            time: String,
            price: f64,
            qty: f64,
            is_long: bool,
        }
        let mut lots: VecDeque<Lot> = VecDeque::new();
        let mut net: f64 = 0.0;

        for t in sym_trades {
            let qty = t.quantity.abs();
            let is_buy = t.side.contains("BUY") || t.side.contains("BOT");
            let signed = if is_buy { qty } else { -qty };

            if (net >= 0.0 && is_buy) || (net <= 0.0 && !is_buy) || net == 0.0 {
                lots.push_back(Lot {
                    time: t.date_time.clone(),
                    price: t.price,
                    qty,
                    is_long: is_buy,
                });
                net += signed;
            } else {
                let mut remaining = qty;
                while remaining > 1e-9 && !lots.is_empty() {
                    let lot = lots.front_mut().unwrap();
                    let mq = remaining.min(lot.qty);
                    let pnl = if lot.is_long {
                        (t.price - lot.price) * mq
                    } else {
                        (lot.price - t.price) * mq
                    };
                    let cost = lot.price * mq;
                    let ret_pct = if cost > 0.0 { pnl / cost * 100.0 } else { 0.0 };

                    result.push(RoundTrip {
                        symbol: symbol.clone(),
                        side: if lot.is_long { "Long".into() } else { "Short".into() },
                        qty: mq,
                        entry_time: lot.time.clone(),
                        exit_time: t.date_time.clone(),
                        entry_price: lot.price,
                        exit_price: t.price,
                        pnl,
                        return_pct: ret_pct,
                        commission: 0.0,
                        holding_days: days_between(&lot.time, &t.date_time),
                        sector: sectors.get(symbol).cloned().unwrap_or_default(),
                    });

                    lot.qty -= mq;
                    remaining -= mq;
                    if lot.qty <= 1e-9 {
                        lots.pop_front();
                    }
                }
                net += signed;
                if remaining > 1e-9 {
                    lots.push_back(Lot {
                        time: t.date_time.clone(),
                        price: t.price,
                        qty: remaining,
                        is_long: is_buy,
                    });
                }
            }
        }
    }

    result.sort_by(|a, b| a.exit_time.cmp(&b.exit_time));
    result
}

// ── Metrics ───────────────────────────────────────────────────────────────────

pub fn compute_metrics(
    rts: &[RoundTrip],
    nav: &[NavPoint],
    _cashflows: &[Cashflow],
) -> Metrics {
    let mut m = Metrics::default();
    if rts.is_empty() && nav.len() < 2 {
        return m;
    }

    m.total_trades = rts.len();
    let winners: Vec<&RoundTrip> = rts.iter().filter(|t| t.pnl > 0.0).collect();
    let losers: Vec<&RoundTrip> = rts.iter().filter(|t| t.pnl <= 0.0).collect();
    m.winners = winners.len();
    m.losers = losers.len();
    m.win_rate = if m.total_trades > 0 {
        m.winners as f64 / m.total_trades as f64 * 100.0
    } else {
        0.0
    };

    let gross_profit: f64 = winners.iter().map(|t| t.pnl).sum();
    let gross_loss: f64 = losers.iter().map(|t| t.pnl.abs()).sum();
    m.profit_factor = if gross_loss > 0.0 { gross_profit / gross_loss } else { 0.0 };
    m.avg_win = if !winners.is_empty() { gross_profit / winners.len() as f64 } else { 0.0 };
    m.avg_loss = if !losers.is_empty() { -(gross_loss / losers.len() as f64) } else { 0.0 };
    m.payoff_ratio = if m.avg_loss.abs() > 0.0 { m.avg_win / m.avg_loss.abs() } else { 0.0 };
    m.expectancy = if m.total_trades > 0 {
        rts.iter().map(|t| t.pnl).sum::<f64>() / m.total_trades as f64
    } else {
        0.0
    };

    m.best_trade = rts.iter().map(|t| t.pnl).fold(f64::NEG_INFINITY, f64::max);
    m.worst_trade = rts.iter().map(|t| t.pnl).fold(f64::INFINITY, f64::min);
    if m.total_trades == 0 {
        m.best_trade = 0.0;
        m.worst_trade = 0.0;
    }
    m.avg_holding_days = if m.total_trades > 0 {
        rts.iter().map(|t| t.holding_days).sum::<f64>() / m.total_trades as f64
    } else {
        0.0
    };
    m.total_commission = rts.iter().map(|t| t.commission).sum();
    m.long_pnl = rts.iter().filter(|t| t.side == "Long").map(|t| t.pnl).sum();
    m.short_pnl = rts.iter().filter(|t| t.side == "Short").map(|t| t.pnl).sum();
    m.total_pnl = rts.iter().map(|t| t.pnl).sum();

    let mut sym_pnl: BTreeMap<String, f64> = BTreeMap::new();
    for t in rts {
        *sym_pnl.entry(t.symbol.clone()).or_default() += t.pnl;
    }
    m.per_symbol_pnl = sym_pnl;

    let mut sec_pnl: BTreeMap<String, f64> = BTreeMap::new();
    for t in rts {
        let s = if t.sector.is_empty() { "Unknown" } else { &t.sector };
        *sec_pnl.entry(s.to_string()).or_default() += t.pnl;
    }
    m.per_sector_pnl = sec_pnl;

    // NAV-based metrics
    if nav.len() >= 2 {
        let first = nav[0].nav;
        let last = nav.last().unwrap().nav;
        m.total_return = (last - first) / first * 100.0;

        let days = days_between(&nav[0].date, &nav.last().unwrap().date);
        let years = days / 365.25;
        m.cagr = if years > 0.0 {
            ((last / first).powf(1.0 / years) - 1.0) * 100.0
        } else {
            0.0
        };

        let daily: Vec<f64> = nav.windows(2).map(|w| (w[1].nav - w[0].nav) / w[0].nav).collect();
        if !daily.is_empty() {
            let mean = daily.iter().sum::<f64>() / daily.len() as f64;
            let var = daily.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / daily.len() as f64;
            m.volatility = var.sqrt() * 252_f64.sqrt() * 100.0;

            let ann = m.cagr / 100.0;
            m.sharpe = if m.volatility > 0.0 { ann / (m.volatility / 100.0) } else { 0.0 };

            let down_var = daily
                .iter()
                .filter(|r| **r < 0.0)
                .map(|r| r.powi(2))
                .sum::<f64>()
                / daily.len() as f64;
            let down_dev = down_var.sqrt() * 252_f64.sqrt();
            m.sortino = if down_dev > 0.0 { ann / down_dev } else { 0.0 };
        }

        let mut peak = nav[0].nav;
        let mut max_dd = 0.0_f64;
        let mut dd_start = 0usize;
        let mut max_dd_dur = 0usize;

        for (i, p) in nav.iter().enumerate() {
            if p.nav > peak {
                peak = p.nav;
                let dur = i - dd_start;
                if dur > max_dd_dur {
                    max_dd_dur = dur;
                }
                dd_start = i;
            }
            let dd = (peak - p.nav) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
        m.max_drawdown = max_dd * 100.0;
        m.max_dd_duration_days = max_dd_dur as u32;
        m.calmar = if max_dd > 0.0 { (m.cagr / 100.0) / max_dd } else { 0.0 };

        m.monthly_returns = monthly_returns(nav);
    }

    m
}

fn monthly_returns(nav: &[NavPoint]) -> Vec<(i32, u32, f64)> {
    if nav.len() < 2 {
        return Vec::new();
    }
    let mut by_month: BTreeMap<(i32, u32), (f64, f64)> = BTreeMap::new();
    for p in nav {
        if let Some((y, mo, _)) = parse_date(&p.date) {
            let e = by_month.entry((y, mo)).or_insert((p.nav, p.nav));
            e.1 = p.nav;
        }
    }
    let sorted: Vec<_> = by_month.into_iter().collect();
    let mut out = Vec::new();
    for i in 0..sorted.len() {
        let ((y, mo), (_first, last)) = &sorted[i];
        let base = if i > 0 { sorted[i - 1].1 .1 } else { sorted[0].1 .0 };
        let ret = if base > 0.0 { (*last - base) / base * 100.0 } else { 0.0 };
        out.push((*y, *mo, ret));
    }
    out
}

// ── Date helpers ──────────────────────────────────────────────────────────────

pub fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let s = s.replace('-', "");
    if s.len() < 8 {
        return None;
    }
    let y: i32 = s[0..4].parse().ok()?;
    let m: u32 = s[4..6].parse().ok()?;
    let d: u32 = s[6..8].parse().ok()?;
    Some((y, m, d))
}

fn ymd_to_days(y: i32, m: u32, d: u32) -> i64 {
    let m = m as i64;
    let y = y as i64 - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) as u64 + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe as i64 - 719468
}

pub fn days_between(a: &str, b: &str) -> f64 {
    let da = parse_date(a).map(|(y, m, d)| ymd_to_days(y, m, d));
    let db = parse_date(b).map(|(y, m, d)| ymd_to_days(y, m, d));
    match (da, db) {
        (Some(a), Some(b)) => (b - a).abs() as f64,
        _ => 0.0,
    }
}
