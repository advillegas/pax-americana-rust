//! Non-invasive, self-applying update check.
//!
//! Checks the latest release for a newer build and (on demand) downloads the new
//! executable and swaps it in via a small relaunch script. The update source is never
//! surfaced to the user — callers only show generic "update available / updating" text —
//! so clients have no visibility into how updates are delivered.

use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct UpdateInfo {
    /// Latest version (without a leading `v`).
    pub version: String,
    /// Direct download URL for this app's executable asset. Kept internal — never shown.
    pub asset_url: String,
}

/// Check `repo` for a release newer than `current`. `asset_hint` selects which release
/// asset matches this app (e.g. "client" or "master"). Returns `None` on no-update or
/// any failure.
pub fn check(repo: &str, current: &str, asset_hint: &str) -> Option<UpdateInfo> {
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
    if !version_gt(&tag, current) {
        return None;
    }
    let mut asset_url = String::new();
    if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
        let hint = asset_hint.to_lowercase();
        for a in assets {
            let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
            if name.contains(&hint) && name.ends_with(".exe") {
                asset_url = a
                    .get("browser_download_url")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                break;
            }
        }
    }
    Some(UpdateInfo {
        version: tag.trim_start_matches('v').to_string(),
        asset_url,
    })
}

/// Download the new executable and apply it: write it beside the running exe, then spawn
/// a detached script that waits for this process to exit, swaps the file in, and
/// relaunches. The caller should exit shortly after this returns `Ok`.
#[cfg(windows)]
pub fn download_and_apply(asset_url: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if asset_url.is_empty() {
        return Err("no update package available".to_string());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no exe directory")?.to_path_buf();
    let file_name = exe.file_name().ok_or("no exe name")?.to_string_lossy().into_owned();
    let new_path = dir.join(format!("{file_name}.new"));

    let resp = ureq::get(asset_url)
        .set("User-Agent", "pax-americana")
        .timeout(Duration::from_secs(180))
        .call()
        .map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(&new_path).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut f).map_err(|e| e.to_string())?;
    f.flush().ok();
    drop(f);

    let bat = dir.join("_pax_apply_update.bat");
    let script = format!(
        "@echo off\r\nping 127.0.0.1 -n 3 >nul\r\nmove /y \"{new}\" \"{exe}\" >nul\r\nstart \"\" \"{exe}\"\r\ndel \"%~f0\"\r\n",
        new = new_path.display(),
        exe = exe.display(),
    );
    std::fs::write(&bat, script).map_err(|e| e.to_string())?;

    std::process::Command::new("cmd")
        .args(["/C", "start", "", "/min", "cmd", "/C", &bat.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
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
