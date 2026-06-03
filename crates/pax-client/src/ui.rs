//! Client GUI — native Win32 (GDI) via native-windows-gui. Renders on ANY Windows
//! machine, including RDP/VPS with no OpenGL. No web, no headless.

use std::cell::Cell;
use std::os::windows::process::CommandExt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use native_windows_gui as nwg;

use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct App {
    state: Arc<SharedState>,
    window: nwg::Window,
    status_label: nwg::Label,
    balance_label: nwg::Label,
    master_label: nwg::Label,
    counts_label: nwg::Label,
    start_btn: nwg::Button,
    stop_btn: nwg::Button,
    close_btn: nwg::Button,
    kill_btn: nwg::Button,
    save_btn: nwg::Button,
    account_combo: nwg::ComboBox<&'static str>,
    trade_combo: nwg::ComboBox<&'static str>,
    exec_combo: nwg::ComboBox<&'static str>,
    mult_input: nwg::TextInput,
    dd_input: nwg::TextInput,
    notional_input: nwg::TextInput,
    qty_input: nwg::TextInput,
    host_input: nwg::TextInput,
    live_input: nwg::TextInput,
    paper_input: nwg::TextInput,
    log_box: nwg::TextBox,
    log_seen: Cell<usize>,
    timer: nwg::AnimationTimer,
}

pub fn run(state: Arc<SharedState>) {
    nwg::init().expect("Failed to init native Windows GUI");
    nwg::enable_visual_styles();

    let app = Rc::new(build(state));
    let evt = app.clone();
    let handler = nwg::full_bind_event_handler(&app.window.handle, move |event, _data, handle| {
        use nwg::Event as E;
        match event {
            E::OnTimerTick => {
                if handle == evt.timer.handle {
                    refresh(&evt);
                }
            }
            E::OnButtonClick => {
                if handle == evt.start_btn.handle {
                    save_settings(&evt);
                    evt.state.start_engine();
                } else if handle == evt.stop_btn.handle {
                    evt.state.stop_engine();
                } else if handle == evt.close_btn.handle {
                    evt.state.request_close_all();
                } else if handle == evt.save_btn.handle {
                    save_settings(&evt);
                    evt.state.log(LogLevel::Info, "Settings saved.");
                } else if handle == evt.kill_btn.handle {
                    kill_other_instances();
                    evt.state.log(LogLevel::Warn, "Kill switch: terminated other instances.");
                }
            }
            E::OnWindowClose => {
                if handle == evt.window.handle {
                    nwg::stop_thread_dispatch();
                }
            }
            _ => {}
        }
    });

    app.timer.start();
    refresh(&app);
    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&handler);
}

fn build(state: Arc<SharedState>) -> App {
    let c = state.controls.lock().clone();

    let mut window = nwg::Window::default();
    nwg::Window::builder()
        .size((700, 760))
        .title("Pax Americana — Client")
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
        .build(&mut window)
        .expect("window");

    let label = |text: &str, x: i32, y: i32, w: i32, win: &nwg::Window| {
        let mut l = nwg::Label::default();
        nwg::Label::builder().text(text).parent(win).position((x, y)).size((w, 22)).build(&mut l).expect("label");
        l
    };
    let input = |text: &str, x: i32, y: i32, w: i32, win: &nwg::Window| {
        let mut t = nwg::TextInput::default();
        nwg::TextInput::builder().text(text).parent(win).position((x, y)).size((w, 24)).build(&mut t).expect("input");
        t
    };
    let button = |text: &str, x: i32, y: i32, w: i32, win: &nwg::Window| {
        let mut b = nwg::Button::default();
        nwg::Button::builder().text(text).parent(win).position((x, y)).size((w, 30)).build(&mut b).expect("button");
        b
    };
    let combo = |opts: Vec<&'static str>, sel: usize, x: i32, y: i32, w: i32, win: &nwg::Window| {
        let mut cb = nwg::ComboBox::default();
        nwg::ComboBox::builder().collection(opts).selected_index(Some(sel)).parent(win).position((x, y)).size((w, 24)).build(&mut cb).expect("combo");
        cb
    };

    let status_label = label("Stopped", 16, 12, 660, &window);
    let _ = label("PAX AMERICANA — CLIENT", 16, 38, 660, &window);
    let balance_label = label("Net Liquidation: —", 16, 64, 320, &window);
    let master_label = label("Master: —", 340, 64, 330, &window);
    let counts_label = label("M·C 0·0   Opened 0  Closed 0  Failed 0", 16, 90, 660, &window);

    let _ = label("Engine", 16, 124, 200, &window);
    let start_btn = button("▶ START", 16, 148, 110, &window);
    let stop_btn = button("■ STOP", 134, 148, 90, &window);
    let close_btn = button("CLOSE ALL", 232, 148, 120, &window);
    let kill_btn = button("KILL OTHER INSTANCES", 362, 148, 190, &window);

    let _ = label("Settings", 16, 192, 200, &window);
    let _ = label("Account", 16, 222, 90, &window);
    let account_combo = combo(vec!["Live", "Paper"], if c.account_mode == AccountMode::Live { 0 } else { 1 }, 110, 220, 110, &window);
    let _ = label("Trading", 240, 222, 70, &window);
    let trade_combo = combo(vec!["Long & Short", "Long Only"], if c.trade_mode == TradeMode::LongOnly { 1 } else { 0 }, 320, 220, 140, &window);

    let _ = label("Execution", 16, 254, 90, &window);
    let exec_combo = combo(vec!["Existing + New", "New Only"], if c.execution_mode == ExecutionMode::NewOnly { 1 } else { 0 }, 110, 252, 160, &window);

    let _ = label("Multiplier ×", 16, 288, 100, &window);
    let mult_input = input(&format!("{:.1}", c.multiplier), 120, 286, 80, &window);
    let _ = label("Max Drawdown %", 230, 288, 110, &window);
    let dd_input = input(&format!("{:.1}", c.max_drawdown_pct), 350, 286, 80, &window);

    let _ = label("Max Pos $ (0=off)", 16, 320, 110, &window);
    let notional_input = input(&format!("{:.0}", c.max_position_notional), 130, 318, 100, &window);
    let _ = label("Max Pos Qty (0=off)", 250, 320, 120, &window);
    let qty_input = input(&format!("{:.0}", c.max_position_qty), 380, 318, 100, &window);

    let _ = label("IB Host", 16, 352, 90, &window);
    let host_input = input(&c.ib_host, 110, 350, 180, &window);
    let _ = label("Live port", 16, 384, 90, &window);
    let live_input = input(&c.ib_port_live.to_string(), 110, 382, 80, &window);
    let _ = label("Paper port", 230, 384, 80, &window);
    let paper_input = input(&c.ib_port_paper.to_string(), 320, 382, 80, &window);

    let save_btn = button("SAVE SETTINGS", 16, 418, 160, &window);
    let _ = label("Gateway 4001/4002 · TWS 7496/7497 · host & ports apply on START", 190, 424, 480, &window);

    let _ = label("Live Order Feed", 16, 458, 300, &window);
    let mut log_box = nwg::TextBox::default();
    nwg::TextBox::builder().parent(&window).position((16, 482)).size((664, 230)).readonly(true).build(&mut log_box).expect("log");

    let mut timer = nwg::AnimationTimer::default();
    nwg::AnimationTimer::builder().parent(&window).interval(Duration::from_millis(600)).active(true).build(&mut timer).expect("timer");

    App {
        state,
        window,
        status_label,
        balance_label,
        master_label,
        counts_label,
        start_btn,
        stop_btn,
        close_btn,
        kill_btn,
        save_btn,
        account_combo,
        trade_combo,
        exec_combo,
        mult_input,
        dd_input,
        notional_input,
        qty_input,
        host_input,
        live_input,
        paper_input,
        log_box,
        log_seen: Cell::new(0),
        timer,
    }
}

fn save_settings(app: &App) {
    let mut c = app.state.controls.lock();
    c.account_mode = match app.account_combo.selection() {
        Some(0) => AccountMode::Live,
        _ => AccountMode::Paper,
    };
    c.trade_mode = match app.trade_combo.selection() {
        Some(1) => TradeMode::LongOnly,
        _ => TradeMode::LongShort,
    };
    c.execution_mode = match app.exec_combo.selection() {
        Some(1) => ExecutionMode::NewOnly,
        _ => ExecutionMode::ExistingPlusNew,
    };
    if let Ok(v) = app.mult_input.text().trim().parse::<f64>() {
        c.multiplier = v.clamp(0.1, 5.0);
    }
    if let Ok(v) = app.dd_input.text().trim().parse::<f64>() {
        c.max_drawdown_pct = v.clamp(1.0, 50.0);
    }
    if let Ok(v) = app.notional_input.text().trim().parse::<f64>() {
        c.max_position_notional = v.max(0.0);
    }
    if let Ok(v) = app.qty_input.text().trim().parse::<f64>() {
        c.max_position_qty = v.max(0.0);
    }
    let host = app.host_input.text().trim().to_string();
    if !host.is_empty() {
        c.ib_host = host;
    }
    if let Ok(v) = app.live_input.text().trim().parse::<u16>() {
        c.ib_port_live = v;
    }
    if let Ok(v) = app.paper_input.text().trim().parse::<u16>() {
        c.ib_port_paper = v;
    }
}

fn refresh(app: &App) {
    let s = app.state.status.lock().clone();
    let running = app.state.is_running();

    app.status_label.set_text(if s.drawdown_hit {
        "⚠ DRAWDOWN HALT"
    } else if s.connected {
        "● CONNECTED — syncing"
    } else if running {
        "… connecting"
    } else {
        "■ STOPPED"
    });
    app.balance_label.set_text(&format!("Net Liquidation: {}", money(s.client_balance)));
    app.master_label.set_text(&format!("Master: {}", money(s.master_balance)));
    app.counts_label.set_text(&format!(
        "M·C {}·{}   Opened {}  Closed {}  Failed {}",
        s.master_positions, s.client_positions, s.orders_placed, s.orders_closed, s.orders_failed
    ));
    app.start_btn.set_text(if running { "▶ RUNNING" } else { "▶ START" });

    let (count, text) = {
        let log = app.state.log.lock();
        let lines = log.lines();
        let count = lines.len();
        let start = count.saturating_sub(300);
        let text: String = lines[start..]
            .iter()
            .map(|l| format!("[{}] {} {}\r\n", l.ts, tag(l.level), l.msg))
            .collect();
        (count, text)
    };
    if count != app.log_seen.get() {
        app.log_box.set_text(&text);
        app.log_seen.set(count);
    }
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

/// Kill switch: terminate every other instance of this executable.
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
