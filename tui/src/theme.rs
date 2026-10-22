//! Fullscreen dark palette: quiet surfaces, readable secondary text and a
//! single blue accent. State colours are reserved for actionable status.

use ratatui::style::Color;

pub const FG: Color = Color::Rgb(0xe6, 0xe9, 0xef);
pub const GREY: Color = Color::Rgb(0x8b, 0x93, 0xa1);
pub const MUTED: Color = Color::Rgb(0x5d, 0x66, 0x74);
pub const BG: Color = Color::Rgb(0x0b, 0x0d, 0x10);
pub const PANEL_BG: Color = Color::Rgb(0x14, 0x17, 0x1d);
pub const CHIP_BG: Color = Color::Rgb(0x1f, 0x25, 0x2e);
pub const BORDER: Color = Color::Rgb(0x2c, 0x33, 0x3d);
pub const ACCENT: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
pub const USER_BG: Color = Color::Rgb(0x19, 0x21, 0x2d);
pub const CODE: Color = Color::Rgb(0xbf, 0xce, 0xde);
pub const ADDED: Color = Color::Rgb(0x8f, 0xc9, 0x8f);
pub const NOTICE: Color = Color::Rgb(0xa8, 0xb0, 0xbc);
pub const ERROR: Color = Color::Rgb(0xe0, 0x6c, 0x75);
pub const WARNING: Color = Color::Rgb(0xe5, 0xc0, 0x7b);
pub const SUCCESS: Color = Color::Rgb(0x8f, 0xc9, 0x8f);
