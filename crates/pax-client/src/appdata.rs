//! Hidden, obfuscated application-data storage shared by settings and the ledger.
//!
//! Files live under `%LOCALAPPDATA%\NeroAI\cache\` (hidden), with XOR-obfuscated contents
//! so they aren't plainly readable or openly sitting beside the executable. This is
//! obfuscation, not encryption — it deters casual viewing/tampering, nothing more.

use std::fs;
use std::path::{Path, PathBuf};

const OBFS_KEY: &[u8] = b"px-amrcn-2026-ledger-obfuscation-key";

/// Hidden app-data directory, created (and hidden) on first use.
fn dir() -> Option<PathBuf> {
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .or_else(|| std::env::var("APPDATA").ok())
        .or_else(|| std::env::var("USERPROFILE").ok())?;
    let d = PathBuf::from(base).join("NeroAI").join("cache");
    let existed = d.exists();
    fs::create_dir_all(&d).ok()?;
    if !existed {
        hide(&d);
    }
    Some(d)
}

fn file(name: &str) -> Option<PathBuf> {
    Some(dir()?.join(name))
}

/// Set the Windows FILE_ATTRIBUTE_HIDDEN flag via the Win32 API directly.
/// No subprocess spawning (avoids attrib.exe errors on low-memory systems).
fn hide(p: &Path) {
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn GetFileAttributesW(lpFileName: *const u16) -> u32;
        fn SetFileAttributesW(lpFileName: *const u16, dwFileAttributes: u32) -> i32;
    }

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const INVALID: u32 = 0xFFFF_FFFF;

    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs != INVALID {
            SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN);
        }
    }
}

/// Reversible, non-cryptographic byte obfuscation so files aren't human-readable.
fn obfuscate(data: &mut [u8]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= OBFS_KEY[i % OBFS_KEY.len()];
    }
}

/// Read and de-obfuscate a hidden file, or `None` if it's missing/unreadable.
pub fn read(name: &str) -> Option<Vec<u8>> {
    let p = file(name)?;
    let mut bytes = fs::read(&p).ok()?;
    obfuscate(&mut bytes);
    Some(bytes)
}

/// Obfuscate and write a hidden file (best-effort), setting the hidden attribute.
pub fn write(name: &str, mut bytes: Vec<u8>) {
    let Some(p) = file(name) else { return };
    obfuscate(&mut bytes);
    if fs::write(&p, bytes).is_ok() {
        hide(&p);
    }
}

/// Path of a legacy plaintext file beside the executable (for one-time migration).
pub fn legacy(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(name))
}
