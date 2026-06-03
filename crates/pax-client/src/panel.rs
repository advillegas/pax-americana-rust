//! Browser control panel for the client. Serves the dashboard and the control/settings
//! endpoints over HTTP so the client is fully operable on a headless/VPS machine.
//!
//! Security note: this panel can START/STOP the engine and CLOSE ALL positions. Set
//! `PAX_PANEL_KEY` (and bind only where you trust the network) whenever it's reachable
//! beyond localhost.

use std::sync::Arc;
use std::thread;

use serde::Deserialize;
use tiny_http::{Header, Method, Response, Server};

use crate::dashboard;
use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

pub fn spawn(bind: String, key: String, state: Arc<SharedState>) {
    thread::spawn(move || {
        let server = match Server::http(&bind) {
            Ok(s) => s,
            Err(e) => {
                state.log(LogLevel::Err, format!("Control panel bind failed on {bind}: {e}"));
                return;
            }
        };
        state.log(LogLevel::Ok, format!("Control panel on http://{bind}/"));
        let server = Arc::new(server);
        for _ in 0..4 {
            let server = server.clone();
            let state = state.clone();
            let key = key.clone();
            thread::spawn(move || {
                for request in server.incoming_requests() {
                    handle(request, &state, &key);
                }
            });
        }
        loop {
            thread::park();
        }
    });
}

#[derive(Deserialize, Default)]
struct SettingsUpdate {
    account_mode: Option<String>,
    trade_mode: Option<String>,
    execution_mode: Option<String>,
    multiplier: Option<f64>,
    max_drawdown_pct: Option<f64>,
    max_position_notional: Option<f64>,
    max_position_qty: Option<f64>,
    ib_host: Option<String>,
    ib_port_live: Option<u16>,
    ib_port_paper: Option<u16>,
}

#[derive(Deserialize)]
struct ControlAction {
    action: String,
}

fn handle(mut request: tiny_http::Request, state: &SharedState, key: &str) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let method = request.method().clone();

    if path == "/" && method == Method::Get {
        let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
        let _ = request.respond(Response::from_string(dashboard::HTML).with_header(header));
        return;
    }

    if !key.is_empty() {
        let provided = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-API-Key"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if provided != key {
            let _ = request.respond(json(401, r#"{"error":"unauthorized"}"#));
            return;
        }
    }

    if method == Method::Post && path == "/control" {
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        match serde_json::from_str::<ControlAction>(&body) {
            Ok(c) => {
                match c.action.as_str() {
                    "start" => state.start_engine(),
                    "stop" => state.stop_engine(),
                    "close_all" => {
                        state.request_close_all();
                    }
                    _ => {}
                }
                let _ = request.respond(json(200, r#"{"ok":true}"#));
            }
            Err(e) => {
                let _ = request.respond(json(400, &format!(r#"{{"error":"{e}"}}"#)));
            }
        }
        return;
    }

    if method == Method::Post && path == "/settings" {
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        match serde_json::from_str::<SettingsUpdate>(&body) {
            Ok(u) => {
                apply_settings(state, u);
                let _ = request.respond(json(200, r#"{"ok":true}"#));
            }
            Err(e) => {
                let _ = request.respond(json(400, &format!(r#"{{"error":"{e}"}}"#)));
            }
        }
        return;
    }

    if method != Method::Get {
        let _ = request.respond(json(405, r#"{"error":"method_not_allowed"}"#));
        return;
    }

    let out = match path.as_str() {
        "/state" => state_json(state),
        "/log" => log_json(state),
        _ => {
            let _ = request.respond(json(404, r#"{"error":"not_found"}"#));
            return;
        }
    };
    let _ = request.respond(json(200, &out));
}

fn apply_settings(state: &SharedState, u: SettingsUpdate) {
    let mut c = state.controls.lock();
    if let Some(v) = u.account_mode {
        c.account_mode = if v.eq_ignore_ascii_case("live") { AccountMode::Live } else { AccountMode::Paper };
    }
    if let Some(v) = u.trade_mode {
        c.trade_mode = if v.eq_ignore_ascii_case("long_only") { TradeMode::LongOnly } else { TradeMode::LongShort };
    }
    if let Some(v) = u.execution_mode {
        c.execution_mode = if v.eq_ignore_ascii_case("new") { ExecutionMode::NewOnly } else { ExecutionMode::ExistingPlusNew };
    }
    if let Some(v) = u.multiplier {
        c.multiplier = v.clamp(0.1, 5.0);
    }
    if let Some(v) = u.max_drawdown_pct {
        c.max_drawdown_pct = v.clamp(1.0, 50.0);
    }
    if let Some(v) = u.max_position_notional {
        c.max_position_notional = v.max(0.0);
    }
    if let Some(v) = u.max_position_qty {
        c.max_position_qty = v.max(0.0);
    }
    if let Some(v) = u.ib_host {
        if !v.trim().is_empty() {
            c.ib_host = v.trim().to_string();
        }
    }
    if let Some(v) = u.ib_port_live {
        c.ib_port_live = v;
    }
    if let Some(v) = u.ib_port_paper {
        c.ib_port_paper = v;
    }
}

fn state_json(state: &SharedState) -> String {
    let s = state.status.lock();
    let c = state.controls.lock();
    let account_mode = match c.account_mode {
        AccountMode::Live => "live",
        AccountMode::Paper => "paper",
    };
    let trade_mode = match c.trade_mode {
        TradeMode::LongShort => "long_short",
        TradeMode::LongOnly => "long_only",
    };
    let execution_mode = match c.execution_mode {
        ExecutionMode::ExistingPlusNew => "existing",
        ExecutionMode::NewOnly => "new",
    };
    format!(
        r#"{{"running":{},"connected":{},"account":{},"client_balance":{},"master_balance":{},"master_connected":{},"master_positions":{},"client_positions":{},"drawdown_hit":{},"last_sync":{},"orders_placed":{},"orders_closed":{},"orders_failed":{},"controls":{{"account_mode":"{}","trade_mode":"{}","execution_mode":"{}","multiplier":{},"max_drawdown_pct":{},"max_position_notional":{},"max_position_qty":{},"ib_host":{},"ib_port_live":{},"ib_port_paper":{}}}}}"#,
        state.is_running(),
        s.connected,
        json_str(&s.account),
        s.client_balance,
        s.master_balance,
        s.master_connected,
        s.master_positions,
        s.client_positions,
        s.drawdown_hit,
        json_str(&s.last_sync),
        s.orders_placed,
        s.orders_closed,
        s.orders_failed,
        account_mode,
        trade_mode,
        execution_mode,
        c.multiplier,
        c.max_drawdown_pct,
        c.max_position_notional,
        c.max_position_qty,
        json_str(&c.ib_host),
        c.ib_port_live,
        c.ib_port_paper,
    )
}

fn log_json(state: &SharedState) -> String {
    let log = state.log.lock();
    let items: Vec<String> = log
        .lines()
        .iter()
        .map(|l| {
            let lvl = match l.level {
                LogLevel::Ok => "OK",
                LogLevel::Warn => "WARN",
                LogLevel::Err => "ERR",
                LogLevel::Info => "INFO",
                LogLevel::Buy => "BUY",
                LogLevel::Sell => "SELL",
            };
            format!(r#"{{"ts":{},"level":"{}","msg":{}}}"#, json_str(&l.ts), lvl, json_str(&l.msg))
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn json(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    Response::from_string(body).with_status_code(status).with_header(header)
}
