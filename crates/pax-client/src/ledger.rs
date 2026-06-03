//! Persist the matched "ledger" (master-change gate state) across client restarts.
//!
//! Without this, a restart resets the gate to empty, so the engine recomputes every target
//! from the current balances and resizes the whole book on the next sync — the exact
//! increase/decrease churn we want to avoid. By saving the locked fingerprint + targets +
//! mirror orders, a restart resumes the SAME matched structure and only resizes if the
//! master's ledger actually changed while the client was down.
//!
//! Stored as a hidden, obfuscated file under LocalAppData (see [`crate::appdata`]).

use std::collections::BTreeMap;
use std::fs;

use pax_core::WorkingOrder;
use serde::{Deserialize, Serialize};

use crate::appdata;

const FILE: &str = "lx.dat";
const LEGACY: &str = "pax-client.ledger.json";

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

/// Load the saved ledger if it exists AND belongs to `account`. Returns `None` otherwise
/// (a fresh start), so a different account never inherits another account's targets.
/// Transparently migrates a legacy plaintext file, then removes it.
pub fn load(account: &str) -> Option<Ledger> {
    if let Some(bytes) = appdata::read(FILE) {
        if let Ok(l) = serde_json::from_slice::<Ledger>(&bytes) {
            return if l.account == account { Some(l) } else { None };
        }
    }
    // Migration: pick up an old plaintext ledger once, then delete it so it isn't left
    // openly available (re-save to the hidden location happens on the next periodic write).
    if let Some(old) = appdata::legacy(LEGACY) {
        if let Ok(s) = fs::read_to_string(&old) {
            let _ = fs::remove_file(&old);
            if let Ok(l) = serde_json::from_str::<Ledger>(&s) {
                return if l.account == account { Some(l) } else { None };
            }
        }
    }
    None
}

/// Write the current gate state to disk (best-effort; ignores I/O errors).
pub fn save(
    account: &str,
    fingerprint: &Option<String>,
    targets: &BTreeMap<String, f64>,
    desired: &[WorkingOrder],
) {
    let l = Ledger {
        account: account.to_string(),
        fingerprint: fingerprint.clone(),
        targets: targets.clone(),
        desired: desired.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec(&l) {
        appdata::write(FILE, bytes);
    }
}
