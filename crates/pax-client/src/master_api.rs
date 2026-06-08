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
        let resp = req
            .call()
            .map_err(|e| format!("unreachable ({})", scrub_url(&e.to_string(), &self.base)))?;
        let body = resp
            .into_string()
            .map_err(|e| format!("read failed: {e}"))?;
        let snap = MasterSnapshot::from_json(&body).map_err(|e| format!("bad data: {e}"))?;
        // (scrub_url keeps the address out of user-facing logs)
        if snap.schema != PROTOCOL_SCHEMA {
            return Err(format!(
                "version mismatch (remote {}, local {})",
                snap.schema, PROTOCOL_SCHEMA
            ));
        }
        Ok(snap)
    }
}

/// Remove the server address from an error string so it never lands in the visible log.
fn scrub_url(msg: &str, base: &str) -> String {
    let mut s = msg.replace(base, "host");
    // ureq prefixes transport errors with the full request URL; drop everything up to and
    // including the "/snapshot: " marker, leaving only the human-readable reason.
    if let Some(idx) = s.find("/snapshot") {
        if let Some(colon) = s[idx..].find(": ") {
            s = s[idx + colon + 2..].to_string();
        }
    }
    let s = s.trim();
    if s.is_empty() {
        "no response".to_string()
    } else {
        s.to_string()
    }
}
