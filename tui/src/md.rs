//! Minimal Markdown → ratatui lines for Leader replies and the live stream.
//! Covers headers, fenced code, inline code, bold, links, lists, hr, quotes.
//! ponytail: not a full CommonMark renderer — upgrade path is the
//! `tui-markdown` crate if Leader output outgrows this subset.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{ACCENT, FG, NOTICE, PANEL_BG};

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

pub fn render(text: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
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
        if let Some(hashes) = trimmed.chars().take_while(|c| *c == '#').count().into() {
            let level: usize = hashes;
            if level >= 1 && level <= 6 && trimmed.len() > level && trimmed.as_bytes()[level] == b' ' {
                let body = &trimmed[level + 1..];
                out.push(Line::from(Span::styled(
                    body.to_string(),
                    Style::default().fg(FG).add_modifier(Modifier::BOLD),
                )));
                continue;
            }
        }
        out.push(Line::from(inline(line)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn headers_bold_code_fence() {
        let lines = render("# Title\nsome **bold** and `code`\n```\nfn main() {}\n```\ntail");
        let texts = plain(&lines);
        assert_eq!(texts[0], "Title");
        assert_eq!(texts[1], "some bold and code");
        assert_eq!(texts[2], " fn main() {}");
        assert_eq!(texts[3], "tail");
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(lines[1].spans[3].style.bg, Some(PANEL_BG)); // inline code
    }

    #[test]
    fn links_and_quotes() {
        let lines = render("see [docs](https://x)\n> quoted");
        let texts = plain(&lines);
        assert_eq!(texts[0], "see docs");
        assert!(texts[1].starts_with("▌ "));
    }

    #[test]
    fn empty_and_hr() {
        let lines = render("a\n\n---\nb");
        let texts = plain(&lines);
        assert_eq!(texts, vec!["a", "", "────────", "b"]);
    }
}
