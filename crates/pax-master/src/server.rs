//! Blocking HTTP API served to clients. No async runtime — one tiny_http server thread
//! plus a small worker pool, which keeps the master simple and crash-resistant.

use std::sync::Arc;
use std::thread;

use pax_core::protocol::StatusDoc;
use pax_core::PROTOCOL_SCHEMA;
use tiny_http::{Header, Method, Response, Server};

use crate::state::{LogLevel, SharedState};

pub fn spawn(bind: String, api_key: String, state: Arc<SharedState>) {
    thread::spawn(move || {
        let server = match Server::http(&bind) {
            Ok(s) => s,
            Err(e) => {
                state.log(LogLevel::Err, format!("HTTP bind failed on {bind}: {e}"));
                return;
            }
        };
        state.log(LogLevel::Ok, format!("HTTP API listening on {bind}"));
        let server = Arc::new(server);

        // A small fixed pool of handler threads.
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

        // Park the spawner thread forever; the pool does the work.
        loop {
            thread::park();
        }
    });
}

fn handle(request: tiny_http::Request, state: &SharedState, api_key: &str) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/");

    // Auth: when configured, require a matching X-API-Key header.
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

    if request.method() != &Method::Get {
        let _ = request.respond(json_response(405, r#"{"error":"method_not_allowed"}"#));
        return;
    }

    let body = match path {
        "/snapshot" | "/positions" => {
            let snap = state.snapshot.lock();
            snap.to_json()
        }
        "/balance" => {
            let snap = state.snapshot.lock();
            format!(r#"{{"balance":{},"connected":{}}}"#, snap.balance, snap.connected)
        }
        "/status" | "/" => {
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

fn json_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    Response::from_string(body).with_status_code(status).with_header(header)
}
