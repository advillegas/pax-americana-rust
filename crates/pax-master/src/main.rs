//! Pax Americana — Master.
//!
//! Connects to the local IB Gateway/TWS, tracks the master account's net positions and
//! balance, and broadcasts an authoritative snapshot over HTTP for clients to reconcile
//! against.
//!
//! The master is a server daemon: it runs with a console so logs/errors are visible, and
//! the monitoring GUI is optional. On a headless or RDP server (no OpenGL), or with
//! `PAX_HEADLESS=1` / `--headless`, it runs without the GUI and keeps serving.

mod config;
mod ib;
mod server;
mod state;
mod ui;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use eframe::egui;

use crate::config::MasterConfig;
use crate::state::{LogLevel, SharedState};

fn main() {
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

    // Background workers run regardless of whether the GUI comes up.
    server::spawn(cfg.http_bind.clone(), cfg.api_key.clone(), state.clone());
    let _stop = ib::spawn(cfg.clone(), state.clone());

    let headless = std::env::var("PAX_HEADLESS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || std::env::args().any(|a| a == "--headless");

    if headless {
        run_headless(&state);
    }

    // Try the GUI; if it can't initialise (e.g. no OpenGL over RDP) fall back to headless.
    println!("Starting Pax Americana - Master. GUI will open if a display is available.");
    let gui_state = state.clone();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| run_gui(gui_state)));
    match result {
        Ok(Ok(())) => {
            // GUI window closed by the operator — shut down.
        }
        Ok(Err(e)) => {
            eprintln!("GUI could not start ({e}). Continuing headless - set PAX_HEADLESS=1 to skip the GUI.");
            state.log(LogLevel::Warn, format!("GUI unavailable ({e}) - running headless."));
            run_headless(&state);
        }
        Err(_) => {
            eprintln!("GUI crashed during init (likely no OpenGL on this server/RDP session). Continuing headless - set PAX_HEADLESS=1 to skip the GUI.");
            state.log(LogLevel::Warn, "GUI crashed (no OpenGL?) - running headless.");
            run_headless(&state);
        }
    }
}

fn run_gui(state: Arc<SharedState>) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 720.0])
            .with_min_inner_size([520.0, 420.0])
            .with_title("Pax Americana — Master"),
        ..Default::default()
    };
    eframe::run_native(
        "Pax Americana — Master",
        native_options,
        Box::new(move |cc| Ok(Box::new(ui::MasterApp::new(cc, state)))),
    )
}

/// Run forever without a GUI: the IB worker and HTTP server (already spawned) keep
/// serving. Log output goes to the console.
fn run_headless(state: &Arc<SharedState>) -> ! {
    state.log(LogLevel::Ok, "Master running headless - IB worker + HTTP API active.");
    println!("Pax Americana - Master is running headless. Press Ctrl+C to stop.");
    loop {
        std::thread::park();
    }
}
