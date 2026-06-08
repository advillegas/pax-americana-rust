//! Disconnect email alerts.
//!
//! Watches the IB connection while the engine is running. If the client stays disconnected
//! through the configured number of *weekday* hours (weekends excluded — markets are
//! closed), it emails the client to reconnect their IB Gateway, and resends hourly while
//! the disconnection persists. Sends over SMTP using the operator-configured mailbox
//! (native-tls / Windows SChannel, so the exe stays self-contained).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

use crate::market_hours;
use crate::state::{LogLevel, SharedState};

/// How often the monitor wakes to evaluate connection state.
const TICK: Duration = Duration::from_secs(60);
/// Resend interval once the threshold has been crossed and we're still down.
const RESEND: Duration = Duration::from_secs(3600);

#[derive(Clone)]
struct MailCfg {
    enabled: bool,
    to: String,
    host: String,
    port: u16,
    user: String,
    pass: String,
    from: String,
    after_hours: f64,
}

fn read_cfg(state: &SharedState) -> MailCfg {
    let c = state.controls.lock();
    let from = if c.smtp_from.trim().is_empty() { c.smtp_user.clone() } else { c.smtp_from.clone() };
    MailCfg {
        enabled: c.alerts_enabled,
        to: c.alert_email.trim().to_string(),
        host: c.smtp_host.trim().to_string(),
        port: c.smtp_port,
        user: c.smtp_user.trim().to_string(),
        pass: c.smtp_pass.clone(),
        from: from.trim().to_string(),
        after_hours: c.alert_after_hours.max(0.0),
    }
}

pub fn spawn(state: Arc<SharedState>) {
    thread::spawn(move || monitor(state));
}

fn monitor(state: Arc<SharedState>) {
    // Disconnected weekday time accrued since the last time we were connected.
    let mut down_weekday = Duration::ZERO;
    let mut last_email: Option<Instant> = None;
    let mut last_tick = Instant::now();

    loop {
        thread::sleep(TICK);
        let elapsed = last_tick.elapsed();
        last_tick = Instant::now();

        // One-off test email requested from the GUI.
        if state.alert_test.swap(false, Ordering::Relaxed) {
            let cfg = read_cfg(&state);
            match send(&cfg, "Pax Americana — test alert", "This is a test message. Email alerts are configured correctly.") {
                Ok(()) => set_status(&state, "Test email sent ✓"),
                Err(e) => set_status(&state, &format!("Test failed: {e}")),
            }
        }

        let cfg = read_cfg(&state);
        if !cfg.enabled {
            down_weekday = Duration::ZERO;
            last_email = None;
            continue;
        }

        // "Disconnected" = the engine is meant to be running but IB isn't connected.
        let connected = {
            let s = state.status.lock();
            s.connected
        };
        let disconnected = state.is_running() && !connected;

        if !disconnected {
            // Reconnected (or stopped) — clear the accrual and any alert cadence.
            down_weekday = Duration::ZERO;
            last_email = None;
            continue;
        }

        // Only accrue and alert on weekdays (markets closed on weekends).
        if market_hours::is_weekend_et_now() {
            continue;
        }
        down_weekday += elapsed;

        let threshold = Duration::from_secs_f64(cfg.after_hours * 3600.0);
        if down_weekday < threshold {
            continue;
        }

        let due = match last_email {
            None => true,
            Some(t) => t.elapsed() >= RESEND,
        };
        if !due {
            continue;
        }

        let hours = down_weekday.as_secs_f64() / 3600.0;
        let account = state.status.lock().account.clone();
        let subject = "Pax Americana — connection offline, please reconnect";
        let body = format!(
            "Your trading connection has been offline for about {hours:.1} hour(s) during market days.\n\n\
             Account: {account}\n\n\
             Please log back into your IB Gateway / Trader Workstation so trading can resume.\n\n\
             You will keep receiving this reminder hourly until the connection is restored."
        );
        match send(&cfg, subject, &body) {
            Ok(()) => {
                last_email = Some(Instant::now());
                state.log(LogLevel::Warn, format!("Disconnect alert emailed to {} ({hours:.1}h offline).", cfg.to));
            }
            Err(e) => {
                // Don't reset the cadence on failure, but don't hammer — wait a tick.
                state.log(LogLevel::Err, format!("Alert email failed: {e}"));
                last_email = Some(Instant::now());
            }
        }
    }
}

fn set_status(state: &SharedState, msg: &str) {
    *state.alert_status.lock() = msg.to_string();
    state.log(LogLevel::Info, format!("Alerts: {msg}"));
}

/// Send a plain-text email via the configured SMTP mailbox. Port 465 uses implicit TLS;
/// any other port (e.g. 587) uses STARTTLS. Both go through native-tls (SChannel on Windows).
fn send(cfg: &MailCfg, subject: &str, body: &str) -> Result<(), String> {
    if cfg.host.is_empty() || cfg.to.is_empty() || cfg.from.is_empty() {
        return Err("alert email/SMTP not fully configured".into());
    }
    let email = Message::builder()
        .from(cfg.from.parse().map_err(|e| format!("bad From address: {e}"))?)
        .to(cfg.to.parse().map_err(|e| format!("bad recipient address: {e}"))?)
        .subject(subject)
        .body(body.to_string())
        .map_err(|e| format!("compose: {e}"))?;

    let builder = if cfg.port == 465 {
        SmtpTransport::relay(&cfg.host).map_err(|e| format!("smtp: {e}"))?
    } else {
        SmtpTransport::starttls_relay(&cfg.host).map_err(|e| format!("smtp: {e}"))?
    };
    let mut builder = builder.port(cfg.port);
    if !cfg.user.is_empty() {
        builder = builder.credentials(Credentials::new(cfg.user.clone(), cfg.pass.clone()));
    }
    let mailer = builder.build();
    mailer.send(&email).map(|_| ()).map_err(|e| format!("send: {e}"))
}
