//! Pax Americana — Master.
//!
//! A single standalone Windows executable: connects to the local IB Gateway/TWS, tracks
//! the master account's net positions + balance, broadcasts an authoritative snapshot
//! over HTTP for clients, and shows a themed Slint GUI. Slint's software renderer draws
//! on the CPU (no OpenGL), so the window renders on any Windows machine, including
//! RDP/VPS sessions.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod ib;
mod server;
mod state;

use std::os::windows::process::CommandExt;
use std::time::Duration;

use crate::config::MasterConfig;
use crate::state::{IbMode, LogLevel, SharedState};

slint::include_modules!();

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    // CPU/software rendering — works without a GPU/OpenGL (RDP/VPS friendly).
    std::env::set_var("SLINT_BACKEND", "winit-software");

    // Kill switch on launch: clear stale instances so they don't clog the port or hold
    // the TWS clientId. Wait long enough for the OS to release the socket + IB session.
    kill_other_instances();
    std::thread::sleep(Duration::from_millis(1500));

    let cfg = MasterConfig::from_env();
    let state = SharedState::new(cfg.ib_host.clone(), cfg.ib_port_live, cfg.ib_port_paper, cfg.start_mode);

    server::spawn(cfg.http_bind.clone(), cfg.api_key.clone(), state.clone());
    let _stop = ib::spawn(cfg.clone(), state.clone());

    let ui = MasterWindow::new().expect("failed to create window");

    {
        let conn = state.conn.lock();
        ui.set_host(conn.host.clone().into());
        ui.set_live_port(conn.port_live.to_string().into());
        ui.set_paper_port(conn.port_paper.to_string().into());
        ui.set_mode(if conn.mode == IbMode::Live { 0 } else { 1 });
    }

    {
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_apply(move || {
            let Some(ui) = w.upgrade() else { return };
            let host = ui.get_host().to_string();
            let live = ui.get_live_port().to_string();
            let paper = ui.get_paper_port().to_string();
            let mode = ui.get_mode();
            {
                let mut conn = state.conn.lock();
                if !host.trim().is_empty() {
                    conn.host = host.trim().to_string();
                }
                if let Ok(p) = live.trim().parse::<u16>() {
                    conn.port_live = p;
                }
                if let Ok(p) = paper.trim().parse::<u16>() {
                    conn.port_paper = p;
                }
                conn.mode = if mode == 0 { IbMode::Live } else { IbMode::Paper };
            }
            state.request_reconnect();
            state.log(LogLevel::Warn, format!("Reconnecting to {}", state.endpoint()));
        });
    }

    {
        let state = state.clone();
        ui.on_kill(move || {
            kill_other_instances();
            state.log(LogLevel::Warn, "Kill switch: terminated other instances.");
        });
    }

    let timer = slint::Timer::default();
    {
        let state = state.clone();
        let w = ui.as_weak();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(ui) = w.upgrade() else { return };
            let (connected, account, balance, npos) = {
                let s = state.snapshot.lock();
                (s.connected, s.account.clone(), s.balance, s.positions.len())
            };
            ui.set_connected(connected);
            ui.set_status_text(
                if connected { "● CONNECTED - broadcasting" } else { "✕ DISCONNECTED - connecting..." }.into(),
            );
            ui.set_account_text(format!("Account: {}", if account.is_empty() { "-" } else { &account }).into());
            ui.set_balance_text(format!("Net Liquidation: {}", money(balance)).into());
            ui.set_positions_text(format!("Positions: {npos}").into());
            ui.set_log_text(recent_log(&state).into());
        });
    }

    ui.run().expect("failed to run GUI");
}

fn recent_log(state: &SharedState) -> String {
    let log = state.log.lock();
    let lines = log.lines();
    let start = lines.len().saturating_sub(200);
    lines[start..]
        .iter()
        .map(|l| format!("[{}] {} {}\n", l.ts, tag(l.level), l.msg))
        .collect()
}

fn tag(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Ok => "OK  ",
        LogLevel::Warn => "WARN",
        LogLevel::Err => "ERR ",
        LogLevel::Info => "INFO",
    }
}

fn money(v: f64) -> String {
    let neg = v < 0.0;
    let whole = v.abs().trunc() as u64;
    let cents = (v.abs().fract() * 100.0).round() as u64;
    let mut s = whole.to_string();
    let mut grouped = String::new();
    while s.len() > 3 {
        let split = s.len() - 3;
        grouped = format!(",{}{}", &s[split..], grouped);
        s.truncate(split);
    }
    grouped = format!("{s}{grouped}");
    format!("{}${}.{:02}", if neg { "-" } else { "" }, grouped, cents)
}

/// Kill switch: terminate every other instance of this executable (frees clogged ports).
pub fn kill_other_instances() {
    let pid = std::process::id();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "pax-master.exe".to_string());
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", &exe, "/FI", &format!("PID ne {pid}")])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}
