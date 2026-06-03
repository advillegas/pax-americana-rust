//! Blocking HTTP API + embedded web control panel. No async runtime — one tiny_http
//! server plus a small worker pool, which keeps the master simple and crash-resistant.
//! The web panel (served at `/`) is the VPS-friendly GUI: it needs no graphics stack.

use std::sync::Arc;
use std::thread;

use pax_core::protocol::StatusDoc;
use pax_core::PROTOCOL_SCHEMA;
use serde::Deserialize;
use tiny_http::{Header, Method, Response, Server};

use crate::dashboard;
use crate::state::{IbMode, LogLevel, SharedState};

pub fn spawn(bind: String, api_key: String, state: Arc<SharedState>) {
    thread::spawn(move || {
        let server = match Server::http(&bind) {
            Ok(s) => s,
            Err(e) => {
                state.log(LogLevel::Err, format!("HTTP bind failed on {bind}: {e}"));
                return;
            }
        };
        state.log(LogLevel::Ok, format!("HTTP API + web panel on http://{bind}/"));
        let server = Arc::new(server);

        for _ in 0..4 {
            let server = server.clone();
            let state = state.clone();
            let api_key = api_key.clone();
            thread::spawn(move || {
                for request in server.incoming_requests() {
                    handle(request, &state, &api_key);
                }
            });
        }
        loop {
            thread::park();
        }
    });
}

#[derive(Deserialize)]
struct ConfigUpdate {
    host: Option<String>,
    port_live: Option<u16>,
    port_paper: Option<u16>,
    mode: Option<String>,
}

fn handle(mut request: tiny_http::Request, state: &SharedState, api_key: &str) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let method = request.method().clone();

    // The dashboard shell loads without auth; its JS sends the key for data/config.
    if path == "/" && method == Method::Get {
        let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
        let _ = request.respond(Response::from_string(dashboard::HTML).with_header(header));
        return;
    }

    // Auth gate for everything else.
    if !api_key.is_empty() {
        let provided = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-API-Key"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if provided != api_key {
            let _ = request.respond(json_response(401, r#"{"error":"unauthorized"}"#));
            return;
        }
    }

    // Config update (the only mutating endpoint).
    if path == "/config" && method == Method::Post {
        let mut body = String::new();
        if request.as_reader().read_to_string(&mut body).is_err() {
            let _ = request.respond(json_response(400, r#"{"error":"bad_body"}"#));
            return;
        }
        match serde_json::from_str::<ConfigUpdate>(&body) {
            Ok(upd) => {
                {
                    let mut conn = state.conn.lock();
                    if let Some(h) = upd.host {
                        if !h.trim().is_empty() {
                            conn.host = h.trim().to_string();
                        }
                    }
                    if let Some(p) = upd.port_live {
                        conn.port_live = p;
                    }
                    if let Some(p) = upd.port_paper {
                        conn.port_paper = p;
                    }
                    if let Some(m) = upd.mode {
                        conn.mode = if m.eq_ignore_ascii_case("live") {
                            IbMode::Live
                        } else {
                            IbMode::Paper
                        };
                    }
                }
                state.request_reconnect();
                state.log(LogLevel::Warn, format!("Config updated via web → reconnecting to {}", state.endpoint()));
                let _ = request.respond(json_response(200, r#"{"ok":true}"#));
            }
            Err(e) => {
                let _ = request.respond(json_response(400, &format!(r#"{{"error":"bad_json: {e}"}}"#)));
            }
        }
        return;
    }

    if method != Method::Get {
        let _ = request.respond(json_response(405, r#"{"error":"method_not_allowed"}"#));
        return;
    }

    let body = match path.as_str() {
        "/snapshot" | "/positions" => state.snapshot.lock().to_json(),
        "/balance" => {
            let snap = state.snapshot.lock();
            format!(r#"{{"balance":{},"connected":{}}}"#, snap.balance, snap.connected)
        }
        "/config" => {
            let conn = state.conn.lock();
            let mode = match conn.mode {
                IbMode::Live => "live",
                IbMode::Paper => "paper",
            };
            format!(
                r#"{{"host":"{}","port_live":{},"port_paper":{},"mode":"{}"}}"#,
                conn.host, conn.port_live, conn.port_paper, mode
            )
        }
        "/log" => log_json(state),
        "/status" => {
            let snap = state.snapshot.lock();
            let doc = StatusDoc {
                status: "running",
                connected: snap.connected,
                balance: snap.balance,
                total_positions: snap.positions.len(),
                schema: PROTOCOL_SCHEMA,
            };
            serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string())
        }
        _ => {
            let _ = request.respond(json_response(404, r#"{"error":"not_found"}"#));
            return;
        }
    };

    let _ = request.respond(json_response(200, &body));
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
            };
            format!(
                r#"{{"ts":{},"level":"{}","msg":{}}}"#,
                serde_json::to_string(&l.ts).unwrap_or_else(|_| "\"\"".into()),
                lvl,
                serde_json::to_string(&l.msg).unwrap_or_else(|_| "\"\"".into()),
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn json_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    Response::from_string(body).with_status_code(status).with_header(header)
}
