//! D-447: styled grapheme wrapping shared by transcript, Markdown and session history.
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Word wrap without splitting a grapheme or losing span/line styles.
/// An indivisible glyph wider than the entire viewport uses a display-only replacement.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }
    let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
    let mut styles = Vec::new();
    let mut offset = 0;
    for span in &line.spans {
        offset += span.content.len();
        styles.push((offset, line.style.patch(span.style)));
    }
    let mut style_index = 0;
    let mut tokens: Vec<Vec<(String, Style, usize)>> = Vec::new();
    let mut previous_space = false;
    for (offset, grapheme) in text.grapheme_indices(true) {
        while style_index + 1 < styles.len() && offset >= styles[style_index].0 {
            style_index += 1;
        }
        let space = grapheme.chars().all(char::is_whitespace);
        if tokens.is_empty() || space != previous_space {
            tokens.push(vec![]);
        }
        previous_space = space;
        let size = UnicodeWidthStr::width(grapheme);
        let (grapheme, size) = if size > width { ("�", 1) } else { (grapheme, size) };
        tokens.last_mut().unwrap().push((grapheme.into(), styles[style_index].1, size));
    }
    let mut out = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for token in tokens {
        let size: usize = token.iter().map(|c| c.2).sum();
        let space = token.iter().all(|c| c.0.chars().all(char::is_whitespace));
        if !space && used > 0 && size > width - used {
            out.push(Line::from(std::mem::take(&mut row)).style(line.style));
            used = 0;
        }
        for (text, style, size) in token {
            if used + size > width {
                out.push(Line::from(std::mem::take(&mut row)).style(line.style));
                used = 0;
                if space {
                    continue;
                }
            }
            if let Some(last) = row.last_mut().filter(|last| last.style == style) {
                last.content.to_mut().push_str(&text);
            } else {
                row.push(Span::styled(text, style));
            }
            used += size;
        }
    }
    if !row.is_empty() || out.is_empty() {
        out.push(Line::from(row).style(line.style));
    }
    out
}
