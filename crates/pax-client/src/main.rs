//! Pax Americana — Client.
//!
//! A single standalone Windows executable: connects to this machine's IB Gateway/TWS,
//! polls the master snapshot, and reconciles the local book to a proportionally-scaled
//! copy of the master's structure (positions + resting limit/stop orders). Direction is
//! always derived from the position delta — never from a raw action — so a master close
//! can never open a client short. The themed Slint GUI renders in software (no OpenGL),
//! so it works on any Windows machine including RDP/VPS.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod ib;
mod master_api;
mod state;

use std::os::windows::process::CommandExt;
use std::time::Duration;

use crate::config::ClientConfig;
use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

slint::include_modules!();

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    std::env::set_var("SLINT_BACKEND", "winit-software");

    kill_other_instances();
    std::thread::sleep(Duration::from_millis(1500));

    let cfg = ClientConfig::from_env();
    let state = SharedState::new();
    {
        let mut c = state.controls.lock();
        c.ib_host = cfg.ib_host.clone();
        c.ib_port_live = cfg.ib_port_live;
        c.ib_port_paper = cfg.ib_port_paper;
        c.master_url = cfg.master_url.clone();
    }
    state.log(LogLevel::Info, format!("Pax Americana ready. Master: {}", cfg.master_url));

    engine::spawn(cfg.clone(), state.clone());

    let ui = ClientWindow::new().expect("failed to create window");

    {
        let c = state.controls.lock();
        ui.set_account_mode(if c.account_mode == AccountMode::Live { 0 } else { 1 });
        ui.set_trade_mode(if c.trade_mode == TradeMode::LongOnly { 1 } else { 0 });
        ui.set_exec_mode(if c.execution_mode == ExecutionMode::NewOnly { 1 } else { 0 });
        ui.set_multiplier(format!("{:.1}", c.multiplier).into());
        ui.set_drawdown(format!("{:.1}", c.max_drawdown_pct).into());
        ui.set_max_notional(format!("{:.0}", c.max_position_notional).into());
        ui.set_max_qty(format!("{:.0}", c.max_position_qty).into());
        ui.set_host(c.ib_host.clone().into());
        ui.set_live_port(c.ib_port_live.to_string().into());
        ui.set_paper_port(c.ib_port_paper.to_string().into());
        ui.set_master_url(c.master_url.clone().into());
    }

    {
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_save(move || {
            if let Some(ui) = w.upgrade() {
                apply_settings(&ui, &state);
                state.log(LogLevel::Info, "Settings saved.");
            }
        });
    }
    {
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_start(move || {
            if let Some(ui) = w.upgrade() {
                apply_settings(&ui, &state);
            }
            state.start_engine();
        });
    }
    {
        let state = state.clone();
        ui.on_stop(move || state.stop_engine());
    }
    {
        let state = state.clone();
        ui.on_close_all(move || {
            state.request_close_all();
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
            let s = state.status.lock().clone();
            let running = state.is_running();
            ui.set_connected(s.connected && !s.drawdown_hit);
            ui.set_running(running);
            ui.set_status_text(
                if s.drawdown_hit {
                    "⚠ DRAWDOWN HALT".to_string()
                } else if s.connected {
                    "● CONNECTED - syncing".to_string()
                } else if running {
                    "... connecting".to_string()
                } else {
                    "■ STOPPED".to_string()
                }
                .into(),
            );
            ui.set_balance_text(format!("Net Liquidation: {}", money(s.client_balance)).into());
            ui.set_master_text(format!("Master: {}", money(s.master_balance)).into());
            ui.set_counts_text(
                format!(
                    "Positions  M·C {}·{}     Opened {}   Closed {}   Failed {}",
                    s.master_positions, s.client_positions, s.orders_placed, s.orders_closed, s.orders_failed
                )
                .into(),
            );
            ui.set_margin_blocked(s.margin_blocks_opens);
            ui.set_margin_text(
                format!(
                    "Cushion {:.0}%   Excess {}   SMA {}{}",
                    s.cushion * 100.0,
                    money(s.excess_liquidity),
                    money(s.sma),
                    if s.margin_blocks_opens { "   — SMA CALL: opens blocked" } else { "" }
                )
                .into(),
            );
            ui.set_log_text(recent_log(&state).into());
        });
    }

    ui.run().expect("failed to run GUI");
}

fn apply_settings(ui: &ClientWindow, state: &SharedState) {
    let mut c = state.controls.lock();
    c.account_mode = if ui.get_account_mode() == 0 { AccountMode::Live } else { AccountMode::Paper };
    c.trade_mode = if ui.get_trade_mode() == 1 { TradeMode::LongOnly } else { TradeMode::LongShort };
    c.execution_mode = if ui.get_exec_mode() == 1 { ExecutionMode::NewOnly } else { ExecutionMode::ExistingPlusNew };
    if let Ok(v) = ui.get_multiplier().trim().parse::<f64>() {
        c.multiplier = v.clamp(0.1, 5.0);
    }
    if let Ok(v) = ui.get_drawdown().trim().parse::<f64>() {
        c.max_drawdown_pct = v.clamp(1.0, 50.0);
    }
    if let Ok(v) = ui.get_max_notional().trim().parse::<f64>() {
        c.max_position_notional = v.max(0.0);
    }
    if let Ok(v) = ui.get_max_qty().trim().parse::<f64>() {
        c.max_position_qty = v.max(0.0);
    }
    let host = ui.get_host().trim().to_string();
    if !host.is_empty() {
        c.ib_host = host;
    }
    if let Ok(v) = ui.get_live_port().trim().parse::<u16>() {
        c.ib_port_live = v;
    }
    if let Ok(v) = ui.get_paper_port().trim().parse::<u16>() {
        c.ib_port_paper = v;
    }
    let url = ui.get_master_url().trim().to_string();
    if !url.is_empty() {
        c.master_url = url;
    }
}

fn recent_log(state: &SharedState) -> String {
    let log = state.log.lock();
    let lines = log.lines();
    let start = lines.len().saturating_sub(250);
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
        LogLevel::Buy => "BUY ",
        LogLevel::Sell => "SELL",
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

pub fn kill_other_instances() {
    let pid = std::process::id();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "pax-client.exe".to_string());
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", &exe, "/FI", &format!("PID ne {pid}")])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}
