//! Pax Americana visual theme — refined "ember on slate".
//!
//! A modern dark fintech palette: deep cool-slate surfaces, a warm ember brand accent
//! (a nod to Rust), and crisp semantic colors for trading state. Exposed as raw
//! `(r, g, b)` tuples so this crate stays GUI-framework-agnostic; `pax-ui` converts them
//! to `egui::Color32` and builds the widget styling on top.

pub type Rgb = (u8, u8, u8);

// ── surfaces ────────────────────────────────────────────────────────────────
pub const BG: Rgb = (0x0a, 0x0e, 0x16); // app background
pub const BG_PANEL: Rgb = (0x10, 0x17, 0x23); // raised panel
pub const BG_ELEV: Rgb = (0x17, 0x21, 0x30); // cards / inputs
pub const BG_HOVER: Rgb = (0x20, 0x2c, 0x3e); // hovered surface
pub const BG_INPUT: Rgb = (0x12, 0x1a, 0x28);

// ── lines ─────────────────────────────────────────────────────────────────────
pub const BORDER: Rgb = (0x22, 0x2d, 0x40);
pub const BORDER_HOVER: Rgb = (0x35, 0x45, 0x60);

// ── text ─────────────────────────────────────────────────────────────────────
pub const TEXT: Rgb = (0xe9, 0xf0, 0xfb);
pub const TEXT_DIM: Rgb = (0x90, 0x9f, 0xb7);
pub const TEXT_FAINT: Rgb = (0x5a, 0x68, 0x80);
pub const WHITE: Rgb = (0xf5, 0xf9, 0xff);

// ── brand + semantic accents ─────────────────────────────────────────────────
pub const EMBER: Rgb = (0xff, 0x78, 0x49); // brand / primary highlight
pub const EMBER_DIM: Rgb = (0xc9, 0x57, 0x2f);
pub const GREEN: Rgb = (0x34, 0xd3, 0x99); // up / connected / go
pub const GREEN_DIM: Rgb = (0x1f, 0xa9, 0x7a);
pub const RED: Rgb = (0xfb, 0x5d, 0x6e); // down / disconnected / stop
pub const RED_DIM: Rgb = (0xc4, 0x3f, 0x50);
pub const AMBER: Rgb = (0xf5, 0xb4, 0x54); // warnings / caution
pub const INFO: Rgb = (0x53, 0xb9, 0xf2); // info / neutral metrics
pub const CYAN: Rgb = INFO;

// Header/accent label color.
pub const TEXT_HEADER: Rgb = EMBER;
pub const ACCENT: Rgb = EMBER;
