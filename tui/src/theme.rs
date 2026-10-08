//! Fullscreen dark palette: quiet surfaces, readable secondary text and a
//! single blue accent. State colours are reserved for actionable status.

use ratatui::style::Color;

pub const FG: Color = Color::Rgb(0xe5, 0xe7, 0xeb);
pub const GREY: Color = Color::Rgb(0x94, 0x94, 0x94);
pub const BG: Color = Color::Rgb(0x0d, 0x0d, 0x0d);
pub const PANEL_BG: Color = Color::Rgb(0x18, 0x18, 0x18);
pub const ACCENT: Color = Color::Rgb(0x3b, 0x82, 0xf6);
pub const SUCCESS: Color = Color::Rgb(0x86, 0xef, 0xac);
pub const NOTICE: Color = Color::Rgb(0xaf, 0xaf, 0xaf);
pub const ERROR: Color = Color::Red;
pub const WARNING: Color = Color::Yellow;
