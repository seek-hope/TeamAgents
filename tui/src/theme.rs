//! Codex palette.

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
/// `$primary` at 40% over the #0d0d0d background (table cursor row).
pub const CURSOR_ROW: Color = Color::Rgb(0x1f, 0x3c, 0x6a);
/// Selected table row: a barely lighter block under the accent bar.
pub const SELECT_BG: Color = Color::Rgb(0x1a, 0x1a, 0x1a);
/// Alternating table rows, one step above the background.
pub const ZEBRA_BG: Color = Color::Rgb(0x11, 0x11, 0x11);
/// Hovered clickable thing: grey surface + white text (the tab/row hint).
pub const HOVER_BG: Color = Color::Rgb(0x3a, 0x3a, 0x3a);
