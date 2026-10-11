//! D-450: explicit-language coloring with plain-source fallback and bounded parser input.
use crate::theme::*;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::sync::OnceLock;
use syntect::{easy::HighlightLines, highlighting::Theme, parsing::SyntaxSet, util::LinesWithEndings};

pub(crate) fn code_lines(code: &str, language: &str) -> Vec<Line<'static>> {
    code.strip_suffix('\n')
        .unwrap_or(code)
        .split('\n')
        .map(|line| {
            let color = if matches!(language, "diff" | "patch") {
                match line.chars().next() {
                    Some('+') => ADDED,
                    Some('-') => ERROR,
                    Some('@') => ACCENT,
                    _ => FG,
                }
            } else {
                CODE
            };
            Line::from(Span::styled(line.to_string(), Style::default().fg(color).bg(PANEL_BG)))
        })
        .collect()
}

pub(crate) fn eligible(code: &str, language: &str) -> bool {
    !language.is_empty()
        && !matches!(language, "diff" | "patch")
        && code.len() <= 64 * 1024
        && code.lines().count() <= 500
        && code.lines().all(|line| line.len() <= 4096)
}

/// Called only inside the isolated display helper.
pub(crate) fn highlighted(code: &str, language: &str) -> Option<Vec<Line<'static>>> {
    if !eligible(code, language) {
        return None;
    }
    static SYNTAX: OnceLock<SyntaxSet> = OnceLock::new();
    static THEME: OnceLock<Theme> = OnceLock::new();
    let set = SYNTAX.get_or_init(two_face::syntax::extra_newlines);
    let language = match language {
        "shell" | "sh" | "bash" => "Bourne Again Shell (bash)",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "py" => "Python",
        "rs" => "Rust",
        other => other,
    };
    let syntax = set.find_syntax_by_token(language)?;
    if std::ptr::eq(syntax, set.find_syntax_plain_text()) {
        return None;
    }
    let theme =
        THEME.get_or_init(|| two_face::theme::extra()[two_face::theme::EmbeddedThemeName::Base16OceanDark].clone());
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = vec![];
    // The parent enforces a wall deadline and kills this process inside a costly regex call.
    for line in LinesWithEndings::from(code) {
        let spans = highlighter
            .highlight_line(line, set)
            .ok()?
            .into_iter()
            .map(|(style, text)| {
                let fg = style.foreground;
                Span::styled(
                    text.trim_end_matches('\n').to_string(),
                    Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b)).bg(PANEL_BG),
                )
            })
            .collect::<Vec<_>>();
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    Some(lines)
}
