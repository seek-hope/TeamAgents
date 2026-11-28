//! Pure Black + Rainbow Prism palette, `NO_COLOR`/`TERM=dumb` plain fallback, and CJK truncation.
use crate::protocol::{sanitize_inline, TaskStatus, Verification};
use ratatui::style::{Color, Modifier, Style};
use std::env;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠏"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    pub canvas_bg: Color,
    pub sidebar_bg: Color,
    pub surface_bg: Color,
    pub elevated_bg: Color,
    pub selected_bg: Color,
    pub danger_bg: Color,
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub border_subtle: Color,
    pub border_focus: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub ok: Color,
    pub active: Color,
    pub warn: Color,
    pub error: Color,
    pub selected_fg: Color,
    pub rainbow: [Color; 7],
}

const RAINBOW_SPECTRUM: [Color; 7] = [
    Color::Rgb(255, 77, 109),  // #FF4D6D red-rose
    Color::Rgb(255, 146, 43),  // #FF922B orange
    Color::Rgb(255, 212, 59),  // #FFD43B yellow
    Color::Rgb(46, 230, 122),  // #2EE67A green
    Color::Rgb(0, 229, 255),   // #00E5FF cyan
    Color::Rgb(77, 150, 255),  // #4D96FF blue
    Color::Rgb(192, 112, 255), // #C070FF violet
];

pub(crate) const RAINBOW_PALETTE: Palette = Palette {
    canvas_bg: Color::Rgb(0, 0, 0),
    sidebar_bg: Color::Rgb(0, 0, 0),
    surface_bg: Color::Rgb(14, 14, 18),
    elevated_bg: Color::Rgb(12, 12, 16),
    selected_bg: Color::Rgb(24, 24, 34),
    danger_bg: Color::Rgb(58, 10, 18),
    diff_add_bg: Color::Rgb(6, 36, 18),
    diff_del_bg: Color::Rgb(52, 10, 18),
    border_subtle: Color::Rgb(42, 42, 52),
    border_focus: RAINBOW_SPECTRUM[4],
    text_primary: Color::Rgb(255, 255, 255),
    text_secondary: Color::Rgb(196, 200, 212),
    text_muted: Color::Rgb(124, 128, 144),
    text_dim: Color::Rgb(82, 86, 102),
    accent: RAINBOW_SPECTRUM[4],
    accent_soft: RAINBOW_SPECTRUM[6],
    ok: RAINBOW_SPECTRUM[3],
    active: RAINBOW_SPECTRUM[2],
    warn: RAINBOW_SPECTRUM[1],
    error: RAINBOW_SPECTRUM[0],
    selected_fg: Color::Rgb(255, 255, 255),
    rainbow: RAINBOW_SPECTRUM,
};

impl Palette {
    /// Monochrome fallback honoring `NO_COLOR` / `TERM=dumb` using attributes (`BOLD`/`DIM`/`REVERSED`) only.
    pub fn plain() -> Self {
        Self {
            canvas_bg: Color::Reset,
            sidebar_bg: Color::Reset,
            surface_bg: Color::Reset,
            elevated_bg: Color::Reset,
            selected_bg: Color::Reset,
            danger_bg: Color::Reset,
            diff_add_bg: Color::Reset,
            diff_del_bg: Color::Reset,
            border_subtle: Color::Reset,
            border_focus: Color::Reset,
            text_primary: Color::Reset,
            text_secondary: Color::Reset,
            text_muted: Color::Reset,
            text_dim: Color::Reset,
            accent: Color::Reset,
            accent_soft: Color::Reset,
            ok: Color::Reset,
            active: Color::Reset,
            warn: Color::Reset,
            error: Color::Reset,
            selected_fg: Color::Reset,
            rainbow: [Color::Reset; 7],
        }
    }

    pub fn select(no_color: bool, dumb_term: bool) -> Self {
        if no_color || dumb_term {
            Self::plain()
        } else {
            RAINBOW_PALETTE
        }
    }

    pub fn default_for_env() -> Self {
        let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let dumb = env::var("TERM").is_ok_and(|t| t == "dumb");
        Self::select(no_color, dumb)
    }

    pub fn is_plain(&self) -> bool {
        self.accent == Color::Reset
    }

    pub fn rainbow_color(&self, idx: usize) -> Color {
        self.rainbow[idx % self.rainbow.len()]
    }

    pub fn canvas(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.canvas_bg)
    }

    pub fn sidebar(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.sidebar_bg)
    }

    pub fn surface(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.surface_bg)
    }

    pub fn elevated(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.elevated_bg)
    }

    pub fn primary(&self) -> Style {
        Style::default().fg(self.text_primary)
    }

    pub fn secondary(&self) -> Style {
        Style::default().fg(self.text_secondary)
    }

    pub fn muted(&self) -> Style {
        if self.is_plain() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.text_muted)
        }
    }

    pub fn dim(&self) -> Style {
        if self.is_plain() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.text_dim)
        }
    }

    pub fn accent(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn accent_bold(&self) -> Style {
        Style::default().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    pub fn accent_soft(&self) -> Style {
        Style::default().fg(self.accent_soft).add_modifier(Modifier::BOLD)
    }

    pub fn ok(&self) -> Style {
        Style::default().fg(self.ok)
    }

    pub fn active(&self) -> Style {
        Style::default().fg(self.active)
    }

    pub fn warn(&self) -> Style {
        Style::default().fg(self.warn).add_modifier(Modifier::BOLD)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.error)
    }

    pub fn border(&self, focused: bool) -> Style {
        if focused {
            Style::default().fg(self.border_focus)
        } else if self.is_plain() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.border_subtle)
        }
    }

    pub fn selected_row(&self) -> Style {
        if self.selected_bg == Color::Reset {
            Style::default().fg(self.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.selected_fg).bg(self.selected_bg).add_modifier(Modifier::BOLD)
        }
    }

    pub fn danger_row(&self) -> Style {
        if self.danger_bg == Color::Reset {
            Style::default().fg(self.error).add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().fg(self.error).bg(self.danger_bg).add_modifier(Modifier::BOLD)
        }
    }

    pub fn pill(&self, color: Color) -> Style {
        if color == Color::Reset {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(0, 0, 0)).bg(color).add_modifier(Modifier::BOLD)
        }
    }

    pub fn pill_inactive(&self) -> Style {
        if self.is_plain() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.text_muted).bg(self.surface_bg)
        }
    }

    pub fn keycap(&self) -> Style {
        Style::default().fg(self.accent).bg(self.surface_bg).add_modifier(Modifier::BOLD)
    }

    pub fn section_title(&self) -> Style {
        Style::default().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    pub fn diff_add(&self) -> Style {
        Style::default().fg(self.ok).bg(self.diff_add_bg)
    }

    pub fn diff_del(&self) -> Style {
        Style::default().fg(self.error).bg(self.diff_del_bg)
    }

    pub fn status_badge(&self, status: &TaskStatus, tick: u64) -> (&'static str, Style) {
        match status {
            TaskStatus::Queued => ("○", self.dim()),
            TaskStatus::Running => (
                SPINNER[(tick as usize) % SPINNER.len()],
                Style::default().fg(self.rainbow_color((tick as usize) / 2)).add_modifier(Modifier::BOLD),
            ),
            TaskStatus::Waiting => ("⋯", self.active()),
            TaskStatus::Input => ("?", self.accent_soft()),
            TaskStatus::Approval => ("!", self.warn()),
            TaskStatus::Done => ("✓", self.ok()),
            TaskStatus::Failed => ("✗", self.error()),
            TaskStatus::Cancelled => ("⊘", self.muted()),
            TaskStatus::Unknown => ("⚠", self.error().add_modifier(Modifier::BOLD)),
            TaskStatus::Other(_) => ("·", self.muted()),
        }
    }

    pub fn verification_badge(&self, v: &Verification) -> (&'static str, Style) {
        match v {
            Verification::Passed => ("✓ verified", self.ok()),
            Verification::Failed => ("✗ check failed", self.error()),
            Verification::Pending => ("⋯ check pending", self.active()),
            Verification::NotRequested => ("– unchecked", self.muted()),
            Verification::NotRun => ("– check not run", self.muted()),
            Verification::Unknown => ("⚠ check unknown", self.error().add_modifier(Modifier::BOLD)),
            Verification::Other(_) => ("? check", self.muted()),
        }
    }
}

pub(crate) fn truncate_display(text: &str, max_width: usize) -> String {
    let clean = sanitize_inline(text);
    let clean = clean.trim();
    if max_width == 0 {
        return String::new();
    }
    if clean.width() <= max_width {
        return clean.to_owned();
    }
    if max_width == 1 {
        return "…".into();
    }
    let limit = max_width - 1;
    let mut width = 0;
    let mut out = String::new();
    for ch in clean.chars() {
        let w = ch.width().unwrap_or(0);
        if width + w > limit {
            break;
        }
        out.push(ch);
        width += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn truncates_cjk_and_control_sequences_without_exceeding_width() {
        assert_eq!(truncate_display("hello\x1b[31m world", 20), "hello\\u001b[31m wor…");
        let cjk = "你好世界测试文本";
        for max in [0, 1, 2, 5, 6, 10, 20] {
            let t = truncate_display(cjk, max);
            assert!(t.width() <= max, "width {} > {max} for {t:?}", t.width());
        }
        assert_eq!(truncate_display("你好世界", 5), "你好…");
    }

    #[test]
    fn selects_pure_black_rainbow_palette_and_plain_fallback() {
        assert_eq!(Palette::select(false, false), RAINBOW_PALETTE);
        assert_eq!(RAINBOW_PALETTE.canvas_bg, Color::Rgb(0, 0, 0));
        assert_eq!(RAINBOW_PALETTE.sidebar_bg, Color::Rgb(0, 0, 0));
        assert_eq!(RAINBOW_PALETTE.text_primary, Color::Rgb(255, 255, 255));
        let unique: std::collections::HashSet<_> = RAINBOW_PALETTE.rainbow.iter().collect();
        assert_eq!(unique.len(), 7);
        assert_eq!(Palette::select(true, false), Palette::plain());
        assert_eq!(Palette::select(false, true), Palette::plain());
        assert!(Palette::plain().is_plain());
    }
}
