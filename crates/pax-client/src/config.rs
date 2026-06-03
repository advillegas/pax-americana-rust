//! Client configuration, sourced from environment variables with sane defaults.

use std::env;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL of the master HTTP API, e.g. `http://1.2.3.4:5001`.
    pub master_url: String,
    /// Optional shared secret matching the master's `PAX_API_KEY`.
    pub master_api_key: String,
    /// IB Gateway / TWS host for this client.
    pub ib_host: String,
    /// Live port (IB Gateway 4001 / TWS 7496).
    pub ib_port_live: u16,
    /// Paper port (IB Gateway 4002 / TWS 7497).
    pub ib_port_paper: u16,
    /// Seconds between reconciliation passes.
    pub sync_interval_secs: u64,
    /// After placing an order for a symbol, suppress further orders for it for this
    /// many seconds so fills can settle (prevents duplicate submissions).
    pub order_cooldown_secs: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            master_url: "http://127.0.0.1:5001".to_string(),
            master_api_key: String::new(),
            ib_host: "127.0.0.1".to_string(),
            ib_port_live: 4001,
            ib_port_paper: 4002,
            sync_interval_secs: 2,
            order_cooldown_secs: 10,
        }
    }
}

impl ClientConfig {
    pub fn from_env() -> Self {
        let d = ClientConfig::default();
        ClientConfig {
            master_url: env::var("PAX_MASTER_URL").unwrap_or(d.master_url),
            master_api_key: env::var("PAX_API_KEY").unwrap_or(d.master_api_key),
            ib_host: env::var("PAX_IB_HOST").unwrap_or(d.ib_host),
            ib_port_live: env::var("PAX_IB_PORT_LIVE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.ib_port_live),
            ib_port_paper: env::var("PAX_IB_PORT_PAPER")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.ib_port_paper),
            sync_interval_secs: env::var("PAX_SYNC_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.sync_interval_secs),
            order_cooldown_secs: env::var("PAX_ORDER_COOLDOWN_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.order_cooldown_secs),
        }
    }
}

/// Derive a stable, collision-resistant clientId (21..=999) from the hostname so two
/// client machines don't clash, while staying clear of master's clientId 0.
pub fn stable_client_id() -> i32 {
    let host = env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "pax-client".to_string());
    // FNV-1a 32-bit.
    let mut hash: u32 = 0x811c9dc5;
    for b in host.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    21 + (hash % 978) as i32
}
