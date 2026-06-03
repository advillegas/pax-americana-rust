//! Wire protocol between master and client.
//!
//! The master serves a single authoritative [`MasterSnapshot`] document. The client
//! reconciles against this snapshot rather than against a stream of order events, which
//! is what makes orphan-close and missed-open detection robust and idempotent.

use serde::{Deserialize, Serialize};

use crate::model::{Position, WorkingOrder};

/// Bump when the snapshot shape changes incompatibly.
pub const PROTOCOL_SCHEMA: u32 = 2;

/// Full, self-contained view of the master account at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterSnapshot {
    /// Protocol schema version — the client refuses mismatched majors.
    pub schema: u32,
    /// Whether the master is currently connected to its IB Gateway/TWS.
    pub connected: bool,
    /// Master account id (informational).
    pub account: String,
    /// Master NetLiquidation in USD. Drives proportional sizing.
    pub balance: f64,
    /// Net positions per symbol (signed quantities).
    pub positions: Vec<Position>,
    /// Resting limit/stop/stop-limit orders the client should mirror.
    #[serde(default)]
    pub working_orders: Vec<WorkingOrder>,
    /// Unix epoch millis when this snapshot was generated.
    pub generated_at_ms: u64,
}

impl Default for MasterSnapshot {
    fn default() -> Self {
        MasterSnapshot {
            schema: PROTOCOL_SCHEMA,
            connected: false,
            account: String::new(),
            balance: 0.0,
            positions: Vec::new(),
            working_orders: Vec::new(),
            generated_at_ms: 0,
        }
    }
}

impl MasterSnapshot {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(s: &str) -> Result<MasterSnapshot, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Lightweight status document served at `/status` and `/balance`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusDoc {
    pub status: &'static str,
    pub connected: bool,
    pub balance: f64,
    pub total_positions: usize,
    pub schema: u32,
}
