//! Pax Americana — Client.
//!
//! A single standalone Windows executable: connects to this machine's IB Gateway/TWS,
//! polls the master snapshot, and reconciles the local book to a proportionally-scaled
//! copy of the master's structure (positions + resting limit/stop orders). Direction is
//! always derived from the position delta — never from a raw action — so a master close
//! can never open a client short. The themed Slint GUI renders in software (no OpenGL),
//! so it works on any Windows machine including RDP/VPS.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alert;
mod appdata;
mod config;
mod data;
mod engine;
mod ib;
mod ledger;
mod license;
mod market_hours;
mod master_api;
mod persist;
mod state;

use std::os::windows::process::CommandExt;
use std::time::Duration;

use crate::config::ClientConfig;
use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

slint::include_modules!();

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    init_opaque_software_backend();

    kill_other_instances();
    std::thread::sleep(Duration::from_millis(1500));

    let cfg = ClientConfig::from_env();
    let state = SharedState::new();
    {
        let mut c = state.controls.lock();
        c.ib_host = cfg.ib_host.clone();
        c.ib_port_live = cfg.ib_port_live;
        c.ib_port_paper = cfg.ib_port_paper;
        c.ib_account = cfg.ib_account.clone();
        c.master_url = cfg.master_url.clone();
        // Saved settings (if any) override the env/config defaults.
        persist::load_into(&mut c);
    }
    state.log(LogLevel::Info, "Pax Americana ready.".to_string());

    engine::spawn(cfg.clone(), state.clone());
    data::spawn(cfg.clone(), state.clone());
    alert::spawn(state.clone());
    spawn_update_check(state.clone());
    spawn_detect_accounts(state.clone());

    let ui = ClientWindow::new().expect("failed to create window");

    {
        let c = state.controls.lock();
        ui.set_account_mode(if c.account_mode == AccountMode::Live { 0 } else { 1 });
        ui.set_trade_mode(if c.trade_mode == TradeMode::LongOnly { 1 } else { 0 });
        ui.set_exec_mode(if c.execution_mode == ExecutionMode::NewOnly { 1 } else { 0 });
        ui.set_hours_mode(if c.rth_only { 1 } else { 0 });
        ui.set_multiplier(format!("{:.1}", c.multiplier).into());
        ui.set_drawdown(format!("{:.1}", c.max_drawdown_pct).into());
        ui.set_host(c.ib_host.clone().into());
        ui.set_live_port(c.ib_port_live.to_string().into());
        ui.set_paper_port(c.ib_port_paper.to_string().into());
        ui.set_account(c.ib_account.clone().into());
        ui.set_alerts_enabled(c.alerts_enabled);
        ui.set_alert_email(c.alert_email.clone().into());
        ui.set_alert_hours(format!("{:.1}", c.alert_after_hours).into());
        ui.set_smtp_host(c.smtp_host.clone().into());
        ui.set_smtp_port(c.smtp_port.to_string().into());
        ui.set_smtp_user(c.smtp_user.clone().into());
        ui.set_smtp_pass(c.smtp_pass.clone().into());
        ui.set_smtp_from(c.smtp_from.clone().into());
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
    {
        let state = state.clone();
        ui.on_check_update(move || spawn_update_check(state.clone()));
    }
    {
        let state = state.clone();
        ui.on_detect_accounts(move || spawn_detect_accounts(state.clone()));
    }
    {
        let state = state.clone();
        ui.on_download_update(move || spawn_self_update(state.clone()));
    }
    {
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_load_chart(move || {
            if let Some(ui) = w.upgrade() {
                *state.chart_symbol.lock() = ui.get_chart_symbol().trim().to_uppercase();
                state.chart_tf.store(ui.get_chart_tf() as u8, std::sync::atomic::Ordering::Relaxed);
                state.chart_request.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    {
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_view_symbol(move |sym| {
            if let Some(ui) = w.upgrade() {
                ui.set_chart_symbol(sym.clone());
                ui.set_active_tab(2);
                *state.chart_symbol.lock() = sym.to_string();
                state.chart_request.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    // ── Chart pan / scroll / zoom (re-window the stored bars, no IB round-trip) ──
    {
        // Capture the current pan offset at the start of a drag.
        let state = state.clone();
        ui.on_chart_drag_begin(move || {
            let start = state.chart_start.load(std::sync::atomic::Ordering::Relaxed);
            state.chart_anchor.store(start, std::sync::atomic::Ordering::Relaxed);
        });
    }
    {
        // While dragging, translate the pixel delta into a bar offset from the anchor.
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_chart_drag_to(move |delta_px, width_px| {
            let Some(ui) = w.upgrade() else { return };
            use std::sync::atomic::Ordering::Relaxed;
            let len = state.chart_bars.lock().len() as i64;
            let count = state.chart_count.load(Relaxed) as i64;
            if len == 0 || count == 0 || width_px <= 0.0 {
                return;
            }
            let bar_px = (width_px / count as f32).max(0.001);
            let bars = (delta_px / bar_px).round() as i64;
            let anchor = state.chart_anchor.load(Relaxed) as i64;
            // Drag right (positive delta) reveals older bars → window moves earlier.
            let new_start = (anchor - bars).clamp(0, (len - count).max(0)) as usize;
            state.chart_start.store(new_start, Relaxed);
            data::rerender(&state);
            push_chart(&ui, &state);
        });
    }
    {
        // Scroll the wheel to slide through time: forward (up) or back (down).
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_chart_scroll(move |forward| {
            let Some(ui) = w.upgrade() else { return };
            use std::sync::atomic::Ordering::Relaxed;
            let len = state.chart_bars.lock().len() as i64;
            let count = state.chart_count.load(Relaxed) as i64;
            if len == 0 {
                return;
            }
            let step = (count / 6).max(1);
            let start = state.chart_start.load(Relaxed) as i64;
            let delta = if forward { step } else { -step };
            let new_start = (start + delta).clamp(0, (len - count).max(0)) as usize;
            state.chart_start.store(new_start, Relaxed);
            data::rerender(&state);
            push_chart(&ui, &state);
        });
    }
    {
        // '=' zooms in (fewer bars), '-' zooms out (more bars), anchored to the right edge.
        let state = state.clone();
        let w = ui.as_weak();
        ui.on_chart_zoom(move |zoom_in| {
            let Some(ui) = w.upgrade() else { return };
            use std::sync::atomic::Ordering::Relaxed;
            let len = state.chart_bars.lock().len();
            if len == 0 {
                return;
            }
            let count = state.chart_count.load(Relaxed);
            let start = state.chart_start.load(Relaxed);
            let end = start + count; // keep the right edge fixed while zooming
            let new_count = if zoom_in {
                (count * 4 / 5).max(8).min(len)
            } else {
                ((count * 5 / 4) + 1).min(len)
            };
            let new_start = end.saturating_sub(new_count).min(len - new_count);
            state.chart_count.store(new_count, Relaxed);
            state.chart_start.store(new_start, Relaxed);
            data::rerender(&state);
            push_chart(&ui, &state);
        });
    }

    {
        let state = state.clone();
        ui.on_test_alert(move || {
            state.alert_test.store(true, std::sync::atomic::Ordering::Relaxed);
            *state.alert_status.lock() = "Sending test…".to_string();
        });
    }

    let timer = slint::Timer::default();
    {
        let state = state.clone();
        let w = ui.as_weak();
        let mut last_accounts: Vec<String> = Vec::new();
        let mut last_port_sig = String::new();
        let mut last_chart_gen: u64 = 0;
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(ui) = w.upgrade() else { return };
            let s = state.status.lock().clone();
            let running = state.is_running();
            ui.set_connected(s.connected && !s.drawdown_hit && !s.halted);
            ui.set_running(running);
            ui.set_status_text(
                if s.drawdown_hit {
                    "⚠ DRAWDOWN HALT".to_string()
                } else if s.halted {
                    "■ HALTED — CLOSE ALL done; press START to resume".to_string()
                } else if s.connected && s.master_connected {
                    "● CONNECTED - syncing".to_string()
                } else if s.connected {
                    "● CONNECTED - standby".to_string()
                } else if running {
                    "... connecting".to_string()
                } else {
                    "■ STOPPED".to_string()
                }
                .into(),
            );
            ui.set_balance_text(format!("Net Liquidation: {}", money(s.client_balance)).into());
            ui.set_server_text(if s.master_connected { "Sync: active" } else { "Sync: standby" }.into());
            ui.set_counts_text(
                format!(
                    "Positions {}     Opened {}   Closed {}   Failed {}",
                    s.client_positions, s.orders_placed, s.orders_closed, s.orders_failed
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
            {
                let u = state.update.lock();
                ui.set_update_text(u.message.clone().into());
                ui.set_update_available(u.available);
            }
            ui.set_alert_status(state.alert_status.lock().clone().into());
            // ── Portfolio table (rebuild only when the data changed) ──────────
            ui.set_data_connected(state.data_connected.load(std::sync::atomic::Ordering::Relaxed));
            {
                let rows = state.portfolio.lock().clone();
                let sig: String = rows
                    .iter()
                    .map(|r| format!("{}:{:.0}:{:.2}:{:.2};", r.symbol, r.position, r.market_value, r.unrealized_pnl))
                    .collect();
                if sig != last_port_sig {
                    last_port_sig = sig;
                    let mut total_value = 0.0;
                    let mut total_pnl = 0.0;
                    let model: Vec<PortRow> = rows
                        .iter()
                        .map(|r| {
                            total_value += r.market_value;
                            total_pnl += r.unrealized_pnl;
                            PortRow {
                                symbol: r.symbol.as_str().into(),
                                qty: format!("{:.0}", r.position).into(),
                                avg: money(r.avg_cost).into(),
                                last: money(r.market_price).into(),
                                value: money(r.market_value).into(),
                                pnl: money(r.unrealized_pnl).into(),
                                up: r.unrealized_pnl >= 0.0,
                            }
                        })
                        .collect();
                    ui.set_portfolio(std::rc::Rc::new(slint::VecModel::from(model)).into());
                    ui.set_port_total(money(total_value).into());
                    ui.set_port_pnl(money(total_pnl).into());
                    ui.set_port_pnl_up(total_pnl >= 0.0);
                }
            }

            // ── Chart (copy precomputed candles/labels when the data thread re-renders) ──
            {
                let gen = state.chart_gen.load(std::sync::atomic::Ordering::Relaxed);
                if gen != last_chart_gen {
                    last_chart_gen = gen;
                    push_chart(&ui, &state);
                }
            }

            // Refresh the account picker when the detected list changes.
            let detected = state.detected_accounts.lock().clone();
            if detected != last_accounts {
                last_accounts = detected.clone();
                let model: Vec<slint::SharedString> = detected.iter().map(|a| a.as_str().into()).collect();
                ui.set_accounts(std::rc::Rc::new(slint::VecModel::from(model)).into());
                // Auto-select when exactly one account exists and none is chosen yet.
                if ui.get_account().trim().is_empty() && detected.len() == 1 {
                    ui.set_account(detected[0].clone().into());
                }
            }
        });
    }

    ui.run().expect("failed to run GUI");
}

/// Copy the precomputed `ChartView` from shared state into the GUI's chart properties.
/// Called by the timer on a fresh load and directly by the pan/scroll/zoom callbacks.
fn push_chart(ui: &ClientWindow, state: &SharedState) {
    let c = state.chart.lock().clone();
    let model: Vec<Candle> = c
        .candles
        .iter()
        .map(|k| Candle {
            cx: k.cx,
            bw: k.bw,
            high_y: k.high_y,
            low_y: k.low_y,
            top_y: k.top_y,
            bot_y: k.bot_y,
            up: k.up,
        })
        .collect();
    ui.set_candles(std::rc::Rc::new(slint::VecModel::from(model)).into());
    ui.set_chart_status(c.status.into());
    ui.set_chart_min(c.min_label.into());
    ui.set_chart_max(c.max_label.into());
    ui.set_chart_last(c.last_label.into());
    ui.set_chart_up(c.up);
    ui.set_chart_min_val(c.min_val);
    ui.set_chart_max_val(c.max_val);
    ui.set_pos_present(c.pos_present);
    ui.set_pos_label(c.pos_label.into());
    ui.set_pos_long(c.pos_long);
    ui.set_entry_y(c.entry_y);
    ui.set_entry_label(c.entry_label.into());
    ui.set_sl_present(c.sl_present);
    ui.set_sl_y(c.sl_y);
    ui.set_sl_label(c.sl_label.into());
    ui.set_tp_present(c.tp_present);
    ui.set_tp_y(c.tp_y);
    ui.set_tp_label(c.tp_label.into());
}

fn apply_settings(ui: &ClientWindow, state: &SharedState) {
    let mut c = state.controls.lock();
    c.account_mode = if ui.get_account_mode() == 0 { AccountMode::Live } else { AccountMode::Paper };
    c.trade_mode = if ui.get_trade_mode() == 1 { TradeMode::LongOnly } else { TradeMode::LongShort };
    c.execution_mode = if ui.get_exec_mode() == 1 { ExecutionMode::NewOnly } else { ExecutionMode::ExistingPlusNew };
    c.rth_only = ui.get_hours_mode() == 1;
    if let Ok(v) = ui.get_multiplier().trim().parse::<f64>() {
        c.multiplier = v.clamp(0.1, 5.0);
    }
    if let Ok(v) = ui.get_drawdown().trim().parse::<f64>() {
        c.max_drawdown_pct = v.clamp(1.0, 50.0);
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
    c.ib_account = ui.get_account().trim().to_string();
    // Disconnect alerts.
    c.alerts_enabled = ui.get_alerts_enabled();
    c.alert_email = ui.get_alert_email().trim().to_string();
    if let Ok(v) = ui.get_alert_hours().trim().parse::<f64>() {
        c.alert_after_hours = v.max(0.0);
    }
    c.smtp_host = ui.get_smtp_host().trim().to_string();
    if let Ok(v) = ui.get_smtp_port().trim().parse::<u16>() {
        c.smtp_port = v;
    }
    c.smtp_user = ui.get_smtp_user().trim().to_string();
    c.smtp_pass = ui.get_smtp_pass().to_string();
    c.smtp_from = ui.get_smtp_from().trim().to_string();
    persist::save(&c);
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

const UPDATE_REPO: &str = "advillegas/pax-americana-rust";

/// Non-blocking update check on a background thread; result stored in shared state.
fn spawn_update_check(state: std::sync::Arc<SharedState>) {
    std::thread::spawn(move || {
        state.update.lock().message = "Checking for updates…".to_string();
        let repo = std::env::var("PAX_UPDATE_REPO").unwrap_or_else(|_| UPDATE_REPO.to_string());
        let current = env!("CARGO_PKG_VERSION");
        let mut u = state.update.lock();
        match pax_core::update::check(&repo, current, "client") {
            Some(info) => {
                u.available = true;
                u.url = info.asset_url;
                u.message = format!("Update available: v{}  (you have v{current})", info.version);
            }
            None => {
                u.available = false;
                u.url.clear();
                u.message = format!("Up to date (v{current})");
            }
        }
    });
}

/// Download and self-apply the update, then exit so the relaunch script can swap the exe.
fn spawn_self_update(state: std::sync::Arc<SharedState>) {
    std::thread::spawn(move || {
        let asset = state.update.lock().url.clone();
        state.update.lock().message = "Downloading update…".to_string();
        match pax_core::update::download_and_apply(&asset) {
            Ok(()) => {
                state.update.lock().message = "Restarting to apply update…".to_string();
                std::thread::sleep(Duration::from_millis(900));
                std::process::exit(0);
            }
            Err(e) => {
                state.update.lock().message = format!("Update failed: {e}");
            }
        }
    });
}

/// Detect the IBKR accounts on the local login (background thread) for the GUI picker.
fn spawn_detect_accounts(state: std::sync::Arc<SharedState>) {
    std::thread::spawn(move || {
        let (mode, host, live, paper) = {
            let c = state.controls.lock();
            (c.account_mode, c.ib_host.clone(), c.ib_port_live, c.ib_port_paper)
        };
        let port = match mode {
            AccountMode::Live => live,
            AccountMode::Paper => paper,
        };
        let endpoint = format!("{host}:{port}");
        let cid = config::stable_client_id().wrapping_add(1);
        state.log(LogLevel::Info, format!("Detecting accounts on {endpoint}…"));
        match ib::list_accounts(&endpoint, cid) {
            Ok(list) => {
                let msg = if list.is_empty() { "none".to_string() } else { list.join(", ") };
                *state.detected_accounts.lock() = list;
                state.log(LogLevel::Ok, format!("Detected accounts: {msg}"));
            }
            Err(e) => state.log(LogLevel::Warn, format!("Account detection failed: {e}")),
        }
    });
}

/// Initialize Slint with CPU/software rendering and an OPAQUE OS window (see the master's
/// equivalent for the full rationale). Software rendering is RDP/VPS-friendly; forcing the
/// window non-transparent eliminates the see-through artifact from Slint's default
/// transparent winit surface under partial CPU redraws. Falls back to the env-var default
/// if explicit selection fails, so the GUI always launches.
fn init_opaque_software_backend() {
    let selected = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attrs| attrs.with_transparent(false))
        .select();
    if selected.is_err() {
        std::env::set_var("SLINT_BACKEND", "winit-software");
    }
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
