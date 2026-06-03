//! Pax Americana — Client.
//!
//! A single standalone Windows executable: connects to this machine's IB Gateway/TWS,
//! polls the master snapshot, and reconciles the local book to a proportionally-scaled
//! copy of the master's structure (positions + resting limit/stop orders). Direction is
//! always derived from the position delta — never from a raw action — so a master close
//! can never open a client short. Native Win32 GUI renders on any Windows machine.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod ib;
mod master_api;
mod state;
mod ui;

use crate::config::ClientConfig;
use crate::state::{LogLevel, SharedState};

fn main() {
    // Kill switch on launch: clear stale instances.
    ui::kill_other_instances();
    std::thread::sleep(std::time::Duration::from_millis(400));

    let cfg = ClientConfig::from_env();
    let state = SharedState::new();
    {
        let mut c = state.controls.lock();
        c.ib_host = cfg.ib_host.clone();
        c.ib_port_live = cfg.ib_port_live;
        c.ib_port_paper = cfg.ib_port_paper;
    }
    state.log(LogLevel::Info, format!("Pax Americana ready. Master: {}", cfg.master_url));

    engine::spawn(cfg.clone(), state.clone());

    ui::run(state);
}
