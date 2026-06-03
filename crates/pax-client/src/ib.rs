//! IB Gateway / TWS integration for the client: reads, and order placement.

use std::time::{Duration, Instant};

use ibapi::accounts::types::AccountGroup;
use ibapi::accounts::{AccountSummaryResult, PositionUpdate};
use ibapi::client::blocking::Client;
use ibapi::contracts::Contract;
use ibapi::orders::{Action, Order, Orders, PlaceOrder};
use pax_core::{OrderKind, Position, Side, WorkingOrder};

/// Connect briefly to the local IB Gateway/TWS and return the accounts this login
/// manages — used to populate the GUI account picker. The connection drops on return.
pub fn list_accounts(endpoint: &str, client_id: i32) -> Result<Vec<String>, String> {
    let client = Client::connect(endpoint, client_id).map_err(|e| e.to_string())?;
    let mut accts = client.managed_accounts().map_err(|e| e.to_string())?;
    accts.retain(|a| !a.trim().is_empty());
    Ok(accts)
}

/// Snapshot the client's net positions for `account` only (drains until `PositionEnd`).
/// Positions in any OTHER account the login can access are ignored — critical when a
/// login manages more than one account.
pub fn read_positions(client: &Client, account: &str) -> Result<Vec<Position>, String> {
    let sub = client.positions().map_err(|e| e.to_string())?;
    let mut out: Vec<Position> = Vec::new();
    for update in &sub {
        match update {
            PositionUpdate::Position(p) => {
                if p.position == 0.0 || p.account != account {
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

/// Read NetLiquidation + margin/SMA fields for `account` only. The "All" group returns
/// per-account rows; we take only the target account's so a multi-account login can't
/// skew sizing/margin by aggregating across accounts.
pub fn read_margin(client: &Client, account: &str) -> Result<MarginInfo, String> {
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
                if s.account != account {
                    continue;
                }
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

/// Build a stock order pinned to `account`.
#[allow(clippy::too_many_arguments)]
fn build_order(account: &str, side: Side, qty: f64, kind: OrderKind, limit_price: f64, aux_price: f64) -> Order {
    Order {
        action: match side {
            Side::Buy => Action::Buy,
            Side::Sell => Action::Sell,
        },
        order_type: kind.as_ib().to_string(),
        total_quantity: qty,
        limit_price: matches!(kind, OrderKind::Limit | OrderKind::StopLimit).then_some(limit_price),
        aux_price: matches!(kind, OrderKind::Stop | OrderKind::StopLimit).then_some(aux_price),
        account: account.to_string(),
        ..Default::default()
    }
}

/// Place a single order on `account`. `qty` must be a positive whole share count.
/// Returns the assigned IBKR order id so the caller can correlate later status updates
/// (fills / rejections) back to the symbol that was ordered.
#[allow(clippy::too_many_arguments)]
pub fn place_order(
    client: &Client,
    account: &str,
    symbol: &str,
    currency: &str,
    exchange: &str,
    side: Side,
    qty: f64,
    kind: OrderKind,
    limit_price: f64,
    aux_price: f64,
) -> Result<i32, String> {
    let contract = Contract::stock(symbol)
        .in_currency(currency)
        .on_exchange(exchange)
        .build();
    let order = build_order(account, side, qty, kind, limit_price, aux_price);
    let id = client.next_order_id();
    client.submit_order(id, &contract, &order).map_err(|e| e.to_string())?;
    Ok(id)
}

/// A single live (working) order on the account, regardless of type. Market orders are
/// included here (unlike [`read_open_orders`]) so the engine can see its own in-flight
/// orders and avoid stacking duplicates on the same contract/side.
#[derive(Debug, Clone)]
pub struct LiveOrder {
    pub id: i32,
    pub symbol: String,
    pub currency: String,
    pub exchange: String,
    pub side: Side,
    pub qty: f64,
    pub kind: OrderKind,
    pub limit_price: f64,
    pub aux_price: f64,
}

impl LiveOrder {
    /// View this live order as a [`WorkingOrder`] (used for the mirror diff). `is_entry`
    /// is not knowable from the order alone, so it is left false here.
    pub fn to_working(&self) -> WorkingOrder {
        WorkingOrder {
            symbol: self.symbol.clone(),
            currency: self.currency.clone(),
            exchange: self.exchange.clone(),
            side: self.side,
            quantity: self.qty,
            kind: self.kind,
            limit_price: self.limit_price,
            aux_price: self.aux_price,
            is_entry: false,
        }
    }
}

/// Read ALL of this client's live/working orders for `account` (any order type, including
/// market). Scoped to the target account so a multi-account login is never affected.
pub fn read_live_orders(client: &Client, account: &str) -> Result<Vec<LiveOrder>, String> {
    let sub = client.open_orders().map_err(|e| e.to_string())?;
    let mut out: Vec<LiveOrder> = Vec::new();
    for item in &sub {
        if let Orders::OrderData(d) = item {
            if d.order.account != account {
                continue;
            }
            let qty = d.order.total_quantity.abs();
            if qty == 0.0 {
                continue;
            }
            let side = match d.order.action {
                Action::Buy => Side::Buy,
                _ => Side::Sell,
            };
            out.push(LiveOrder {
                id: d.order_id,
                symbol: d.contract.symbol.to_string(),
                currency: nonempty(d.contract.currency.to_string(), "USD"),
                exchange: nonempty(d.contract.exchange.to_string(), "SMART"),
                side,
                qty,
                kind: OrderKind::from_ib(&d.order.order_type),
                limit_price: d.order.limit_price.unwrap_or(0.0),
                aux_price: d.order.aux_price.unwrap_or(0.0),
            });
        }
    }
    Ok(out)
}

/// Read this client's own resting limit/stop/stop-limit orders **for `account`**, paired
/// with their order ids (so stale mirrors can be cancelled). Market orders are excluded —
/// use [`read_live_orders`] when in-flight market orders matter.
pub fn read_open_orders(client: &Client, account: &str) -> Result<Vec<(i32, WorkingOrder)>, String> {
    Ok(read_live_orders(client, account)?
        .into_iter()
        .filter(|o| !matches!(o.kind, OrderKind::Market))
        .map(|o| (o.id, o.to_working()))
        .collect())
}

/// Cancel a single order by id.
pub fn cancel_order(client: &Client, order_id: i32) -> Result<(), String> {
    client.cancel_order(order_id, "").map(|_| ()).map_err(|e| e.to_string())
}

/// Ask IBKR (via a what-if order, which is NOT executed) how much **initial margin** this
/// order would add to the account. Returns that delta in account currency. The caller
/// uses it to ensure cumulative opening orders stay within buying power.
#[allow(clippy::too_many_arguments)]
pub fn what_if_init_margin(
    client: &Client,
    account: &str,
    symbol: &str,
    currency: &str,
    exchange: &str,
    side: Side,
    qty: f64,
    kind: OrderKind,
    limit_price: f64,
    aux_price: f64,
) -> Result<f64, String> {
    let contract = Contract::stock(symbol).in_currency(currency).on_exchange(exchange).build();
    let mut order = build_order(account, side, qty, kind, limit_price, aux_price);
    order.what_if = true;
    let order_id = client.next_order_id();
    let sub = client.place_order(order_id, &contract, &order).map_err(|e| e.to_string())?;

    // Poll for the what-if response, bounded so we never hang the engine thread.
    let start = Instant::now();
    loop {
        match sub.try_next() {
            Some(PlaceOrder::OpenOrder(d)) => {
                return d
                    .order_state
                    .initial_margin_change
                    .ok_or_else(|| "what-if returned no margin data".to_string());
            }
            Some(PlaceOrder::Message(m)) => return Err(m.message),
            Some(_) => {} // status/other — keep waiting for the OpenOrder
            None => {
                if start.elapsed() > Duration::from_secs(4) {
                    return Err("what-if timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

fn nonempty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}
