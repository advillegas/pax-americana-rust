//! Shared, thread-safe master state.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use pax_core::MasterSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Ok,
    Warn,
    Err,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub ts: String,
    pub level: LogLevel,
    pub msg: String,
}

/// Bounded in-memory log shared with the GUI.
#[derive(Default)]
pub struct LogBuffer {
    lines: Vec<LogLine>,
}

impl LogBuffer {
    const CAP: usize = 500;

    pub fn push(&mut self, level: LogLevel, msg: impl Into<String>) {
        let ts = now_hms();
        self.lines.push(LogLine {
            ts,
            level,
            msg: msg.into(),
        });
        if self.lines.len() > Self::CAP {
            let overflow = self.lines.len() - Self::CAP;
            self.lines.drain(0..overflow);
        }
    }

    pub fn lines(&self) -> &[LogLine] {
        &self.lines
    }
}

/// Everything the GUI, IB worker, and HTTP server share.
pub struct SharedState {
    pub snapshot: Mutex<MasterSnapshot>,
    pub log: Mutex<LogBuffer>,
    pub endpoint: String,
    pub http_bind: String,
}

impl SharedState {
    pub fn new(endpoint: String, http_bind: String) -> Arc<Self> {
        Arc::new(SharedState {
            snapshot: Mutex::new(MasterSnapshot::default()),
            log: Mutex::new(LogBuffer::default()),
            endpoint,
            http_bind,
        })
    }

    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        self.log.lock().push(level, msg);
    }
}

pub fn now_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
