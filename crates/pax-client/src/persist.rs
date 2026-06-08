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
    #[serde(default)]
    alerts_enabled: bool,
    #[serde(default)]
    alert_email: String,
    #[serde(default = "default_alert_hours")]
    alert_after_hours: f64,
    #[serde(default)]
    smtp_host: String,
    #[serde(default = "default_smtp_port")]
    smtp_port: u16,
    #[serde(default)]
    smtp_user: String,
    #[serde(default)]
    smtp_pass: String,
    #[serde(default)]
    smtp_from: String,
}

fn default_alert_hours() -> f64 {
    2.0
}
fn default_smtp_port() -> u16 {
    587
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
    c.alerts_enabled = ps.alerts_enabled;
    c.alert_email = ps.alert_email;
    c.alert_after_hours = ps.alert_after_hours;
    c.smtp_host = ps.smtp_host;
    c.smtp_port = ps.smtp_port;
    c.smtp_user = ps.smtp_user;
    c.smtp_pass = ps.smtp_pass;
    c.smtp_from = ps.smtp_from;
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
        alerts_enabled: c.alerts_enabled,
        alert_email: c.alert_email.clone(),
        alert_after_hours: c.alert_after_hours,
        smtp_host: c.smtp_host.clone(),
        smtp_port: c.smtp_port,
        smtp_user: c.smtp_user.clone(),
        smtp_pass: c.smtp_pass.clone(),
        smtp_from: c.smtp_from.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec(&ps) {
        appdata::write(FILE, bytes);
    }
}
