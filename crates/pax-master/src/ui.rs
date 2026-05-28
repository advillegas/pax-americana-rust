//! Master GUI — themed status console (Dark Navy + Fintech Green).

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use pax_core::theme as t;

use crate::state::{LogLevel, SharedState};

pub fn col(rgb: t::Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// Format a USD amount with thousands separators (Rust format strings lack `{:,}`).
pub fn money(v: f64) -> String {
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

pub struct MasterApp {
    state: Arc<SharedState>,
}

impl MasterApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: Arc<SharedState>) -> Self {
        apply_theme(&cc.egui_ctx);
        MasterApp { state }
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = col(t::BG);
    visuals.window_fill = col(t::BG_PANEL);
    visuals.extreme_bg_color = col(t::BG_INPUT);
    visuals.faint_bg_color = col(t::BG_PANEL);
    visuals.override_text_color = Some(col(t::TEXT));
    visuals.widgets.noninteractive.bg_stroke.color = col(t::BORDER);
    ctx.set_visuals(visuals);
}

impl eframe::App for MasterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        let (connected, account, balance, positions, gen_ms) = {
            let s = self.state.snapshot.lock();
            (
                s.connected,
                s.account.clone(),
                s.balance,
                s.positions.clone(),
                s.generated_at_ms,
            )
        };

        egui::TopBottomPanel::top("hdr").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("⬡ PAX AMERICANA").color(col(t::ACCENT)).size(20.0).strong());
                ui.label(RichText::new("MASTER").color(col(t::TEXT_HEADER)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (dot, label, c) = if connected {
                        ("●", "Connected — broadcasting", col(t::GREEN))
                    } else {
                        ("●", "Disconnected", col(t::RED))
                    };
                    ui.label(RichText::new(format!("{dot} {label}")).color(c).strong());
                });
            });
            ui.add_space(8.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("API: {}", self.state.http_bind)).color(col(t::TEXT_DIM)));
                ui.separator();
                ui.label(RichText::new(format!("IB: {}", self.state.endpoint)).color(col(t::TEXT_DIM)));
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::group(ui.style())
                .fill(col(t::BG_PANEL))
                .stroke(egui::Stroke::new(1.0, col(t::BORDER)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("ACCOUNT").color(col(t::TEXT_HEADER)).strong());
                        ui.label(RichText::new(if account.is_empty() { "—" } else { &account }).color(col(t::TEXT)));
                        ui.separator();
                        ui.label(RichText::new("NET LIQUIDATION").color(col(t::TEXT_HEADER)).strong());
                        ui.label(RichText::new(money(balance)).color(col(t::GREEN)).strong());
                        ui.separator();
                        ui.label(RichText::new(format!("{} positions", positions.len())).color(col(t::CYAN)));
                    });
                });

            ui.add_space(8.0);
            ui.label(RichText::new("OPEN POSITIONS (broadcast structure)").color(col(t::TEXT_HEADER)).strong());
            ui.add_space(4.0);

            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                egui::Grid::new("positions").striped(true).num_columns(4).show(ui, |ui| {
                    ui.label(RichText::new("SYMBOL").color(col(t::TEXT_DIM)).strong());
                    ui.label(RichText::new("NET QTY").color(col(t::TEXT_DIM)).strong());
                    ui.label(RichText::new("SIDE").color(col(t::TEXT_DIM)).strong());
                    ui.label(RichText::new("AVG COST").color(col(t::TEXT_DIM)).strong());
                    ui.end_row();
                    for p in &positions {
                        let (side, sc) = if p.net_qty >= 0.0 {
                            ("LONG", col(t::GREEN))
                        } else {
                            ("SHORT", col(t::RED))
                        };
                        ui.label(RichText::new(&p.symbol).color(col(t::WHITE)).strong());
                        ui.label(RichText::new(format!("{:.0}", p.net_qty)).color(col(t::TEXT)));
                        ui.label(RichText::new(side).color(sc));
                        ui.label(RichText::new(format!("{:.2}", p.avg_cost)).color(col(t::TEXT_DIM)));
                        ui.end_row();
                    }
                });
            });

            ui.add_space(8.0);
            ui.label(RichText::new("LOG").color(col(t::TEXT_HEADER)).strong());
            let _ = gen_ms;
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                let log = self.state.log.lock();
                for line in log.lines() {
                    let c = match line.level {
                        LogLevel::Ok => col(t::GREEN),
                        LogLevel::Warn => col(t::AMBER),
                        LogLevel::Err => col(t::RED),
                        LogLevel::Info => col(t::CYAN),
                    };
                    ui.label(
                        RichText::new(format!("[{}] {}", line.ts, line.msg))
                            .color(c)
                            .monospace(),
                    );
                }
            });
        });
    }
}
