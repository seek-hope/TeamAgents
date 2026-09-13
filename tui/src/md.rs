//! Minimal Markdown → ratatui lines for Leader replies and the live stream.
//! Covers headers, fenced code, inline code, bold, links, lists, hr, quotes.
//! ponytail: not a full CommonMark renderer — upgrade path is the
//! `tui-markdown` crate if Leader output outgrows this subset.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{ACCENT, FG, NOTICE, PANEL_BG};
use unicode_width::UnicodeWidthStr;

/// Parse inline spans: `code`, **bold**, [text](url).
fn inline(text: &str) -> Vec<Span<'static>> {
    let code_style = Style::default().fg(FG).bg(PANEL_BG);
    let bold_style = Style::default().fg(FG).add_modifier(Modifier::BOLD);
    let link_style = Style::default().fg(FG).add_modifier(Modifier::UNDERLINED);
    let plain = Style::default().fg(FG);
    let mut spans = Vec::new();
    let mut rest = text.to_string();
    let mut buf = String::new();
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), plain));
            }
        };
    }
    while !rest.is_empty() {
        if let Some(s) = rest.strip_prefix("`") {
            if let Some(end) = s.find('`') {
                flush!();
                spans.push(Span::styled(s[..end].to_string(), code_style));
                rest = s[end + 1..].to_string();
                continue;
            }
        } else if let Some(s) = rest.strip_prefix("**") {
            if let Some(end) = s.find("**") {
                flush!();
                spans.push(Span::styled(s[..end].to_string(), bold_style));
                rest = s[end + 2..].to_string();
                continue;
            }
        } else if let Some(s) = rest.strip_prefix('[') {
            if let Some(close) = s.find("](") {
                if let Some(end) = s[close + 2..].find(')') {
                    flush!();
                    spans.push(Span::styled(s[..close].to_string(), link_style));
                    rest = s[close + 2 + end + 1..].to_string();
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        rest = rest[ch.len_utf8()..].to_string();
    }
    flush!();
    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    spans
}

/// Rich `Markdown` baseline (tui/panels.py renders Leader replies with it):
/// H1 centred+underlined, other headings bold, lists as " • item" / " 1 item"
/// with 3-space nesting steps, block quotes "▌ text", rules a full `-` row.
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    let mut fence_started = false;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim_start().starts_with("```") {
            if in_fence && fence_started {
                out.push(Line::raw("")); // Rich puts a blank line after a code block
            }
            in_fence = !in_fence;
            fence_started = false;
            continue;
        }
        if in_fence {
            if !fence_started {
                out.push(Line::raw(""));
                fence_started = true;
            }
            out.push(Line::from(Span::styled(
                format!(" {line}"),
                Style::default().fg(FG).bg(PANEL_BG),
            )));
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(Line::raw(""));
            continue;
        }
        if trimmed.chars().all(|c| c == '-' || c == '_' || c == '*') && trimmed.len() >= 3 {
            out.push(Line::from(Span::styled("─".repeat(8), Style::default().fg(NOTICE))));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let mut spans = vec![Span::styled("▌ ", Style::default().fg(ACCENT))];
            spans.extend(inline(rest.trim()).into_iter().map(|s| {
                Span::styled(s.content.into_owned(), s.style.fg(NOTICE))
            }));
            out.push(Line::from(spans));
            continue;
        }
        let level = line.chars().take_while(|c| *c == ' ').count() / 2;
        let indent = " ".repeat(1 + 3 * level);
        if let Some(body) = bullet_body(trimmed) {
            let mut spans = vec![Span::styled(format!("{indent}• "), Style::default().fg(FG))];
            spans.extend(inline(body));
            out.push(Line::from(spans));
            continue;
        }
        if let Some((number, body)) = ordered_body(trimmed) {
            let mut spans = vec![Span::styled(format!("{indent}{number} "), Style::default().fg(FG))];
            spans.extend(inline(body));
            out.push(Line::from(spans));
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            let body = inline(&trimmed[hashes + 1..]);
            let heading: Vec<Span> = body
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style.add_modifier(Modifier::BOLD)))
                .collect();
            let line = if hashes == 1 {
                let text_w: usize = heading.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
                let pad = width.saturating_sub(text_w) / 2;
                let mut spans = vec![Span::raw(" ".repeat(pad))];
                spans.extend(heading);
                Line::from(spans)
            } else {
                Line::from(heading)
            };
            out.push(line);
            if !out.last().map(|l| l.spans.is_empty()).unwrap_or(true) {
                out.push(Line::raw(""));
            }
            continue;
        }
        out.push(Line::from(inline(line)));
    }
    out
}

/// "- item" / "* item" / "+ item" → the body (Rich draws "•").
fn bullet_body(trimmed: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest.trim_end());
        }
    }
    None
}

/// "1. item" / "1) item" → (number, body); Rich drops the dot.
fn ordered_body(trimmed: &str) -> Option<(String, &str)> {
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &trimmed[digits.len()..];
    let body = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    Some((digits, body.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn headers_bold_code_fence() {
        let lines = render("# Title\nsome **bold** and `code`\n```\nfn main() {}\n```\ntail", 40);
        let texts = plain(&lines);
        // Rich baseline: H1 centred + bold, blank after headings, blanks around code
        assert_eq!(texts[0].trim(), "Title");
        assert_eq!(texts[1], "");
        assert_eq!(texts[2], "some bold and code");
        assert_eq!(texts[3], "");
        assert_eq!(texts[4], " fn main() {}");
        assert_eq!(texts[5], "");
        assert_eq!(texts[6], "tail");
        let title_span = lines[0].spans.iter().find(|s| !s.content.trim().is_empty()).unwrap();
        assert!(title_span.style.add_modifier.contains(Modifier::BOLD));
        let code_line = lines.iter().find(|l| l.spans.iter().any(|s| s.content.contains("fn main"))).unwrap();
        assert!(code_line.spans.iter().any(|s| s.style.bg == Some(PANEL_BG)));
    }

    #[test]
    fn links_and_quotes() {
        let lines = render("see [docs](https://x)\n> quoted", 40);
        let texts = plain(&lines);
        assert_eq!(texts[0], "see docs");
        assert!(texts[1].starts_with("▌ "));
    }

    #[test]
    fn empty_and_hr() {
        let lines = render("a\n\n---\nb", 40);
        let texts = plain(&lines);
        assert_eq!(texts, vec!["a", "", "────────", "b"]);
        // Rich baseline: " • item" / " 1 item", nested lists step by 3 spaces
        let lists = render("- one\n  - nested\n1. first", 40);
        let texts: Vec<String> = lists
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec![" • one", "    • nested", " 1 first"]);
    }
}
