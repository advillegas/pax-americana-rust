//! Core domain models shared between master and client.

use serde::{Deserialize, Serialize};

/// Order side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// The IBKR action string.
    pub fn as_ib(self) -> &'static str {
        match self {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
    }

    /// The side that reduces / closes a position of the given signed quantity.
    pub fn closing(signed_qty: f64) -> Side {
        if signed_qty > 0.0 {
            Side::Sell
        } else {
            Side::Buy
        }
    }

    /// The side that moves a position in the direction of `delta`.
    pub fn from_delta(delta: f64) -> Side {
        if delta >= 0.0 {
            Side::Buy
        } else {
            Side::Sell
        }
    }
}

/// Order type, mirrored from the master where possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OrderKind {
    #[default]
    #[serde(rename = "MKT")]
    Market,
    #[serde(rename = "LMT")]
    Limit,
    #[serde(rename = "STP")]
    Stop,
    #[serde(rename = "STP LMT")]
    StopLimit,
}

impl OrderKind {
    /// Parse from an IBKR order-type string. Unknown types fall back to Market.
    pub fn from_ib(s: &str) -> OrderKind {
        match s.trim().to_uppercase().as_str() {
            "LMT" => OrderKind::Limit,
            "STP" => OrderKind::Stop,
            "STP LMT" | "STPLMT" => OrderKind::StopLimit,
            _ => OrderKind::Market,
        }
    }

    pub fn as_ib(self) -> &'static str {
        match self {
            OrderKind::Market => "MKT",
            OrderKind::Limit => "LMT",
            OrderKind::Stop => "STP",
            OrderKind::StopLimit => "STP LMT",
        }
    }
}

/// A net position in a single instrument.
///
/// `net_qty` is **signed**: positive is long, negative is short. This is the unit of
/// truth for reconciliation — never an order action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default = "default_exchange")]
    pub exchange: String,
    pub net_qty: f64,
    #[serde(default)]
    pub avg_cost: f64,
    /// Hint of the order type last seen for this symbol on the master.
    #[serde(default)]
    pub order_kind: OrderKind,
    #[serde(default)]
    pub limit_price: f64,
    #[serde(default)]
    pub aux_price: f64,
}

fn default_currency() -> String {
    "USD".to_string()
}

fn default_exchange() -> String {
    "SMART".to_string()
}

impl Position {
    pub fn new(symbol: impl Into<String>, net_qty: f64) -> Self {
        Position {
            symbol: symbol.into(),
            currency: default_currency(),
            exchange: default_exchange(),
            net_qty,
            avg_cost: 0.0,
            order_kind: OrderKind::Market,
            limit_price: 0.0,
            aux_price: 0.0,
        }
    }

    pub fn is_long(&self) -> bool {
        self.net_qty > 0.0
    }

    pub fn is_short(&self) -> bool {
        self.net_qty < 0.0
    }
}
