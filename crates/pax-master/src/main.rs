//! Pax Americana — Master.
//!
//! Connects to the local IB Gateway/TWS, tracks the master account's net positions and
//! balance, and broadcasts an authoritative snapshot over HTTP for clients to reconcile
//! against. Ships with a themed status GUI.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod ib;
mod server;
mod state;
mod ui;

use eframe::egui;

use crate::config::MasterConfig;
use crate::state::SharedState;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = MasterConfig::from_env();
    let state = SharedState::new(
        cfg.ib_host.clone(),
        cfg.ib_port_live,
        cfg.ib_port_paper,
        cfg.http_bind.clone(),
        cfg.start_mode,
    );

    // Background workers.
    server::spawn(cfg.http_bind.clone(), cfg.api_key.clone(), state.clone());
    let _stop = ib::spawn(cfg.clone(), state.clone());

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 680.0])
            .with_min_inner_size([520.0, 360.0])
            .with_title("Pax Americana — Master"),
        ..Default::default()
    };

    let ui_state = state.clone();
    eframe::run_native(
        "Pax Americana — Master",
        native_options,
        Box::new(move |cc| Ok(Box::new(ui::MasterApp::new(cc, ui_state)))),
    )
}
