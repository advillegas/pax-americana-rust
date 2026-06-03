//! pax-core — shared types and the safety-critical reconciliation engine for the
//! Pax Americana copy-trading system.
//!
//! The defining design choice of this rewrite is **target-position reconciliation**
//! instead of blind order mirroring. The master publishes its *net position* per
//! symbol; the client computes a proportionally-scaled target and only ever trades
//! the delta required to reach it. This makes it structurally impossible to open an
//! accidental short when the master merely closes a long (the bug class that plagues
//! action-mirroring systems): when the master goes flat, the client's target is `0`,
//! so the client closes toward flat and stops — it never crosses zero unless the
//! master itself is genuinely net short.

pub mod model;
pub mod orders;
pub mod protocol;
pub mod reconcile;
pub mod sizing;
pub mod theme;

pub use model::{OrderKind, Position, Side, WorkingOrder};
pub use orders::{desired_working_orders, diff_working_orders, effective_positions, WorkingDiff};
pub use protocol::{MasterSnapshot, PROTOCOL_SCHEMA};
pub use reconcile::{reconcile, IntentReason, ReconcileInput, ReconcileResult, SkipNote, TradeIntent};
pub use sizing::{round_qty, target_net_qty, SizingParams};
