//! IB Gateway / TWS integration for the client: reads, and order placement.

use ibapi::accounts::types::AccountGroup;
use ibapi::accounts::{AccountSummaryResult, PositionUpdate};
use ibapi::client::blocking::Client;
use ibapi::contracts::Contract;
use ibapi::orders::{Action, Orders};
use pax_core::{OrderKind, Position, Side, WorkingOrder};

/// Snapshot the client's net positions (drains until `PositionEnd`).
pub fn read_positions(client: &Client) -> Result<Vec<Position>, String> {
    let sub = client.positions().map_err(|e| e.to_string())?;
    let mut out: Vec<Position> = Vec::new();
    for update in &sub {
        match update {
            PositionUpdate::Position(p) => {
                if p.position == 0.0 {
                    continue;
                }
                out.push(Position {
                    symbol: p.contract.symbol.to_string(),
                    currency: nonempty(p.contract.currency.to_string(), "USD"),
                    exchange: nonempty(p.contract.exchange.to_string(), "SMART"),
                    net_qty: p.position,
                    avg_cost: p.average_cost,
                    order_kind: OrderKind::Market,
                    limit_price: 0.0,
                    aux_price: 0.0,
                });
            }
            PositionUpdate::PositionEnd => break,
        }
    }
    Ok(out)
}

/// Account margin / SMA snapshot used for risk gating.
#[derive(Debug, Clone, Copy, Default)]
pub struct MarginInfo {
    pub net_liq: f64,
    pub excess_liquidity: f64,
    pub available_funds: f64,
    pub buying_power: f64,
    /// IBKR Cushion = (ELV - maintenance margin) / ELV, in 0..1 (0 ⇒ liquidation).
    pub cushion: f64,
    /// Special Memorandum Account; negative ⇒ Reg-T / Fed call.
    pub sma: f64,
    pub maint_margin: f64,
}

/// Read NetLiquidation + margin/SMA fields for the connected account in one request.
pub fn read_margin(client: &Client) -> Result<MarginInfo, String> {
    let group = AccountGroup("All".to_string());
    let tags = [
        "NetLiquidation",
        "ExcessLiquidity",
        "AvailableFunds",
        "BuyingPower",
        "Cushion",
        "SMA",
        "FullMaintMarginReq",
    ];
    let sub = client.account_summary(&group, &tags).map_err(|e| e.to_string())?;
    let mut m = MarginInfo::default();
    for item in &sub {
        match item {
            AccountSummaryResult::Summary(s) => {
                let v = s.value.parse::<f64>().unwrap_or(0.0);
                match s.tag.as_str() {
                    "NetLiquidation" => m.net_liq = v,
                    "ExcessLiquidity" => m.excess_liquidity = v,
                    "AvailableFunds" => m.available_funds = v,
                    "BuyingPower" => m.buying_power = v,
                    "Cushion" => m.cushion = v,
                    "SMA" => m.sma = v,
                    "FullMaintMarginReq" => m.maint_margin = v,
                    _ => {}
                }
            }
            AccountSummaryResult::End => break,
        }
    }
    Ok(m)
}

/// Place a single order described by side/qty/kind. `qty` must be a positive whole
/// share count. Returns the assigned order id.
#[allow(clippy::too_many_arguments)]
pub fn place_order(
    client: &Client,
    symbol: &str,
    currency: &str,
    exchange: &str,
    side: Side,
    qty: f64,
    kind: OrderKind,
    limit_price: f64,
    aux_price: f64,
) -> Result<(), String> {
    let contract = Contract::stock(symbol)
        .in_currency(currency)
        .on_exchange(exchange)
        .build();

    let qb = client.order(&contract);
    let sized = match side {
        Side::Buy => qb.buy(qty),
        Side::Sell => qb.sell(qty),
    };
    let result = match kind {
        OrderKind::Market => sized.market().submit(),
        OrderKind::Limit => sized.limit(limit_price).submit(),
        OrderKind::Stop => sized.stop(aux_price).submit(),
        OrderKind::StopLimit => sized.stop_limit(aux_price, limit_price).submit(),
    };
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// Cancel every working order in the account.
pub fn cancel_all(client: &Client) -> Result<(), String> {
    client.global_cancel().map_err(|e| e.to_string())
}

/// Read this client's own resting limit/stop/stop-limit orders, paired with their order
/// ids (so stale mirrors can be cancelled). Scoped to this API client id, so it never
/// returns the user's manual TWS orders.
pub fn read_open_orders(client: &Client) -> Result<Vec<(i32, WorkingOrder)>, String> {
    let sub = client.open_orders().map_err(|e| e.to_string())?;
    let mut out: Vec<(i32, WorkingOrder)> = Vec::new();
    for item in &sub {
        if let Orders::OrderData(d) = item {
            let kind = OrderKind::from_ib(&d.order.order_type);
            if matches!(kind, OrderKind::Market) {
                continue;
            }
            let side = match d.order.action {
                Action::Buy => Side::Buy,
                _ => Side::Sell,
            };
            let qty = d.order.total_quantity.abs();
            if qty == 0.0 {
                continue;
            }
            out.push((
                d.order_id,
                WorkingOrder {
                    symbol: d.contract.symbol.to_string(),
                    currency: nonempty(d.contract.currency.to_string(), "USD"),
                    exchange: nonempty(d.contract.exchange.to_string(), "SMART"),
                    side,
                    quantity: qty,
                    kind,
                    limit_price: d.order.limit_price.unwrap_or(0.0),
                    aux_price: d.order.aux_price.unwrap_or(0.0),
                    is_entry: false,
                },
            ));
        }
    }
    Ok(out)
}

/// Cancel a single order by id.
pub fn cancel_order(client: &Client, order_id: i32) -> Result<(), String> {
    client.cancel_order(order_id, "").map(|_| ()).map_err(|e| e.to_string())
}

fn nonempty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}
