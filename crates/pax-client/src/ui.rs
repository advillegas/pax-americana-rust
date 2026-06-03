//! Client GUI — themed control panel built on the shared pax-ui design system.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui::{self, RichText};
use pax_ui as ui;

use crate::state::{AccountMode, ExecutionMode, LogLevel, SharedState, TradeMode};

pub struct ClientApp {
    state: Arc<SharedState>,
}

impl ClientApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: Arc<SharedState>) -> Self {
        ui::install(&cc.egui_ctx);
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

impl eframe::App for ClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(400));

        let status = self.state.status.lock().clone();
        let running = self.state.is_running();

        // ── header ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("hdr")
            .frame(egui::Frame::default().fill(ui::BG_PANEL).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |uic| {
                uic.horizontal(|uic| {
                    ui::brand(uic, "CLIENT");
                    uic.with_layout(egui::Layout::right_to_left(egui::Align::Center), |uic| {
                        uic.label(RichText::new(ui::money(status.client_balance)).color(ui::GREEN).strong().size(16.0));
                        uic.label(RichText::new("NET LIQ").color(ui::TEXT_FAINT).size(10.5).strong());
                    });
                });
            });

        // ── status strip ───────────────────────────────────────────────────────
        egui::TopBottomPanel::top("statusbar")
            .frame(egui::Frame::default().fill(ui::BG).inner_margin(egui::Margin::symmetric(16, 7)))
            .show(ctx, |uic| {
                uic.horizontal(|uic| {
                    let (dot, label, tc) = if status.drawdown_hit {
                        (ui::AMBER, "Drawdown halt", ui::AMBER)
                    } else if status.connected {
                        (ui::GREEN, "Connected — syncing", ui::GREEN)
                    } else {
                        (ui::RED, "Disconnected", ui::RED)
                    };
                    ui::status_pill(uic, dot, label, tc);
                    let mdot = if status.master_connected { ui::GREEN } else { ui::RED };
                    ui::status_pill(uic, mdot, &format!("Master {}", ui::money(status.master_balance)), ui::TEXT);
                    uic.label(
                        RichText::new(format!("M {} · C {} pos", status.master_positions, status.client_positions))
                            .color(ui::CYAN)
                            .monospace(),
                    );
                    if !status.last_sync.is_empty() {
                        uic.with_layout(egui::Layout::right_to_left(egui::Align::Center), |uic| {
                            uic.label(RichText::new(format!("last sync {}", status.last_sync)).color(ui::TEXT_DIM).monospace());
                        });
                    }
                });
            });

        // ── order feed (bottom) ──────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("feed")
            .resizable(true)
            .default_height(230.0)
            .frame(egui::Frame::default().fill(ui::BG).inner_margin(egui::Margin::same(16)))
            .show(ctx, |uic| {
                uic.horizontal(|uic| {
                    uic.label(RichText::new("▍").color(ui::EMBER).size(13.0));
                    uic.label(RichText::new("LIVE ORDER FEED").color(ui::TEXT_DIM).strong().size(11.5));
                    uic.with_layout(egui::Layout::right_to_left(egui::Align::Center), |uic| {
                        if uic.add(egui::Button::new(RichText::new("clear").color(ui::TEXT_DIM)).fill(ui::BG_ELEV)).clicked() {
                            self.state.log.lock().clear();
                        }
                    });
                });
                uic.add_space(6.0);
                egui::Frame::default()
                    .fill(ui::BG_INPUT)
                    .stroke(egui::Stroke::new(1.0, ui::BORDER))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(10))
                    .show(uic, |uic| {
                        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(uic, |uic| {
                            let log = self.state.log.lock();
                            for line in log.lines() {
                                let col = match line.level {
                                    LogLevel::Ok => ui::GREEN,
                                    LogLevel::Warn => ui::AMBER,
                                    LogLevel::Err => ui::RED,
                                    LogLevel::Info => ui::INFO,
                                    LogLevel::Buy => ui::GREEN,
                                    LogLevel::Sell => ui::RED,
                                };
                                uic.label(RichText::new(format!("{}  {}", line.ts, line.msg)).color(col).monospace());
                            }
                        });
                    });
            });

        // ── controls (central) ───────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(ui::BG).inner_margin(egui::Margin::same(16)))
            .show(ctx, |uic| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(uic, |uic| {
                    let mut controls = self.state.controls.lock();

                    ui::section(uic, "Connection", |uic| {
                        uic.horizontal(|uic| {
                            uic.label(RichText::new("Account").color(ui::TEXT_DIM));
                            ui::segmented(
                                uic,
                                &mut controls.account_mode,
                                &[(AccountMode::Live, "Live"), (AccountMode::Paper, "Paper")],
                            );
                            uic.add_space(12.0);
                            let clicked = if running {
                                ui::stop_button(uic, "■  STOP").clicked()
                            } else {
                                ui::go_button(uic, "▶  START").clicked()
                            };
                            if clicked {
                                if running {
                                    self.stop();
                                } else {
                                    self.start();
                                }
                            }
                            if ui::warn_button(uic, "CLOSE ALL").clicked() {
                                if running {
                                    self.state.close_all.store(true, Ordering::Relaxed);
                                    self.state.log(LogLevel::Warn, "CLOSE ALL requested.");
                                } else {
                                    self.state.log(LogLevel::Warn, "Press START before using CLOSE ALL.");
                                }
                            }
                        });
                        uic.add_space(6.0);
                        uic.add_enabled_ui(!running, |uic| {
                            egui::Grid::new("client_conn").num_columns(4).spacing([12.0, 6.0]).show(uic, |uic| {
                                uic.label(RichText::new("IB Host").color(ui::TEXT_DIM));
                                uic.add(egui::TextEdit::singleline(&mut controls.ib_host).desired_width(150.0));
                                uic.end_row();
                                uic.label(RichText::new("Live port").color(ui::TEXT_DIM));
                                uic.add(egui::DragValue::new(&mut controls.ib_port_live).range(1..=65535).speed(1.0));
                                uic.label(RichText::new("Paper port").color(ui::TEXT_DIM));
                                uic.add(egui::DragValue::new(&mut controls.ib_port_paper).range(1..=65535).speed(1.0));
                                uic.end_row();
                            });
                        });
                        uic.label(
                            RichText::new(
                                "Gateway: 4001 live / 4002 paper · TWS: 7496 / 7497 · applied on START",
                            )
                            .color(ui::TEXT_FAINT)
                            .size(10.5),
                        );
                    });
                    uic.add_space(10.0);

                    ui::section(uic, "Execution Mode", |uic| {
                        ui::segmented(
                            uic,
                            &mut controls.execution_mode,
                            &[
                                (ExecutionMode::ExistingPlusNew, "Existing + New"),
                                (ExecutionMode::NewOnly, "New Trades Only"),
                            ],
                        );
                        uic.add_space(4.0);
                        uic.label(
                            RichText::new(
                                "Full sync mirrors the master's entire structure: opens missing, closes orphans, resizes.",
                            )
                            .color(ui::TEXT_FAINT)
                            .size(11.0),
                        );
                    });
                    uic.add_space(10.0);

                    ui::section(uic, "Trading Mode", |uic| {
                        ui::segmented(
                            uic,
                            &mut controls.trade_mode,
                            &[(TradeMode::LongShort, "Long & Short"), (TradeMode::LongOnly, "Long Only")],
                        );
                        uic.add_space(4.0);
                        uic.label(
                            RichText::new("Long Only clamps every target to ≥ 0 — a short can never be opened.")
                                .color(ui::TEXT_FAINT)
                                .size(11.0),
                        );
                    });
                    uic.add_space(10.0);

                    ui::section(uic, "Risk Management", |uic| {
                        egui::Grid::new("risk").num_columns(2).spacing([16.0, 10.0]).show(uic, |uic| {
                            uic.label(RichText::new("Size Multiplier").color(ui::TEXT_DIM));
                            uic.add(egui::Slider::new(&mut controls.multiplier, 0.1..=5.0).suffix("×").fixed_decimals(1));
                            uic.end_row();

                            uic.label(RichText::new("Max Drawdown").color(ui::TEXT_DIM));
                            uic.add(egui::Slider::new(&mut controls.max_drawdown_pct, 1.0..=50.0).suffix("%").fixed_decimals(1));
                            uic.end_row();

                            uic.label(RichText::new("Max Position $ (0=off)").color(ui::TEXT_DIM));
                            uic.add(egui::DragValue::new(&mut controls.max_position_notional).speed(500.0).range(0.0..=1.0e9));
                            uic.end_row();

                            uic.label(RichText::new("Max Position Qty (0=off)").color(ui::TEXT_DIM));
                            uic.add(egui::DragValue::new(&mut controls.max_position_qty).speed(10.0).range(0.0..=1.0e7));
                            uic.end_row();
                        });
                        uic.add_space(4.0);
                        uic.label(
                            RichText::new(format!(
                                "Proportional sizing ×{:.1}. Trading halts if drawdown exceeds {:.1}%.",
                                controls.multiplier, controls.max_drawdown_pct
                            ))
                            .color(ui::TEXT_FAINT)
                            .size(11.0),
                        );
                    });
                    drop(controls);

                    uic.add_space(10.0);
                    uic.columns(3, |cols| {
                        ui::stat_tile(&mut cols[0], "Opened", &status.orders_placed.to_string(), ui::GREEN);
                        ui::stat_tile(&mut cols[1], "Closed / Reduced", &status.orders_closed.to_string(), ui::INFO);
                        ui::stat_tile(&mut cols[2], "Failed", &status.orders_failed.to_string(), ui::RED);
                    });
                });
            });
    }
}
