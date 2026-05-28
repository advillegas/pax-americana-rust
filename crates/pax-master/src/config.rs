//! Master configuration, sourced from environment variables with sane defaults.

use std::env;

#[derive(Debug, Clone)]
pub struct MasterConfig {
    /// IB Gateway / TWS host. Use 127.0.0.1 (never `localhost` — TWS blocks IPv6).
    pub ib_host: String,
    /// IB Gateway: 4001 (live) / 4002 (paper). TWS: 7496 (live) / 7497 (paper).
    pub ib_port: u16,
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
            ib_port: 4002,
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
        MasterConfig {
            ib_host: env::var("PAX_IB_HOST").unwrap_or(d.ib_host),
            ib_port: env::var("PAX_IB_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.ib_port),
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

    pub fn ib_endpoint(&self) -> String {
        format!("{}:{}", self.ib_host, self.ib_port)
    }
}
