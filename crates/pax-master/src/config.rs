//! Master configuration, sourced from environment variables with sane defaults.

use std::env;

use crate::state::IbMode;

#[derive(Debug, Clone)]
pub struct MasterConfig {
    /// IB Gateway / TWS host. Use 127.0.0.1 (never `localhost` — TWS blocks IPv6).
    pub ib_host: String,
    /// Live port (IB Gateway 4001 / TWS 7496).
    pub ib_port_live: u16,
    /// Paper port (IB Gateway 4002 / TWS 7497).
    pub ib_port_paper: u16,
    /// Which mode to start in (toggleable at runtime in the GUI).
    pub start_mode: IbMode,
    /// clientId 0 sees all manually-placed orders/positions in the session.
    pub ib_client_id: i32,
    /// Address the HTTP API binds to.
    pub http_bind: String,
    /// Optional shared secret; when set, clients must send `X-API-Key`.
    pub api_key: String,
    /// How often to refresh balance + positions from IB (seconds).
    pub refresh_secs: u64,
}

impl Default for MasterConfig {
    fn default() -> Self {
        MasterConfig {
            ib_host: "127.0.0.1".to_string(),
            // TWS ports by default (master typically runs against TWS). IB Gateway is
            // 4001 live / 4002 paper — change in the GUI or via env if needed.
            ib_port_live: 7496,
            ib_port_paper: 7497,
            start_mode: IbMode::Paper,
            ib_client_id: 0,
            http_bind: "0.0.0.0:5001".to_string(),
            api_key: String::new(),
            refresh_secs: 5,
        }
    }
}

impl MasterConfig {
    pub fn from_env() -> Self {
        let d = MasterConfig::default();
        // Legacy PAX_IB_PORT (single port) still honored as the paper-port default.
        let legacy_port = env::var("PAX_IB_PORT").ok().and_then(|v| v.parse().ok());
        let start_mode = match env::var("PAX_IB_MODE").unwrap_or_default().to_lowercase().as_str() {
            "live" => IbMode::Live,
            _ => IbMode::Paper,
        };
        MasterConfig {
            ib_host: env::var("PAX_IB_HOST").unwrap_or(d.ib_host),
            ib_port_live: env::var("PAX_IB_PORT_LIVE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.ib_port_live),
            ib_port_paper: env::var("PAX_IB_PORT_PAPER")
                .ok()
                .and_then(|v| v.parse().ok())
                .or(legacy_port)
                .unwrap_or(d.ib_port_paper),
            start_mode,
            ib_client_id: env::var("PAX_IB_CLIENT_ID")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.ib_client_id),
            http_bind: env::var("PAX_HTTP_BIND").unwrap_or(d.http_bind),
            api_key: env::var("PAX_API_KEY").unwrap_or(d.api_key),
            refresh_secs: env::var("PAX_REFRESH_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.refresh_secs),
        }
    }
}
