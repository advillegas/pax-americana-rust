//! Pax Americana — Client.
//!
//! Connects to this machine's IB Gateway/TWS, polls the master snapshot, and reconciles
//! the local book to a proportionally-scaled copy of the master's structure. Orphans are
//! closed, missing positions opened, and direction is always derived from the position
//! delta — never from a raw action — so a master close can never open a client short.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod engine;
mod ib;
mod master_api;
mod state;
mod ui;

use eframe::egui;

use crate::config::ClientConfig;
use crate::state::SharedState;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = ClientConfig::from_env();
    let state = SharedState::new();
    state.log(
        state::LogLevel::Info,
        format!("Pax Americana ready. Master: {}", cfg.master_url),
    );

    engine::spawn(cfg.clone(), state.clone());

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 820.0])
            .with_min_inner_size([560.0, 520.0])
            .with_title("Pax Americana — Client"),
        ..Default::default()
    };

    let ui_state = state.clone();
    eframe::run_native(
        "Pax Americana — Client",
        native_options,
        Box::new(move |cc| Ok(Box::new(ui::ClientApp::new(cc, ui_state)))),
    )
}
