//! Thin HTTP client for fetching the master's authoritative snapshot.

use std::time::Duration;

use pax_core::{MasterSnapshot, PROTOCOL_SCHEMA};

pub struct MasterApi {
    base: String,
    api_key: String,
}

impl MasterApi {
    pub fn new(base: impl Into<String>, api_key: impl Into<String>) -> Self {
        MasterApi {
            base: base.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    /// Fetch and validate the master snapshot. Returns an error string on failure so the
    /// engine can log and safely skip the cycle (never trading on bad data).
    pub fn snapshot(&self) -> Result<MasterSnapshot, String> {
        let url = format!("{}/snapshot", self.base);
        let mut req = ureq::get(&url).timeout(Duration::from_secs(6));
        if !self.api_key.is_empty() {
            req = req.set("X-API-Key", &self.api_key);
        }
        let resp = req.call().map_err(|e| format!("master unreachable: {e}"))?;
        let body = resp
            .into_string()
            .map_err(|e| format!("master read failed: {e}"))?;
        let snap = MasterSnapshot::from_json(&body).map_err(|e| format!("bad snapshot json: {e}"))?;
        if snap.schema != PROTOCOL_SCHEMA {
            return Err(format!(
                "protocol mismatch (master schema {}, client {})",
                snap.schema, PROTOCOL_SCHEMA
            ));
        }
        Ok(snap)
    }
}
