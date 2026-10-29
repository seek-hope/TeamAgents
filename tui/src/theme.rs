//! Codex CLI's terminal-owned prose and semantic palette, adapted to the existing layout.
//! Reference: openai/codex 79be2e20, tui/styles.md and tui/src/style.rs.
//! No terminal color query is issued: use its unknown-background fallbacks for surfaces
//! and selection rather than guessing the user's foreground or background.
//! ponytail: optional startup palette sampling can later enable Codex's adaptive fills
//! and contrast correction; never read terminal input from the rendering path.

use ratatui::style::{Color, Modifier, Style};

/// Primary prose and unpainted surfaces belong to the terminal theme.
pub const FG: Color = Color::Reset;
pub const BG: Color = Color::Reset;
pub const PANEL_BG: Color = Color::Reset;
pub const CHIP_BG: Color = Color::Reset;
pub const GREY: Color = Color::Reset;
pub const MUTED: Color = Color::Reset;
pub const BORDER: Color = Color::Reset;
/// Codex's ChatGPT Blue 200 accent, not the old global Catppuccin blue.
pub const ACCENT: Color = Color::Rgb(99, 168, 248);
/// ANSI magenta identifies the application and assistant, not all Markdown headings.
pub const CODEX: Color = Color::Magenta;
pub const USER: Color = Color::Reset;
/// Codex's submitted-prompt fallback when terminal background sampling is unavailable.
pub const USER_BG: Color = Color::DarkGray;
/// An explicit foreground is needed on this painted ANSI surface, even on light terminals.
pub const USER_FG: Color = Color::White;
pub const CODE: Color = ACCENT;
pub const ADDED: Color = Color::Green;
pub const NOTICE: Color = Color::Reset;
pub const ERROR: Color = Color::Red;
/// Codex avoids yellow attention text without a known dark terminal background.
pub const WARNING: Color = Color::Reset;
pub const SUCCESS: Color = Color::Green;

/// Dim terminal-default text instead of a fixed dark-theme grey.
pub fn secondary() -> Style {
    Style::default().fg(GREY).add_modifier(Modifier::DIM)
}

/// With no sampled palette, Codex selects using inverse terminal colors and bold.
pub(crate) fn selected() -> Style {
    Style::default().fg(FG).bg(BG).add_modifier(Modifier::BOLD | Modifier::REVERSED).remove_modifier(Modifier::DIM)
}

/// Focus stays visible without a hard-coded border grey or a layout change.
pub(crate) fn border(focused: bool) -> Style {
    if focused {
        Style::default().fg(ACCENT)
    } else {
        secondary().fg(BORDER)
    }
}
