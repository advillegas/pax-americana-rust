//! Master GUI — themed status console built on the shared pax-ui design system.

use std::sync::Arc;

use eframe::egui::{self, RichText};
use pax_ui as ui;

use crate::state::{IbMode, LogLevel, SharedState};

pub struct MasterApp {
    state: Arc<SharedState>,
}

impl MasterApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: Arc<SharedState>) -> Self {
        ui::install(&cc.egui_ctx);
        MasterApp { state }
    }
}

impl eframe::App for MasterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        let (connected, account, balance, positions) = {
            let s = self.state.snapshot.lock();
            (s.connected, s.account.clone(), s.balance, s.positions.clone())
        };

        egui::TopBottomPanel::top("hdr")
            .frame(egui::Frame::default().fill(ui::BG_PANEL).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |uic| {
                uic.horizontal(|uic| {
                    ui::brand(uic, "MASTER");
                    uic.with_layout(egui::Layout::right_to_left(egui::Align::Center), |uic| {
                        let (dot, label, tc) = if connected {
                            (ui::GREEN, "Connected — broadcasting", ui::GREEN)
                        } else {
                            (ui::RED, "Disconnected", ui::RED)
                        };
                        ui::status_pill(uic, dot, label, tc);
                    });
                });
            });

        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::default().fill(ui::BG).inner_margin(egui::Margin::symmetric(16, 6)))
            .show(ctx, |uic| {
                uic.horizontal(|uic| {
                    uic.label(RichText::new(format!("API  {}", self.state.http_bind)).color(ui::TEXT_DIM).monospace());
                    uic.separator();
                    uic.label(RichText::new(format!("IB  {}", self.state.endpoint())).color(ui::TEXT_DIM).monospace());
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(ui::BG).inner_margin(egui::Margin::same(16)))
            .show(ctx, |uic| {
                // Connection mode toggle.
                ui::section(uic, "Connection", |uic| {
                    let mut mode = self.state.mode.lock();
                    let before = *mode;
                    uic.horizontal(|uic| {
                        uic.label(RichText::new("Account").color(ui::TEXT_DIM));
                        ui::segmented(uic, &mut *mode, &[(IbMode::Live, "Live"), (IbMode::Paper, "Paper")]);
                        uic.add_space(10.0);
                        let (mc, mlabel) = match *mode {
                            IbMode::Live => (ui::RED, "● LIVE — real funds"),
                            IbMode::Paper => (ui::GREEN, "● Paper — simulated"),
                        };
                        uic.label(RichText::new(mlabel).color(mc).strong());
                    });
                    let changed = *mode != before;
                    drop(mode);
                    if changed {
                        self.state.log(LogLevel::Warn, "Mode changed — reconnecting to new port…");
                    }
                    uic.label(
                        RichText::new(format!("Endpoint  {}", self.state.endpoint()))
                            .color(ui::TEXT_FAINT)
                            .monospace()
                            .size(11.0),
                    );
                });

                uic.add_space(10.0);

                // Metric tiles.
                uic.columns(3, |cols| {
                    ui::stat_tile(
                        &mut cols[0],
                        "Account",
                        if account.is_empty() { "—" } else { &account },
                        ui::TEXT,
                    );
                    ui::stat_tile(&mut cols[1], "Net Liquidation", &ui::money(balance), ui::GREEN);
                    ui::stat_tile(&mut cols[2], "Open Positions", &positions.len().to_string(), ui::INFO);
                });

                uic.add_space(10.0);

                // Positions table.
                ui::section(uic, "Broadcast Structure", |uic| {
                    egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, false]).show(uic, |uic| {
                        egui::Grid::new("positions").striped(true).num_columns(4).spacing([24.0, 6.0]).show(uic, |uic| {
                            for h in ["SYMBOL", "NET QTY", "SIDE", "AVG COST"] {
                                uic.label(RichText::new(h).color(ui::TEXT_FAINT).strong().size(10.5));
                            }
                            uic.end_row();
                            if positions.is_empty() {
                                uic.label(RichText::new("flat — no open positions").color(ui::TEXT_DIM));
                                uic.end_row();
                            }
                            for p in &positions {
                                let (side, sc) = if p.net_qty >= 0.0 {
                                    ("LONG", ui::GREEN)
                                } else {
                                    ("SHORT", ui::RED)
                                };
                                uic.label(RichText::new(&p.symbol).color(ui::WHITE).strong());
                                uic.label(RichText::new(format!("{:.0}", p.net_qty)).color(ui::TEXT).monospace());
                                uic.label(RichText::new(side).color(sc).strong());
                                uic.label(RichText::new(format!("{:.2}", p.avg_cost)).color(ui::TEXT_DIM).monospace());
                                uic.end_row();
                            }
                        });
                    });
                });

                uic.add_space(10.0);

                // Log.
                ui::section(uic, "Event Log", |uic| {
                    egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(uic, |uic| {
                        let log = self.state.log.lock();
                        for line in log.lines() {
                            let col = match line.level {
                                LogLevel::Ok => ui::GREEN,
                                LogLevel::Warn => ui::AMBER,
                                LogLevel::Err => ui::RED,
                                LogLevel::Info => ui::INFO,
                            };
                            uic.label(RichText::new(format!("{}  {}", line.ts, line.msg)).color(col).monospace());
                        }
                    });
                });
            });
    }
}
