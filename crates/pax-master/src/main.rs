//! Pax Americana — Master.
//!
//! A single standalone Windows executable: connects to the local IB Gateway/TWS, tracks
//! the master account's net positions + balance, broadcasts an authoritative snapshot
//! over HTTP for clients, and presents a native Win32 GUI that renders on any Windows
//! machine (including RDP/VPS — no OpenGL required).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod ib;
mod server;
mod state;
mod ui;

use crate::config::MasterConfig;
use crate::state::SharedState;

fn main() {
    // Kill switch on launch: clear any stale instances so they don't clog the port.
    ui::kill_other_instances();
    std::thread::sleep(std::time::Duration::from_millis(400));

    let cfg = MasterConfig::from_env();
    let state = SharedState::new(cfg.ib_host.clone(), cfg.ib_port_live, cfg.ib_port_paper, cfg.start_mode);

    server::spawn(cfg.http_bind.clone(), cfg.api_key.clone(), state.clone());
    let _stop = ib::spawn(cfg.clone(), state.clone());

    // Native GUI runs on the main thread and owns the lifetime — closing the window exits
    // the process cleanly (background workers are daemon threads).
    ui::run(state);
}
