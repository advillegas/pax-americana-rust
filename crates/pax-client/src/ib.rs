//! IB Gateway / TWS integration for the client: reads, and order placement.

use ibapi::accounts::types::AccountGroup;
use ibapi::accounts::{AccountSummaryResult, PositionUpdate};
use ibapi::client::blocking::Client;
use ibapi::contracts::Contract;
use pax_core::{OrderKind, Position, Side};

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

/// Read NetLiquidation in USD for the connected account.
pub fn read_balance(client: &Client) -> Result<f64, String> {
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

fn nonempty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}
