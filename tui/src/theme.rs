//! Catppuccin Mocha palette — the default dark theme of the Codex CLI — mapped
//! to the TUI's semantic roles. Codex's style guide keeps primary text on the
//! default foreground, green for success/additions, red for failures and a
//! distinct magenta for the agent itself; the remaining hues are Catppuccin's.

use ratatui::style::Color;

/// `text` — primary prose.
pub const FG: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
/// `overlay1` — secondary labels.
pub const GREY: Color = Color::Rgb(0x7f, 0x84, 0x9c);
/// `overlay0` — quiet chrome and hints.
pub const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
/// The terminal's own background: no surface is painted.
pub const BG: Color = Color::Reset;
pub const PANEL_BG: Color = Color::Reset;
pub const CHIP_BG: Color = Color::Reset;
/// `surface1` — quiet panel outlines.
pub const BORDER: Color = Color::Rgb(0x45, 0x47, 0x5a);
/// `blue` — active controls, selection and links.
pub const ACCENT: Color = Color::Rgb(0x89, 0xb4, 0xfa);
/// `mauve` — Codex's own colour: brand, agent labels, headings.
pub const CODEX: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
/// `teal` — user input and code.
pub const USER: Color = Color::Rgb(0x94, 0xe2, 0xd5);
pub const USER_BG: Color = Color::Reset;
pub const CODE: Color = Color::Rgb(0x94, 0xe2, 0xd5);
/// `green` — success and additions.
pub const ADDED: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
/// `subtext1` — tool output and notices.
pub const NOTICE: Color = Color::Rgb(0xba, 0xc2, 0xde);
/// `red` — failures and deletions.
pub const ERROR: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
/// `yellow` — pending approvals.
pub const WARNING: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const SUCCESS: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
