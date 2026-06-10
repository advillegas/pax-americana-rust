//! Persist client settings so operator choices survive restarts.
//!
//! Stored as a hidden, obfuscated file under LocalAppData (see [`crate::appdata`]) rather
//! than a readable JSON beside the executable.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::appdata;
use crate::state::{AccountMode, Controls, ExecutionMode, TradeMode};

const FILE: &str = "st.dat";
const LEGACY: &str = "pax-client.settings.json";

#[derive(Serialize, Deserialize)]
struct Persisted {
    account_mode: AccountMode,
    trade_mode: TradeMode,
    execution_mode: ExecutionMode,
    multiplier: f64,
    max_drawdown_pct: f64,
    ib_host: String,
    ib_port_live: u16,
    ib_port_paper: u16,
    ib_account: String,
    /// Default false so older settings (without this key) keep 24h behavior.
    #[serde(default)]
    rth_only: bool,
}

fn apply(c: &mut Controls, ps: Persisted) {
    c.account_mode = ps.account_mode;
    c.trade_mode = ps.trade_mode;
    c.execution_mode = ps.execution_mode;
    c.multiplier = ps.multiplier;
    c.max_drawdown_pct = ps.max_drawdown_pct;
    c.ib_host = ps.ib_host;
    c.ib_port_live = ps.ib_port_live;
    c.ib_port_paper = ps.ib_port_paper;
    c.ib_account = ps.ib_account;
    c.rth_only = ps.rth_only;
}

/// Overlay saved settings onto `c` (called at startup, after env/config defaults).
/// Transparently migrates a legacy plaintext file, then removes it and re-saves hidden.
pub fn load_into(c: &mut Controls) {
    if let Some(bytes) = appdata::read(FILE) {
        if let Ok(ps) = serde_json::from_slice::<Persisted>(&bytes) {
            apply(c, ps);
            return;
        }
    }
    if let Some(old) = appdata::legacy(LEGACY) {
        if let Ok(s) = fs::read_to_string(&old) {
            let _ = fs::remove_file(&old);
            if let Ok(ps) = serde_json::from_str::<Persisted>(&s) {
                apply(c, ps);
                save(c); // re-persist to the hidden location
            }
        }
    }
}

/// Write the current settings to disk (best-effort; ignores I/O errors).
pub fn save(c: &Controls) {
    let ps = Persisted {
        account_mode: c.account_mode,
        trade_mode: c.trade_mode,
        execution_mode: c.execution_mode,
        multiplier: c.multiplier,
        max_drawdown_pct: c.max_drawdown_pct,
        ib_host: c.ib_host.clone(),
        ib_port_live: c.ib_port_live,
        ib_port_paper: c.ib_port_paper,
        ib_account: c.ib_account.clone(),
        rth_only: c.rth_only,
    };
    if let Ok(bytes) = serde_json::to_vec(&ps) {
        appdata::write(FILE, bytes);
    }
}
