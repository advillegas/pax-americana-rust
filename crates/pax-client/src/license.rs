//! License gate. The client verifies its IBKR account against a licensing endpoint
//! before trading, and re-checks periodically. The endpoint is operator-controlled.

use std::time::Duration;

const LICENSE_URL: &str = "https://portal.neroai.com/api/licenses/equities";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseStatus {
    /// Account is in the licensed set.
    Authorized,
    /// Endpoint responded, but this account is not licensed.
    Denied,
    /// Could not reach/parse the endpoint (network/format) — verification inconclusive.
    Unknown,
}

/// Check whether `account` is licensed. Accepts a JSON body shaped as `{"licenses": [...]}`
/// or a bare array; entries may be strings or objects with a `key`/`account` field.
pub fn check(account: &str) -> LicenseStatus {
    let resp = match ureq::get(LICENSE_URL).timeout(Duration::from_secs(8)).call() {
        Ok(r) => r,
        Err(_) => return LicenseStatus::Unknown,
    };
    let body = match resp.into_string() {
        Ok(b) => b,
        Err(_) => return LicenseStatus::Unknown,
    };
    let v: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return LicenseStatus::Unknown,
    };

    let arr = v.get("licenses").and_then(|x| x.as_array()).or_else(|| v.as_array());
    let Some(arr) = arr else {
        return LicenseStatus::Denied;
    };
    for e in arr {
        let key = e
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| e.get("key").and_then(|x| x.as_str()).map(|s| s.to_string()))
            .or_else(|| e.get("account").and_then(|x| x.as_str()).map(|s| s.to_string()))
            .unwrap_or_default();
        if key == account {
            return LicenseStatus::Authorized;
        }
    }
    LicenseStatus::Denied
}
