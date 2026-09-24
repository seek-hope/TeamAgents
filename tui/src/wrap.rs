//! Styled-span word wrapping shared by the v2 UI (extracted from the retired
//! v1 renderer, §14: one owner per behaviour).

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Word-wrap styled spans (Rich fold: break on words, long words split).
/// ponytail: O(lines) rebuild every frame; fine for chat-scale buffers.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::raw("")];
    }
    let mut out: Vec<Line<'static>> = vec![];
    let mut cur: Vec<Span<'static>> = vec![];
    let mut cur_w = 0usize;
    let push_span =
        |text: String, style: Style, cur: &mut Vec<Span<'static>>, cur_w: &mut usize, out: &mut Vec<Line<'static>>| {
            for word in text.split_inclusive(' ') {
                let mut rest = word;
                loop {
                    let ww = UnicodeWidthStr::width(rest);
                    if *cur_w + ww <= width {
                        *cur_w += ww;
                        cur.push(Span::styled(rest.to_string(), style));
                        break;
                    }
                    if *cur_w > 0 && !rest.trim().is_empty() {
                        // Rich never leaves a partial word on the current row: a word
                        // starts on a fresh line (and is folded there if still too long)
                        out.push(Line::from(std::mem::take(cur)));
                        *cur_w = 0;
                        rest = rest.trim_start();
                        continue;
                    }
                    // longer than a full row (or row partly full): split by chars
                    let mut take = String::new();
                    let mut take_w = 0usize;
                    for ch in rest.chars() {
                        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                        if *cur_w + take_w + cw > width {
                            break;
                        }
                        take.push(ch);
                        take_w += cw;
                    }
                    if take.is_empty() && *cur_w == 0 {
                        take.push(rest.chars().next().unwrap_or(' ')); // never lose a char
                    }
                    cur.push(Span::styled(take.clone(), style));
                    *cur_w += take_w;
                    if *cur_w >= width {
                        out.push(Line::from(std::mem::take(cur)));
                        *cur_w = 0;
                    }
                    rest = &rest[take.len()..];
                    if rest.is_empty() {
                        break;
                    }
                }
            }
        };
    for span in &line.spans {
        push_span(span.content.to_string(), span.style, &mut cur, &mut cur_w, &mut out);
    }
    out.push(Line::from(cur));
    out
}

pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines.iter().flat_map(|l| wrap_line(l, width)).collect()
}
