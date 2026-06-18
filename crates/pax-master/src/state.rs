//! Shared, thread-safe master state.

use std::sync::atomic::{AtomicU64, Ordering};
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
/// Which IBKR endpoint the master connects to. Toggleable at runtime in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbMode {
    Live,
    Paper,
}

/// GUI-editable IB connection parameters.
#[derive(Debug, Clone)]
pub struct ConnParams {
    pub host: String,
    pub port_live: u16,
    pub port_paper: u16,
    pub mode: IbMode,
}

impl ConnParams {
    pub fn port(&self) -> u16 {
        match self.mode {
            IbMode::Live => self.port_live,
            IbMode::Paper => self.port_paper,
        }
    }
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port())
    }
}

/// Non-invasive update-check status surfaced in the GUI.
#[derive(Default, Clone)]
pub struct UpdateStatus {
    pub message: String,
    pub available: bool,
    pub url: String,
}

pub struct SharedState {
    pub snapshot: Mutex<MasterSnapshot>,
    /// Pre-serialized JSON of `snapshot`, rebuilt once per publish by the (single) IB
    /// worker. HTTP handlers serve this directly so they never serialize per request —
    /// the snapshot lock is held only long enough to clone the Arc (a refcount bump),
    /// never during JSON encoding. Kept in lockstep with `snapshot` via `publish_snapshot`
    /// / `mark_offline` (only the IB worker writes either, so they can't diverge).
    pub snapshot_json: Mutex<Arc<String>>,
    pub log: Mutex<LogBuffer>,
    /// GUI-editable connection params; the IB worker reads them on (re)connect.
    pub conn: Mutex<ConnParams>,
    /// Bumped by the GUI to ask the worker to drop and reconnect with fresh params.
    pub reconnect_gen: AtomicU64,
    /// Update-check result (background, non-blocking).
    pub update: Mutex<UpdateStatus>,
}

impl SharedState {
    pub fn new(host: String, port_live: u16, port_paper: u16, start_mode: IbMode) -> Arc<Self> {
        let snap0 = MasterSnapshot::default();
        let json0 = Arc::new(snap0.to_json());
        Arc::new(SharedState {
            snapshot: Mutex::new(snap0),
            snapshot_json: Mutex::new(json0),
            log: Mutex::new(LogBuffer::default()),
            conn: Mutex::new(ConnParams {
                host,
                port_live,
                port_paper,
                mode: start_mode,
            }),
            reconnect_gen: AtomicU64::new(0),
            update: Mutex::new(UpdateStatus::default()),
        })
    }

    pub fn endpoint(&self) -> String {
        self.conn.lock().endpoint()
    }

    pub fn reconnect_gen(&self) -> u64 {
        self.reconnect_gen.load(Ordering::Relaxed)
    }

    /// Ask the IB worker to reconnect with the latest params.
    pub fn request_reconnect(&self) {
        self.reconnect_gen.fetch_add(1, Ordering::Relaxed);
    }

    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        self.log.lock().push(level, msg);
    }

    /// Publish a new snapshot AND its serialized JSON together. Serialization happens here
    /// (once per update, on the IB worker) rather than per HTTP request.
    pub fn publish_snapshot(&self, snap: MasterSnapshot) {
        let json = Arc::new(snap.to_json());
        *self.snapshot.lock() = snap;
        *self.snapshot_json.lock() = json;
    }

    /// Flip the feed to offline (connected=false) while keeping the last positions, and
    /// refresh the cached JSON so clients immediately read the standby state. Used during
    /// the position-replay window and on disconnect.
    pub fn mark_offline(&self) {
        let snap = {
            let mut s = self.snapshot.lock();
            s.connected = false;
            s.generated_at_ms = now_ms();
            s.clone()
        };
        *self.snapshot_json.lock() = Arc::new(snap.to_json());
    }

    /// Cheap read for HTTP handlers: clone the Arc (refcount bump), no serialization.
    pub fn snapshot_json(&self) -> Arc<String> {
        self.snapshot_json.lock().clone()
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
