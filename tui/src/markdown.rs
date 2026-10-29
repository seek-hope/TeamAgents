//! D-447: terminal-safe CommonMark presentation. The stored text is never rewritten.
use crate::{
    graphics::{Document, Graphics, Origin, Source},
    theme::*,
    wrap::wrap_line,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Remove terminal control sequences at the display boundary, including OSC clipboard/title commands.
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '_' | '^') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.peek() == Some(&'\\')) {
                            if c == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push_str("    "),
            '\r' if chars.peek() != Some(&'\n') => out.push('\n'),
            '\n' => out.push(c),
            c if !c.is_control() => out.push(c),
            _ => {}
        }
    }
    out
}

pub fn plain(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    clean(text).split('\n').flat_map(|line| wrap_line(&Line::styled(line.to_string(), style), width)).collect()
}

struct Indent {
    first: String,
    rest: String,
}
struct Writer {
    width: usize,
    out: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    indents: Vec<Indent>,
    lists: Vec<Option<u64>>,
    links: Vec<(String, usize)>,
}
impl Writer {
    fn new(width: usize) -> Self {
        Self {
            width,
            out: vec![],
            spans: vec![],
            styles: vec![Style::default().fg(FG)],
            indents: vec![],
            lists: vec![],
            links: vec![],
        }
    }
    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }
    fn text(&mut self, text: impl Into<String>) {
        self.spans.push(Span::styled(text.into(), self.style()));
    }
    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }
    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let first: String = self.indents.iter().map(|p| p.first.as_str()).collect();
        let rest: String = self.indents.iter().map(|p| p.rest.as_str()).collect();
        let indent = unicode_width::UnicodeWidthStr::width(first.as_str());
        // Deep nesting gives content at least one column; the source remains intact.
        let prefix_width = indent.min(self.width.saturating_sub(1));
        let rows = wrap_line(&Line::from(std::mem::take(&mut self.spans)), self.width.saturating_sub(prefix_width));
        for (i, mut row) in rows.into_iter().enumerate() {
            let prefix = if i == 0 { &first } else { &rest };
            let prefix: String = prefix.chars().take(prefix_width).collect();
            row.spans.insert(0, Span::styled(prefix, secondary()));
            self.out.push(row);
        }
        for indent in &mut self.indents {
            indent.first.clone_from(&indent.rest);
        }
    }
    fn blank(&mut self) {
        self.flush();
        if self.out.last().is_some_and(|l| l.width() > 0) {
            self.out.push(Line::default());
        }
    }
    fn inline(&mut self, event: Event<'_>) {
        match event {
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.text(text.to_string()),
            Event::Code(text) => self.spans.push(Span::styled(text.to_string(), self.style().fg(CODE).bg(CHIP_BG))),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Start(Tag::Emphasis) => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strong) => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Strikethrough) => self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT)),
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {
                self.styles.pop();
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                self.links.push((dest_url.to_string(), self.spans.len()));
                self.push_style(Style::default().fg(ACCENT).add_modifier(Modifier::UNDERLINED));
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                self.styles.pop();
                if let Some((url, start)) = self.links.pop() {
                    let label: String = self.spans.iter().skip(start).map(|s| s.content.as_ref()).collect();
                    if label != url {
                        self.text(format!(" ({url})"));
                    }
                }
            }
            Event::TaskListMarker(checked) => self.text(if checked { "[x] " } else { "[ ] " }),
            Event::FootnoteReference(name) => self.text(format!("[{name}]")),
            _ => {}
        }
    }
}

/// Parse complete and partially streamed Markdown with the same renderer.
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    render_media(text, width, None, &Origin::default()).lines
}

/// Only completed fences become diagrams; an unfinished stream retains its source.
pub fn render_media(text: &str, width: usize, mut graphics: Option<&mut Graphics>, origin: &Origin) -> Document {
    if width == 0 {
        return Document::default();
    }
    let text = clean(text);
    let mut pictures = vec![];
    let mut media = vec![];
    let mut events =
        Parser::new_ext(&text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS)
            .into_offset_iter()
            .peekable();
    let mut w = Writer::new(width);
    while let Some((event, range)) = events.next() {
        match event {
            Event::Start(Tag::Paragraph) => w.flush(),
            Event::End(TagEnd::Paragraph) => {
                w.flush();
                if w.lists.is_empty() {
                    w.blank();
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                w.blank();
                let mut style = Style::default().fg(FG).add_modifier(Modifier::BOLD);
                if level == HeadingLevel::H1 {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                w.push_style(style);
            }
            Event::End(TagEnd::Heading(_)) => {
                w.flush();
                w.styles.pop();
                w.blank();
            }
            Event::Start(Tag::BlockQuote(_)) => {
                w.flush();
                w.indents.push(Indent { first: "│ ".into(), rest: "│ ".into() });
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                w.flush();
                w.indents.pop();
                w.blank();
            }
            Event::Start(Tag::List(start)) => {
                w.flush();
                w.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                w.flush();
                w.lists.pop();
                if w.lists.is_empty() {
                    w.blank();
                }
            }
            Event::Start(Tag::Item) => {
                w.flush();
                let marker = match w.lists.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n = n.saturating_add(1);
                        marker
                    }
                    _ => "• ".into(),
                };
                w.indents.push(Indent { rest: " ".repeat(marker.chars().count()), first: marker });
            }
            Event::End(TagEnd::Item) => {
                w.flush();
                w.indents.pop();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                w.blank();
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        lang.split_whitespace().next().unwrap_or("").chars().take(24).collect::<String>()
                    }
                    _ => String::new(),
                };
                let mut code = String::new();
                let mut content_end = range.start;
                let mut end = range.end;
                for (event, span) in events.by_ref() {
                    match event {
                        Event::End(TagEnd::CodeBlock) => {
                            end = span.end;
                            break;
                        }
                        Event::Text(text) => {
                            code.push_str(&text);
                            content_end = span.end;
                        }
                        _ => {}
                    }
                }
                let lang = lang.to_lowercase();
                if matches!(lang.as_str(), "mermaid" | "svg" | "dot" | "graphviz")
                    && closing_fence(&text, range.start, content_end, end)
                {
                    if let Some(graphics) = graphics.as_deref_mut() {
                        let prefix: String = w.indents.iter().map(|p| p.rest.as_str()).collect();
                        let indent =
                            unicode_width::UnicodeWidthStr::width(prefix.as_str()).min(width.saturating_sub(1));
                        let mut doc = graphics.block(Source::Diagram { language: lang, code }, origin, width - indent);
                        doc.offset(w.out.len(), indent);
                        pictures.extend(doc.pictures);
                        media.extend(doc.media);
                        for mut line in doc.lines {
                            line.spans.insert(0, Span::raw(prefix.chars().take(indent).collect::<String>()));
                            w.out.push(line);
                        }
                        w.blank();
                        continue;
                    }
                }
                w.text(format!("╭─ {lang}"));
                w.flush();
                w.indents.push(Indent { first: "│ ".into(), rest: "│ ".into() });
                if closing_fence(&text, range.start, content_end, end)
                    && crate::highlight::eligible(&code, &lang)
                    && graphics.is_some()
                {
                    let prefix: String = w.indents.iter().map(|p| p.rest.as_str()).collect();
                    let indent = unicode_width::UnicodeWidthStr::width(prefix.as_str()).min(width.saturating_sub(1));
                    let mut doc = graphics.as_deref_mut().unwrap().block(
                        Source::Code { language: lang, code },
                        origin,
                        width - indent,
                    );
                    doc.offset(w.out.len(), indent);
                    media.extend(doc.media);
                    for mut line in doc.lines {
                        line.spans
                            .insert(0, Span::styled(prefix.chars().take(indent).collect::<String>(), secondary()));
                        w.out.push(line);
                    }
                } else {
                    for line in crate::highlight::code_lines(&code, &lang) {
                        w.spans.extend(line.spans);
                        w.flush();
                    }
                }
                w.indents.pop();
                w.text("╰─");
                w.flush();
                w.blank();
            }
            Event::Start(Tag::Image { dest_url, .. }) if graphics.is_some() => {
                w.flush();
                let mut alt = String::new();
                for (event, _) in events.by_ref() {
                    match event {
                        Event::End(TagEnd::Image) => break,
                        Event::Text(text) => alt.push_str(&text),
                        _ => {}
                    }
                }
                if !alt.is_empty() {
                    w.text(alt);
                    w.flush();
                }
                let prefix: String = w.indents.iter().map(|p| p.rest.as_str()).collect();
                let indent = unicode_width::UnicodeWidthStr::width(prefix.as_str()).min(width.saturating_sub(1));
                let mut doc = graphics.as_deref_mut().unwrap().block(
                    Source::Image(dest_url.into_string()),
                    origin,
                    width - indent,
                );
                doc.offset(w.out.len(), indent);
                pictures.extend(doc.pictures);
                media.extend(doc.media);
                for mut line in doc.lines {
                    line.spans.insert(0, Span::raw(prefix.chars().take(indent).collect::<String>()));
                    w.out.push(line);
                }
                w.blank();
            }
            Event::Start(Tag::Table(_)) => {
                w.blank();
                let mut rows = Vec::new();
                let mut table_images = Vec::new();
                let mut row = Vec::new();
                let mut cell = Writer::new(usize::MAX);
                for (event, _) in events.by_ref() {
                    match event {
                        Event::End(TagEnd::Table) => break,
                        Event::Start(Tag::Image { link_type, dest_url, title, id }) if graphics.is_some() => {
                            let display = if dest_url.starts_with("data:") {
                                "embedded image".into()
                            } else {
                                dest_url.chars().take(240).collect::<String>()
                            };
                            table_images.push(Source::Image(dest_url.into_string()));
                            cell.inline(Event::Start(Tag::Image { link_type, dest_url: display.into(), title, id }));
                        }
                        Event::Start(Tag::TableCell) => cell = Writer::new(usize::MAX),
                        Event::End(TagEnd::TableCell) => {
                            cell.flush();
                            row.push(std::mem::take(&mut cell.out).into_iter().next().unwrap_or_default());
                        }
                        Event::End(TagEnd::TableRow | TagEnd::TableHead) => rows.push(std::mem::take(&mut row)),
                        other => cell.inline(other),
                    }
                }
                // A table inside a quote/list shares the surrounding indentation.
                let prefix: String = w.indents.iter().map(|p| p.rest.as_str()).collect();
                let indent = unicode_width::UnicodeWidthStr::width(prefix.as_str()).min(width.saturating_sub(1));
                for mut line in table(rows, width - indent) {
                    line.spans.insert(0, Span::styled(prefix.chars().take(indent).collect::<String>(), secondary()));
                    w.out.push(line);
                }
                for source in table_images {
                    let mut doc = graphics.as_deref_mut().unwrap().block(source, origin, width - indent);
                    doc.offset(w.out.len(), indent);
                    pictures.extend(doc.pictures);
                    media.extend(doc.media);
                    for mut line in doc.lines {
                        line.spans.insert(0, Span::raw(prefix.chars().take(indent).collect::<String>()));
                        w.out.push(line);
                    }
                }
                w.blank();
            }
            Event::Rule => {
                w.blank();
                w.text("─".repeat(width.min(32)));
                w.flush();
                w.blank();
            }
            other => w.inline(other),
        }
    }
    w.flush();
    while w.out.last().is_some_and(|l| l.width() == 0) {
        w.out.pop();
    }
    Document { lines: w.out, pictures, media }
}

fn closing_fence(text: &str, start: usize, content_end: usize, end: usize) -> bool {
    let block = &text[start..end];
    let Some(marker @ (b'`' | b'~')) = block.as_bytes().first().copied() else {
        return false;
    };
    let count = block.bytes().take_while(|b| *b == marker).count();
    text[content_end..end].trim_end().bytes().rev().take_while(|b| *b == marker).count() >= count
}

fn table(rows: Vec<Vec<Line<'static>>>, width: usize) -> Vec<Line<'static>> {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return vec![];
    }
    let mut out = Vec::new();
    if width < cols.saturating_mul(10).saturating_sub(3) {
        for row in rows.iter().skip(1) {
            for (i, cell) in row.iter().enumerate() {
                let mut line = rows[0].get(i).cloned().unwrap_or_default();
                for span in &mut line.spans {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                }
                line.spans.push(Span::raw(": "));
                line.spans.extend(cell.spans.clone());
                out.extend(wrap_line(&line, width));
            }
            out.push(Line::default());
        }
        if rows.len() == 1 {
            for cell in &rows[0] {
                out.extend(wrap_line(cell, width));
            }
        }
        return out;
    }
    let budget = width.saturating_sub(3 * (cols - 1));
    let mut widths = (0..cols)
        .map(|i| rows.iter().filter_map(|r| r.get(i)).map(Line::width).max().unwrap_or(1).clamp(1, budget))
        .collect::<Vec<_>>();
    while widths.iter().sum::<usize>() > budget {
        let i = (0..cols).max_by_key(|i| widths[*i]).unwrap();
        widths[i] -= 1;
    }
    for (r, row) in rows.into_iter().enumerate() {
        let cells = (0..cols).map(|i| wrap_line(row.get(i).unwrap_or(&Line::default()), widths[i])).collect::<Vec<_>>();
        for j in 0..cells.iter().map(Vec::len).max().unwrap_or(0) {
            let mut line = Line::default();
            for (i, cell) in cells.iter().enumerate() {
                if i > 0 {
                    line.spans.push(Span::styled(" │ ", secondary()));
                }
                let part = cell.get(j).cloned().unwrap_or_default();
                let padding = widths[i].saturating_sub(part.width());
                line.spans.extend(part.spans.into_iter().map(|mut s| {
                    if r == 0 {
                        s.style = s.style.add_modifier(Modifier::BOLD);
                    }
                    s
                }));
                line.spans.push(Span::raw(" ".repeat(padding)));
            }
            out.push(line);
        }
        if r == 0 {
            out.push(Line::styled(widths.iter().map(|n| "─".repeat(*n)).collect::<Vec<_>>().join("─┼─"), secondary()));
        }
    }
    out
}
