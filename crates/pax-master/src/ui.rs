//! Master GUI — native Win32 (GDI) via native-windows-gui. Renders on ANY Windows
//! machine, including RDP/VPS sessions with no OpenGL. No web, no headless.

use std::cell::Cell;
use std::os::windows::process::CommandExt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use native_windows_gui as nwg;

use crate::state::{IbMode, LogLevel, SharedState};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct App {
    state: Arc<SharedState>,
    window: nwg::Window,
    status_label: nwg::Label,
    account_label: nwg::Label,
    balance_label: nwg::Label,
    pos_label: nwg::Label,
    host_input: nwg::TextInput,
    live_input: nwg::TextInput,
    paper_input: nwg::TextInput,
    mode_combo: nwg::ComboBox<&'static str>,
    apply_btn: nwg::Button,
    kill_btn: nwg::Button,
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
                if handle == evt.apply_btn.handle {
                    on_apply(&evt);
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
    let conn = state.conn.lock().clone();

    let mut window = nwg::Window::default();
    nwg::Window::builder()
        .size((660, 600))
        .title("Pax Americana — Master")
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
        .build(&mut window)
        .expect("window");

    let mk_label = |text: &str, x: i32, y: i32, w: i32, win: &nwg::Window| {
        let mut l = nwg::Label::default();
        nwg::Label::builder().text(text).parent(win).position((x, y)).size((w, 22)).build(&mut l).expect("label");
        l
    };

    let status_label = mk_label("Connecting…", 16, 12, 620, &window);
    let _t = mk_label("PAX AMERICANA — MASTER", 16, 38, 620, &window);
    let account_label = mk_label("Account: —", 16, 64, 300, &window);
    let balance_label = mk_label("Net Liquidation: —", 330, 64, 310, &window);
    let pos_label = mk_label("Positions: 0", 16, 90, 620, &window);

    let _ = mk_label("Connection", 16, 124, 200, &window);
    let _ = mk_label("IB Host", 16, 152, 90, &window);
    let mut host_input = nwg::TextInput::default();
    nwg::TextInput::builder().text(&conn.host).parent(&window).position((110, 150)).size((200, 24)).build(&mut host_input).expect("host");

    let _ = mk_label("Live port", 16, 184, 90, &window);
    let mut live_input = nwg::TextInput::default();
    nwg::TextInput::builder().text(&conn.port_live.to_string()).parent(&window).position((110, 182)).size((90, 24)).build(&mut live_input).expect("live");

    let _ = mk_label("Paper port", 220, 184, 80, &window);
    let mut paper_input = nwg::TextInput::default();
    nwg::TextInput::builder().text(&conn.port_paper.to_string()).parent(&window).position((310, 182)).size((90, 24)).build(&mut paper_input).expect("paper");

    let _ = mk_label("Mode", 16, 216, 90, &window);
    let mut mode_combo = nwg::ComboBox::default();
    nwg::ComboBox::builder()
        .collection(vec!["Live", "Paper"])
        .selected_index(Some(if conn.mode == IbMode::Live { 0 } else { 1 }))
        .parent(&window)
        .position((110, 214))
        .size((120, 24))
        .build(&mut mode_combo)
        .expect("mode");

    let mut apply_btn = nwg::Button::default();
    nwg::Button::builder().text("APPLY && RECONNECT").parent(&window).position((16, 250)).size((180, 30)).build(&mut apply_btn).expect("apply");

    let mut kill_btn = nwg::Button::default();
    nwg::Button::builder().text("KILL OTHER INSTANCES").parent(&window).position((206, 250)).size((190, 30)).build(&mut kill_btn).expect("kill");

    let _ = mk_label("TWS: 7496 live / 7497 paper   ·   Gateway: 4001 / 4002", 16, 288, 620, &window);
    let _ = mk_label("Log", 16, 312, 200, &window);

    let mut log_box = nwg::TextBox::default();
    nwg::TextBox::builder()
        .parent(&window)
        .position((16, 336))
        .size((624, 224))
        .readonly(true)
        .build(&mut log_box)
        .expect("log");

    let mut timer = nwg::AnimationTimer::default();
    nwg::AnimationTimer::builder()
        .parent(&window)
        .interval(Duration::from_millis(600))
        .active(true)
        .build(&mut timer)
        .expect("timer");

    App {
        state,
        window,
        status_label,
        account_label,
        balance_label,
        pos_label,
        host_input,
        live_input,
        paper_input,
        mode_combo,
        apply_btn,
        kill_btn,
        log_box,
        log_seen: Cell::new(0),
        timer,
    }
}

fn on_apply(app: &App) {
    let host = app.host_input.text();
    let host = host.trim().to_string();
    let live = app.live_input.text().trim().parse::<u16>().ok();
    let paper = app.paper_input.text().trim().parse::<u16>().ok();
    let mode = match app.mode_combo.selection() {
        Some(0) => IbMode::Live,
        _ => IbMode::Paper,
    };
    {
        let mut conn = app.state.conn.lock();
        if !host.is_empty() {
            conn.host = host;
        }
        if let Some(p) = live {
            conn.port_live = p;
        }
        if let Some(p) = paper {
            conn.port_paper = p;
        }
        conn.mode = mode;
    }
    app.state.request_reconnect();
    app.state.log(LogLevel::Warn, format!("Reconnecting to {}", app.state.endpoint()));
}

fn refresh(app: &App) {
    let (connected, account, balance, npos) = {
        let s = app.state.snapshot.lock();
        (s.connected, s.account.clone(), s.balance, s.positions.len())
    };
    app.status_label.set_text(if connected {
        "● CONNECTED — broadcasting"
    } else {
        "✕ DISCONNECTED — connecting…"
    });
    app.account_label.set_text(&format!("Account: {}", if account.is_empty() { "—" } else { &account }));
    app.balance_label.set_text(&format!("Net Liquidation: {}", money(balance)));
    app.pos_label.set_text(&format!("Positions: {npos}"));

    let (count, text) = {
        let log = app.state.log.lock();
        let lines = log.lines();
        let count = lines.len();
        let start = count.saturating_sub(250);
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
