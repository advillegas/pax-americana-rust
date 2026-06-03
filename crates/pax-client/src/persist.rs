//! Persist client settings to a JSON file beside the executable so operator choices
//! survive restarts.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::{AccountMode, Controls, ExecutionMode, TradeMode};

#[derive(Serialize, Deserialize)]
struct Persisted {
    account_mode: AccountMode,
    trade_mode: TradeMode,
    execution_mode: ExecutionMode,
    multiplier: f64,
    max_drawdown_pct: f64,
    max_position_notional: f64,
    max_position_qty: f64,
    ib_host: String,
    ib_port_live: u16,
    ib_port_paper: u16,
    ib_account: String,
}

fn path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("pax-client.settings.json"))
}

/// Overlay saved settings onto `c` (called at startup, after env/config defaults).
pub fn load_into(c: &mut Controls) {
    let Some(p) = path() else { return };
    let Ok(s) = fs::read_to_string(&p) else { return };
    let Ok(ps) = serde_json::from_str::<Persisted>(&s) else { return };
    c.account_mode = ps.account_mode;
    c.trade_mode = ps.trade_mode;
    c.execution_mode = ps.execution_mode;
    c.multiplier = ps.multiplier;
    c.max_drawdown_pct = ps.max_drawdown_pct;
    c.max_position_notional = ps.max_position_notional;
    c.max_position_qty = ps.max_position_qty;
    c.ib_host = ps.ib_host;
    c.ib_port_live = ps.ib_port_live;
    c.ib_port_paper = ps.ib_port_paper;
    c.ib_account = ps.ib_account;
}

/// Write the current settings to disk (best-effort; ignores I/O errors).
pub fn save(c: &Controls) {
    let Some(p) = path() else { return };
    let ps = Persisted {
        account_mode: c.account_mode,
        trade_mode: c.trade_mode,
        execution_mode: c.execution_mode,
        multiplier: c.multiplier,
        max_drawdown_pct: c.max_drawdown_pct,
        max_position_notional: c.max_position_notional,
        max_position_qty: c.max_position_qty,
        ib_host: c.ib_host.clone(),
        ib_port_live: c.ib_port_live,
        ib_port_paper: c.ib_port_paper,
        ib_account: c.ib_account.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&ps) {
        let _ = fs::write(p, json);
    }
}
