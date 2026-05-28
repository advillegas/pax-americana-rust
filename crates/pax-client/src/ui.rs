//! Client GUI — themed control panel (Dark Navy + Fintech Green), faithful to the
//! original Pax Americana layout: connection, execution mode, trading mode, risk
//! management, live stats, and the order feed.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use pax_core::theme as t;

use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

pub fn col(rgb: t::Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

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

pub struct ClientApp {
    state: Arc<SharedState>,
}

impl ClientApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: Arc<SharedState>) -> Self {
        apply_theme(&cc.egui_ctx);
        ClientApp { state }
    }

    fn start(&self) {
        self.state.with_status(|s| {
            s.orders_placed = 0;
            s.orders_closed = 0;
            s.orders_failed = 0;
            s.drawdown_hit = false;
        });
        self.state.running.store(true, Ordering::Relaxed);
        self.state.log(LogLevel::Info, "START pressed — engine starting.");
    }

    fn stop(&self) {
        self.state.running.store(false, Ordering::Relaxed);
        self.state.log(LogLevel::Warn, "STOP pressed — engine stopping.");
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = col(t::BG);
    visuals.window_fill = col(t::BG_PANEL);
    visuals.extreme_bg_color = col(t::BG_INPUT);
    visuals.faint_bg_color = col(t::BG_PANEL);
    visuals.override_text_color = Some(col(t::TEXT));
    visuals.selection.bg_fill = col(t::ACCENT).linear_multiply(0.5);
    visuals.widgets.noninteractive.bg_stroke.color = col(t::BORDER);
    ctx.set_visuals(visuals);
}

impl eframe::App for ClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(400));

        let status = self.state.status.lock().clone();
        let running = self.state.is_running();

        // ── header ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("hdr").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("⬡ PAX AMERICANA").color(col(t::ACCENT)).size(20.0).strong());
                ui.label(RichText::new("CLIENT").color(col(t::TEXT_HEADER)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("Net Liquidation: {}", money(status.client_balance)))
                            .color(col(t::TEXT)),
                    );
                });
            });
            ui.add_space(6.0);
        });

        // ── status bar ─────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("statusbar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (dot_c, label) = if status.drawdown_hit {
                    (col(t::AMBER), "Drawdown halt".to_string())
                } else if status.connected {
                    (col(t::GREEN), "Connected — syncing".to_string())
                } else {
                    (col(t::RED), "Disconnected".to_string())
                };
                ui.label(RichText::new("●").color(dot_c).size(14.0));
                ui.label(RichText::new(label).color(dot_c).strong());
                ui.separator();
                let mc = if status.master_connected { col(t::GREEN) } else { col(t::RED) };
                ui.label(RichText::new(format!("Master: {}", money(status.master_balance))).color(mc));
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "M:{} pos  C:{} pos",
                        status.master_positions, status.client_positions
                    ))
                    .color(col(t::CYAN)),
                );
                if !status.last_sync.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(format!("last sync {}", status.last_sync)).color(col(t::TEXT_DIM)));
                }
            });
            ui.add_space(4.0);
        });

        // ── bottom: order feed ─────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("feed")
            .resizable(true)
            .default_height(220.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("LIVE ORDER FEED").color(col(t::TEXT_HEADER)).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("clear").color(col(t::TEXT_DIM))).clicked() {
                            self.state.log.lock().clear();
                        }
                    });
                });
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    let log = self.state.log.lock();
                    for line in log.lines() {
                        let c = match line.level {
                            LogLevel::Ok => col(t::GREEN),
                            LogLevel::Warn => col(t::AMBER),
                            LogLevel::Err => col(t::RED),
                            LogLevel::Info => col(t::CYAN),
                            LogLevel::Buy => Color32::from_rgb(0x00, 0xff, 0x99),
                            LogLevel::Sell => Color32::from_rgb(0xff, 0x40, 0x60),
                        };
                        ui.label(RichText::new(format!("[{}] {}", line.ts, line.msg)).color(c).monospace());
                    }
                });
            });

        // ── central: controls ───────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut controls = self.state.controls.lock();

            // Connection row.
            panel(ui, "CONNECTION", |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Account").color(col(t::TEXT_DIM)));
                    ui.selectable_value(&mut controls.account_mode, AccountMode::Live, "Live");
                    ui.selectable_value(&mut controls.account_mode, AccountMode::Paper, "Paper");
                    ui.add_space(16.0);

                    let (label, fill) = if running {
                        ("■  STOP", col(t::RED))
                    } else {
                        ("▶  START", col(t::ACCENT))
                    };
                    if ui
                        .add(egui::Button::new(RichText::new(label).color(Color32::BLACK).strong()).fill(fill))
                        .clicked()
                    {
                        if running {
                            self.stop();
                        } else {
                            self.start();
                        }
                    }

                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("CLOSE ALL TRADES").color(Color32::BLACK).strong(),
                            )
                            .fill(col(t::AMBER)),
                        )
                        .clicked()
                    {
                        if running {
                            self.state.close_all.store(true, Ordering::Relaxed);
                            self.state.log(LogLevel::Warn, "CLOSE ALL requested.");
                        } else {
                            self.state.log(LogLevel::Warn, "Press START before using CLOSE ALL.");
                        }
                    }
                });
            });

            // Execution mode.
            panel(ui, "EXECUTION MODE", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut controls.execution_mode,
                        ExecutionMode::ExistingPlusNew,
                        "Existing + New (full sync)",
                    );
                    ui.selectable_value(
                        &mut controls.execution_mode,
                        ExecutionMode::NewOnly,
                        "New Trades Only",
                    );
                });
                ui.label(
                    RichText::new(
                        "Full sync mirrors the master's entire structure: opens missing, \
                         closes orphans, resizes proportionally.",
                    )
                    .color(col(t::TEXT_DIM))
                    .size(11.0),
                );
            });

            // Trading mode.
            panel(ui, "TRADING MODE", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut controls.trade_mode, TradeMode::LongShort, "Long & Short");
                    ui.selectable_value(&mut controls.trade_mode, TradeMode::LongOnly, "Long Only");
                });
                ui.label(
                    RichText::new(
                        "Long Only clamps every target to ≥ 0 — a short can never be opened, \
                         even if the master goes short.",
                    )
                    .color(col(t::TEXT_DIM))
                    .size(11.0),
                );
            });

            // Risk management.
            panel(ui, "RISK MANAGEMENT", |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Size Multiplier").color(col(t::TEXT_DIM)));
                    ui.add(egui::Slider::new(&mut controls.multiplier, 0.1..=5.0).suffix("×").fixed_decimals(1));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Max Drawdown ").color(col(t::TEXT_DIM)));
                    ui.add(egui::Slider::new(&mut controls.max_drawdown_pct, 1.0..=50.0).suffix("%").fixed_decimals(1));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Max Position $ (0=off)").color(col(t::TEXT_DIM)));
                    ui.add(egui::DragValue::new(&mut controls.max_position_notional).speed(500.0).range(0.0..=1.0e9));
                    ui.add_space(12.0);
                    ui.label(RichText::new("Max Qty (0=off)").color(col(t::TEXT_DIM)));
                    ui.add(egui::DragValue::new(&mut controls.max_position_qty).speed(10.0).range(0.0..=1.0e7));
                });
                ui.label(
                    RichText::new(format!(
                        "Proportional sizing ×{:.1}. Trading halts if drawdown exceeds {:.1}%.",
                        controls.multiplier, controls.max_drawdown_pct
                    ))
                    .color(col(t::TEXT_DIM))
                    .size(11.0),
                );
            });

            drop(controls);

            // Stat cards.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                stat_card(ui, "OPENED", status.orders_placed, col(t::ACCENT));
                stat_card(ui, "CLOSED/REDUCED", status.orders_closed, col(t::CYAN));
                stat_card(ui, "FAILED", status.orders_failed, col(t::RED));
            });
        });
    }
}

fn panel<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) {
    egui::Frame::group(ui.style())
        .fill(col(t::BG_PANEL))
        .stroke(egui::Stroke::new(1.0, col(t::BORDER)))
        .show(ui, |ui| {
            ui.label(RichText::new(title).color(col(t::TEXT_HEADER)).strong().size(12.0));
            ui.add_space(2.0);
            add(ui);
        });
    ui.add_space(6.0);
}

fn stat_card(ui: &mut egui::Ui, title: &str, value: u64, color: Color32) {
    egui::Frame::group(ui.style())
        .fill(col(t::BG_PANEL))
        .stroke(egui::Stroke::new(1.0, col(t::BORDER)))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).color(col(t::TEXT_DIM)).size(11.0).strong());
                ui.label(RichText::new(value.to_string()).color(color).size(22.0).strong());
            });
        });
}
