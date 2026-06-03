//! Persist the matched "ledger" (master-change gate state) across client restarts.
//!
//! Without this, a restart resets the gate to empty, so the engine recomputes every target
//! from the current balances and resizes the whole book on the next sync — the exact
//! increase/decrease churn we want to avoid. By saving the locked fingerprint + targets +
//! mirror orders, a restart resumes the SAME matched structure and only resizes if the
//! master's ledger actually changed while the client was down.
//!
//! Storage is deliberately out-of-sight: a hidden, obfuscated file under the user's
//! LocalAppData rather than a readable JSON beside the executable.

use std::collections::BTreeMap;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use pax_core::WorkingOrder;
use serde::{Deserialize, Serialize};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Repeating XOR key — not cryptographic, just keeps the file from being plainly readable.
const OBFS_KEY: &[u8] = b"px-amrcn-2026-ledger-obfuscation-key";

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

/// Hidden app-data directory, created (and hidden) on first use.
fn dir() -> Option<PathBuf> {
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .or_else(|| std::env::var("APPDATA").ok())
        .or_else(|| std::env::var("USERPROFILE").ok())?;
    let d = PathBuf::from(base).join("NeroAI").join("cache");
    let existed = d.exists();
    fs::create_dir_all(&d).ok()?;
    if !existed {
        hide(&d);
    }
    Some(d)
}

fn path() -> Option<PathBuf> {
    Some(dir()?.join("lx.dat"))
}

/// Legacy plaintext location (beside the exe) — migrated away from and deleted.
fn legacy_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("pax-client.ledger.json"))
}

/// Best-effort Windows "hidden" attribute (no console flash).
fn hide(p: &Path) {
    let _ = std::process::Command::new("attrib")
        .args(["+h", &p.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

/// Reversible, non-cryptographic byte obfuscation so the file isn't human-readable.
fn obfuscate(data: &mut [u8]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= OBFS_KEY[i % OBFS_KEY.len()];
    }
}

/// Load the saved ledger if it exists AND belongs to `account`. Returns `None` otherwise
/// (a fresh start), so a different account never inherits another account's targets.
/// Transparently migrates a legacy plaintext file, then removes it.
pub fn load(account: &str) -> Option<Ledger> {
    // Preferred: hidden, obfuscated file.
    if let Some(p) = path() {
        if let Ok(mut bytes) = fs::read(&p) {
            obfuscate(&mut bytes);
            if let Ok(l) = serde_json::from_slice::<Ledger>(&bytes) {
                return if l.account == account { Some(l) } else { None };
            }
        }
    }
    // Migration: pick up an old plaintext ledger once, then delete it so it isn't left
    // openly available. Re-save happens on the next periodic write.
    if let Some(old) = legacy_path() {
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
    let Some(p) = path() else { return };
    let l = Ledger {
        account: account.to_string(),
        fingerprint: fingerprint.clone(),
        targets: targets.clone(),
        desired: desired.to_vec(),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&l) {
        obfuscate(&mut bytes);
        if fs::write(&p, bytes).is_ok() {
            hide(&p);
        }
    }
}
