//! Codex palette (the project's terminal colour scheme).
//!
//! Dark surfaces, a single blue accent, grey labels with white body text, and
//! success/warning/error reserved for state. `BG` is the screen background,
//! and `PANEL_BG` the surface of every bordered panel.

use ratatui::style::Color;

pub const FG: Color = Color::Rgb(0xff, 0xff, 0xff);
pub const GREY: Color = Color::Rgb(0x5d, 0x5d, 0x5d);
pub const BG: Color = Color::Rgb(0x0d, 0x0d, 0x0d);
pub const PANEL_BG: Color = Color::Rgb(0x18, 0x18, 0x18);
pub const ACCENT: Color = Color::Rgb(0x3b, 0x82, 0xf6);
pub const SUCCESS: Color = Color::Rgb(0x00, 0xff, 0x00);
pub const NOTICE: Color = Color::Rgb(0xaf, 0xaf, 0xaf);
pub const ERROR: Color = Color::Red;
pub const WARNING: Color = Color::Yellow;
