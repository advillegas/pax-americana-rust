//! Pax Americana — Client.
//!
//! Connects to this machine's IB Gateway/TWS, polls the master snapshot, and reconciles
//! the local book to a proportionally-scaled copy of the master's structure (positions +
//! resting limit/stop orders). Direction is always derived from the position delta — never
//! from a raw action — so a master close can never open a client short.
//!
//! Runs as a console app with an optional native GUI. On a headless/VPS/RDP machine with
//! no OpenGL it falls back to headless and is fully operable via the browser control panel.

mod config;
mod dashboard;
mod engine;
mod ib;
mod master_api;
mod panel;
mod state;
mod ui;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use eframe::egui;

use crate::config::ClientConfig;
use crate::state::{LogLevel, SharedState};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

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
    panel::spawn(cfg.panel_bind.clone(), cfg.panel_key.clone(), state.clone());

    let headless = std::env::var("PAX_HEADLESS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || std::env::args().any(|a| a == "--headless");

    if headless {
        run_headless(&state);
    }

    println!("Starting Pax Americana - Client. GUI will open if a display is available; otherwise use the web panel.");
    let gui_state = state.clone();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| run_gui(gui_state)));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            eprintln!("GUI could not start ({e}). Continuing headless - control via the web panel. Set PAX_HEADLESS=1 to skip the GUI.");
            state.log(LogLevel::Warn, format!("GUI unavailable ({e}) - use the web panel."));
            run_headless(&state);
        }
        Err(_) => {
            eprintln!("GUI crashed during init (likely no OpenGL on this server/RDP session). Continuing headless - control via the web panel.");
            state.log(LogLevel::Warn, "GUI crashed (no OpenGL?) - use the web panel.");
            run_headless(&state);
        }
    }
}

fn run_gui(state: Arc<SharedState>) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 820.0])
            .with_min_inner_size([560.0, 520.0])
            .with_title("Pax Americana — Client"),
        ..Default::default()
    };
    eframe::run_native(
        "Pax Americana — Client",
        native_options,
        Box::new(move |cc| Ok(Box::new(ui::ClientApp::new(cc, state)))),
    )
}

fn run_headless(state: &Arc<SharedState>) -> ! {
    state.log(LogLevel::Ok, "Client running headless - control via the web panel.");
    println!("Pax Americana - Client running headless. Open the web panel in a browser. Ctrl+C to stop.");
    loop {
        std::thread::park();
    }
}
