//! Persist the matched "ledger" (master-change gate state) across client restarts.
//!
//! Without this, a restart resets the gate to empty, so the engine recomputes every target
//! from the current balances and resizes the whole book on the next sync — the exact
//! increase/decrease churn we want to avoid. By saving the locked fingerprint + targets +
//! mirror orders, a restart resumes the SAME matched structure and only resizes if the
//! master's ledger actually changed while the client was down.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use pax_core::WorkingOrder;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Ledger {
    /// IBKR account this ledger belongs to; a mismatch is ignored (never cross accounts).
    pub account: String,
    /// Balance-independent signature of the master's ledger at the last sync.
    pub fingerprint: Option<String>,
    /// Locked per-symbol signed target net quantities.
    pub targets: BTreeMap<String, f64>,
    /// Locked desired mirror (resting) orders.
    pub desired: Vec<WorkingOrder>,
}

fn path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("pax-client.ledger.json"))
}

/// Load the saved ledger if it exists AND belongs to `account`. Returns `None` otherwise
/// (a fresh start), so a different account never inherits another account's targets.
pub fn load(account: &str) -> Option<Ledger> {
    let p = path()?;
    let s = fs::read_to_string(&p).ok()?;
    let l: Ledger = serde_json::from_str(&s).ok()?;
    if l.account == account {
        Some(l)
    } else {
        None
    }
}

/// Write the current gate state to disk (best-effort; ignores I/O errors).
pub fn save(
    account: &str,
    fingerprint: &Option<String>,
    targets: &BTreeMap<String, f64>,
    desired: &[WorkingOrder],
) {
    let Some(p) = path() else { return };
    let l = Ledger {
        account: account.to_string(),
        fingerprint: fingerprint.clone(),
        targets: targets.clone(),
        desired: desired.to_vec(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&l) {
        let _ = fs::write(p, json);
    }
}
