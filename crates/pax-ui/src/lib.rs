//! pax-ui — the shared egui design system for the Pax Americana apps.
//!
//! Centralises the visual language so master and client look like one product: a refined
//! dark "ember on slate" theme, consistent spacing/rounding, and a small kit of reusable
//! widgets (cards, sections, segmented toggles, action buttons, stat tiles, status pills).

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Stroke,
    TextStyle, Ui,
};
use pax_core::theme as th;

// ── palette (Color32) ───────────────────────────────────────────────────────
const fn c(rgb: th::Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

pub const BG: Color32 = c(th::BG);
pub const BG_PANEL: Color32 = c(th::BG_PANEL);
pub const BG_ELEV: Color32 = c(th::BG_ELEV);
pub const BG_HOVER: Color32 = c(th::BG_HOVER);
pub const BG_INPUT: Color32 = c(th::BG_INPUT);
pub const BORDER: Color32 = c(th::BORDER);
pub const BORDER_HOVER: Color32 = c(th::BORDER_HOVER);
pub const TEXT: Color32 = c(th::TEXT);
pub const TEXT_DIM: Color32 = c(th::TEXT_DIM);
pub const TEXT_FAINT: Color32 = c(th::TEXT_FAINT);
pub const WHITE: Color32 = c(th::WHITE);
pub const EMBER: Color32 = c(th::EMBER);
pub const EMBER_DIM: Color32 = c(th::EMBER_DIM);
pub const GREEN: Color32 = c(th::GREEN);
pub const GREEN_DIM: Color32 = c(th::GREEN_DIM);
pub const RED: Color32 = c(th::RED);
pub const RED_DIM: Color32 = c(th::RED_DIM);
pub const AMBER: Color32 = c(th::AMBER);
pub const INFO: Color32 = c(th::INFO);
pub const CYAN: Color32 = INFO;
/// Subtle ember-tinted fill for selections and badges.
pub const SELECTION: Color32 = Color32::from_rgb(0x33, 0x26, 0x20);

// ── style install ─────────────────────────────────────────────────────────────
/// Install the Pax Americana theme + spacing + typography on a context. Call once at
/// app creation.
pub fn install(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG_PANEL;
    v.extreme_bg_color = BG_INPUT;
    v.faint_bg_color = BG_PANEL;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = INFO;
    v.window_corner_radius = CornerRadius::same(12);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.selection.bg_fill = SELECTION;
    v.selection.stroke = Stroke::new(1.0, EMBER);

    let r8 = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG_PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    w.noninteractive.corner_radius = r8;

    w.inactive.bg_fill = BG_ELEV;
    w.inactive.weak_bg_fill = BG_ELEV;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r8;

    w.hovered.bg_fill = BG_HOVER;
    w.hovered.weak_bg_fill = BG_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, BORDER_HOVER);
    w.hovered.fg_stroke = Stroke::new(1.5, WHITE);
    w.hovered.corner_radius = r8;

    w.active.bg_fill = BORDER_HOVER;
    w.active.weak_bg_fill = BORDER_HOVER;
    w.active.bg_stroke = Stroke::new(1.0, EMBER);
    w.active.fg_stroke = Stroke::new(1.5, WHITE);
    w.active.corner_radius = r8;

    w.open.bg_fill = BG_ELEV;
    w.open.bg_stroke = Stroke::new(1.0, BORDER);
    w.open.corner_radius = r8;

    style.visuals = v;

    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.slider_width = 240.0;

    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    ctx.set_style(style);
}

// ── formatting ─────────────────────────────────────────────────────────────────
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

// ── widgets ─────────────────────────────────────────────────────────────────────
fn base_frame(ui: &Ui, fill: Color32, radius: u8) -> Frame {
    Frame::group(ui.style())
        .fill(fill)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(radius))
        .inner_margin(Margin::same(14))
}

/// A flat elevated card.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    base_frame(ui, BG_PANEL, 12).show(ui, add).inner
}

/// A card with an ember section title.
pub fn section<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    base_frame(ui, BG_PANEL, 12)
        .show(ui, |ui| {
            section_title(ui, title);
            ui.add_space(8.0);
            add(ui)
        })
        .inner
}

fn section_title(ui: &mut Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("▍").color(EMBER).size(13.0));
        ui.label(RichText::new(title.to_uppercase()).color(TEXT_DIM).strong().size(11.5));
    });
}

/// A macOS-style segmented selector. Mutates `value` when a segment is clicked.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) {
    Frame::group(ui.style())
        .fill(BG_INPUT)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(|ui| {
                for (val, label) in options {
                    let selected = *value == *val;
                    let (fill, txt) = if selected {
                        (BORDER_HOVER, WHITE)
                    } else {
                        (Color32::TRANSPARENT, TEXT_DIM)
                    };
                    let btn = egui::Button::new(RichText::new(*label).color(txt).strong())
                        .fill(fill)
                        .corner_radius(CornerRadius::same(7))
                        .min_size(egui::vec2(0.0, 26.0));
                    if ui.add(btn).clicked() {
                        *value = *val;
                    }
                }
            });
        });
}

fn action_button(ui: &mut Ui, text: &str, fill: Color32) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::BLACK).strong().size(14.0))
            .fill(fill)
            .corner_radius(CornerRadius::same(8))
            .min_size(egui::vec2(0.0, 34.0)),
    )
}

pub fn ember_button(ui: &mut Ui, text: &str) -> Response {
    action_button(ui, text, EMBER)
}
pub fn go_button(ui: &mut Ui, text: &str) -> Response {
    action_button(ui, text, GREEN)
}
pub fn stop_button(ui: &mut Ui, text: &str) -> Response {
    action_button(ui, text, RED)
}
pub fn warn_button(ui: &mut Ui, text: &str) -> Response {
    action_button(ui, text, AMBER)
}

/// A labelled metric tile. Place inside a column for equal widths.
pub fn stat_tile(ui: &mut Ui, label: &str, value: &str, accent: Color32) {
    base_frame(ui, BG_ELEV, 10).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label.to_uppercase()).color(TEXT_FAINT).strong().size(10.5));
            ui.add_space(2.0);
            ui.label(RichText::new(value).color(accent).strong().size(24.0));
        });
    });
}

/// A rounded status pill: a colored dot + label.
pub fn status_pill(ui: &mut Ui, dot: Color32, text: &str, text_color: Color32) {
    Frame::group(ui.style())
        .fill(BG_ELEV)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(11, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(dot).size(10.0));
                ui.label(RichText::new(text).color(text_color).strong().size(12.0));
            });
        });
}

/// The brand lockup: hex glyph + wordmark + a role badge (e.g. "MASTER" / "CLIENT").
pub fn brand(ui: &mut Ui, role: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("⬢").color(EMBER).size(22.0));
        ui.label(RichText::new("PAX AMERICANA").color(TEXT).strong().size(18.0));
        ui.add_space(2.0);
        Frame::group(ui.style())
            .fill(SELECTION)
            .stroke(Stroke::new(1.0, EMBER_DIM))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(RichText::new(role).color(EMBER).strong().size(11.0));
            });
    });
}
