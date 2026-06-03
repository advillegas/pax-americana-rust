//! Lightweight, non-invasive update check against GitHub Releases.
//!
//! Queries the repo's latest published release and reports whether it's newer than the
//! running build. Never blocks the app (callers run it on a background thread); any
//! network/parse failure simply yields "no update".

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// Latest version (without a leading `v`).
    pub version: String,
    /// Page to open so the user can download it (the release page).
    pub url: String,
}

/// Check `repo` (e.g. "owner/name") for a release newer than `current`. Returns `None`
/// on no-update or any failure (offline, private repo, no releases, etc.).
pub fn check(repo: &str, current: &str) -> Option<UpdateInfo> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = ureq::get(&url)
        .set("User-Agent", "pax-americana")
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(8))
        .call()
        .ok()?;
    let body = resp.into_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = v.get("tag_name")?.as_str()?.to_string();
    let page = v
        .get("html_url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if version_gt(&tag, current) {
        Some(UpdateInfo {
            version: tag.trim_start_matches('v').to_string(),
            url: page,
        })
    } else {
        None
    }
}

fn version_gt(a: &str, b: &str) -> bool {
    parse(a) > parse(b)
}

fn parse(v: &str) -> (u64, u64, u64) {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(version_gt("2.1.0", "2.0.0"));
        assert!(version_gt("v2.0.1", "2.0.0"));
        assert!(version_gt("3.0.0", "2.9.9"));
        assert!(!version_gt("2.0.0", "2.0.0"));
        assert!(!version_gt("1.9.9", "2.0.0"));
    }
}
