//! ratatui rendering — layout mirrors app.py::CSS:
//! status(1) / body(side tabs 2fr over chat 3fr) / footer(1).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{panel_tab_label, App, Cell, Focus, Severity, PANELS};
use crate::i18n::{table_headers, tr};
use crate::md;
use crate::theme::*;

fn style_named(name: &str) -> Style {
    match name {
        "accent" => Style::default().fg(ACCENT),
        "warning" => Style::default().fg(WARNING),
        "error" => Style::default().fg(ERROR),
        "success" => Style::default().fg(SUCCESS),
        _ => Style::default().fg(NOTICE),
    }
}

/// Word-wrap styled spans (Rich fold: break on words, long words split).
/// ponytail: O(lines) rebuild every frame; fine for chat-scale buffers.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::raw("")];
    }
    let mut out: Vec<Line<'static>> = vec![];
    let mut cur: Vec<Span<'static>> = vec![];
    let mut cur_w = 0usize;
    let push_span = |text: String, style: Style, cur: &mut Vec<Span<'static>>, cur_w: &mut usize, out: &mut Vec<Line<'static>>| {
        for word in text.split_inclusive(' ') {
            let mut rest = word;
            loop {
                let ww = UnicodeWidthStr::width(rest);
                if *cur_w + ww <= width {
                    *cur_w += ww;
                    cur.push(Span::styled(rest.to_string(), style));
                    break;
                }
                if *cur_w > 0 && !rest.trim().is_empty() && ww <= width {
                    out.push(Line::from(std::mem::take(cur)));
                    *cur_w = 0;
                    if rest.starts_with(' ') {
                        rest = rest.trim_start();
                    }
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

// ------------------------------------------------------------------ chat log

/// ChatLog::_render_entry — one chat entry to styled (unwrapped) lines.
pub fn chat_entry_lines(lang: &str, who: &str, text: &str) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(GREY).add_modifier(Modifier::BOLD);
    let (mut label_style, mut body_style) = match who {
        "user" | "Leader" => (label_style, Style::default().fg(FG)),
        "system" => (label_style, Style::default().fg(NOTICE)),
        w if w.starts_with("system") => (label_style, Style::default().fg(NOTICE)),
        _ => (label_style, Style::default().fg(FG)),
    };
    if who.starts_with('✗') || who.starts_with('⚠') {
        label_style = Style::default().fg(ERROR);
        body_style = Style::default().fg(ERROR);
    } else if who.ends_with(&tr(lang, "（完成）", &[])) {
        label_style = Style::default().fg(SUCCESS);
    }
    let mut lines = vec![];
    if !who.is_empty() {
        let prefix = if matches!(who, "user" | "你" | "You") { "›" } else { "•" };
        let label = if matches!(who, "user" | "你" | "You") {
            tr(lang, "你", &[])
        } else if who == "system" {
            tr(lang, "系统", &[])
        } else {
            who.to_string()
        };
        lines.push(Line::from(Span::styled(format!("{prefix} {label}"), label_style)));
    }
    if who == "Leader" {
        lines.extend(md::render(text));
        lines.push(Line::raw(""));
        return lines;
    }
    let body_lines: Vec<&str> = text.split('\n').collect();
    for line in if body_lines.is_empty() { vec![""] } else { body_lines } {
        if line.starts_with('✗') || line.starts_with('⚠') {
            lines.push(Line::from(Span::styled(line.to_string(), Style::default().fg(ERROR))));
        } else {
            lines.push(Line::from(Span::styled(line.to_string(), body_style)));
        }
    }
    lines.push(Line::raw(""));
    lines
}

// ------------------------------------------------------------------ tables

fn col_widths(header: &[&str], rows: &[Vec<Cell>], avail: usize) -> Vec<usize> {
    let n = header.len();
    let mut w: Vec<usize> = (0..n)
        .map(|i| {
            let mut m = UnicodeWidthStr::width(header[i]);
            for row in rows {
                if let Some(c) = row.get(i) {
                    m = m.max(UnicodeWidthStr::width(c.0.as_str()));
                }
            }
            m.clamp(3, 60)
        })
        .collect();
    let sep = 2 * (n - 1);
    while w.iter().sum::<usize>() + sep > avail {
        let Some((i, _)) = w.iter().enumerate().max_by_key(|(_, v)| *v) else { break };
        if w[i] <= 3 {
            break;
        }
        w[i] -= 1;
    }
    w
}

fn ellipsize(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        let pad = width - UnicodeWidthStr::width(text);
        return format!("{text}{:pad$}", "", pad = pad);
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    format!("{out}…{:pad$}", "", pad = width.saturating_sub(w + 1))
}

/// A DataTable: bold muted header on panel bg, accent-40% cursor row.
fn render_table(frame: &mut Frame, area: Rect, header: &[&str], rows: &[Vec<Cell>], sel: Option<usize>, focused: bool) {
    if area.height == 0 {
        return;
    }
    let widths = col_widths(header, rows, area.width as usize);
    let header_line: Vec<Span> = header
        .iter()
        .enumerate()
        .map(|(i, h)| {
            Span::styled(
                ellipsize(h, widths[i]),
                Style::default().fg(GREY).bg(PANEL_BG).add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>()
        .join(Span::styled("  ", Style::default().bg(PANEL_BG)));
    frame.render_widget(Paragraph::new(Line::from(header_line)), Rect { height: 1, ..area });
    let body = Rect { y: area.y + 1, height: area.height - 1, ..area };
    let mut lines: Vec<Line> = vec![];
    for (i, row) in rows.iter().enumerate() {
        let selected = sel == Some(i);
        let bg = if selected { CURSOR_ROW } else { BG };
        let spans: Vec<Span> = row
            .iter()
            .enumerate()
            .map(|(j, (text, style))| {
                let base = match style {
                    Some(name) => style_named(name),
                    None => Style::default().fg(FG),
                };
                Span::styled(ellipsize(text, widths[j]), base.bg(bg))
            })
            .collect::<Vec<_>>()
            .join(Span::styled("  ", Style::default().bg(bg)));
        lines.push(Line::from(spans));
    }
    let _ = focused;
    frame.render_widget(Paragraph::new(lines), body);
}

trait JoinSpans {
    fn join(self, sep: Span<'static>) -> Vec<Span<'static>>;
}
impl JoinSpans for Vec<Span<'static>> {
    fn join(mut self, sep: Span<'static>) -> Vec<Span<'static>> {
        let mut out = vec![];
        for (i, s) in self.drain(..).enumerate() {
            if i > 0 {
                out.push(sep.clone());
            }
            out.push(s);
        }
        out
    }
}

// ------------------------------------------------------------------ render

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(ratatui::widgets::Clear, area);
    let root = Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).split(area);
    render_status(frame, app, root[0]);
    render_body(frame, app, root[1]);
    render_footer(frame, app, root[2]);
    render_toasts(frame, app, root[0]);
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let text = app.status_bar();
    let line = Line::from(Span::styled(text, Style::default().fg(NOTICE).bg(PANEL_BG))).centered();
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(PANEL_BG)),
        area,
    );
}

fn render_body(frame: &mut Frame, app: &mut App, area: Rect) {
    // #side 2fr min 6, #chat 3fr min 9 (Textual fr on the body height)
    let side_h = ((area.height as usize) * 2 / 5).max(6).min(area.height as usize) as u16;
    let split = Layout::vertical([Constraint::Length(side_h), Constraint::Fill(1)]).split(area);
    render_side(frame, app, split[0]);
    render_chat(frame, app, split[1]);
}

fn render_side(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 3 {
        return;
    }
    // ContentTabs: one blank row, one label row, then content; #side has a
    // solid $secondary bottom border.
    let tabs_row = Rect { y: area.y + 1, height: 1, ..area };
    let mut spans: Vec<Span> = vec![];
    for (i, _) in PANELS.iter().enumerate() {
        let label = panel_tab_label(app.lang, i);
        let style = if i == app.panel {
            Style::default().fg(FG).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(GREY)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_row);
    let content = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(3), // leave the bottom border line
        ..area
    };
    render_panel(frame, app, content);
    let border_y = area.y + area.height - 1;
    let border = Line::from(Span::styled("─".repeat(area.width as usize), Style::default().fg(GREY)));
    frame.render_widget(Paragraph::new(border), Rect { x: area.x, y: border_y, width: area.width, height: 1 });
}

fn hint_line(app: &App, id: &str) -> String {
    tr(app.lang, id, &[])
}

fn render_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let panel = PANELS[app.panel];
    let focused = app.focus == Focus::Panel;
    match panel {
        "team" | "tasks" | "approvals" | "sessions" => {
            let (header, rows, hint): (&[&str], Vec<(String, Vec<Cell>)>, &str) = match panel {
                "team" => (table_headers("team"), app.team_rows(), "高亮成员=筛选日志 · Enter 取消筛选"),
                "tasks" => (table_headers("tasks"), app.tasks_rows(), "c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）"),
                "approvals" => (table_headers("approvals"), app.approvals_rows(), "待批准操作：a=本次批准  s=会话内批准  d=拒绝"),
                _ => (table_headers("sessions"), app.sessions_rows(), "本目录会话：s=切换  n=新建  a=归档  d=删除（再按 d 确认，删当前会话后退出）"),
            };
            let header: Vec<String> = header.iter().map(|h| tr(app.lang, h, &[])).collect();
            let header_refs: Vec<&str> = header.iter().map(|s| s.as_str()).collect();
            let saved = app.table_cursors.get(panel).cloned().unwrap_or((None, 0));
            let sel = saved.0
                .and_then(|k| rows.iter().position(|(rk, _)| rk == &k))
                .unwrap_or_else(|| saved.1.min(rows.len().saturating_sub(1)));
            let sel = if rows.is_empty() { None } else { Some(sel) };
            let table_area = Rect { height: area.height.saturating_sub(1), ..area };
            render_table(frame, table_area, &header_refs, &rows.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(), sel, focused);
            let hint_y = area.y + area.height - 1;
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(hint_line(app, hint), Style::default().fg(NOTICE)))),
                Rect { x: area.x, y: hint_y, width: area.width, height: 1 },
            );
        }
        "shared" => {
            let header: Vec<String> = table_headers("shared").iter().map(|h| tr(app.lang, h, &[])).collect();
            let header_refs: Vec<&str> = header.iter().map(|s| s.as_str()).collect();
            let rows = app.shared_rows();
            render_table(frame, area, &header_refs, &rows, None, false);
        }
        "log" => {
            let body = Rect { height: area.height.saturating_sub(1), ..area };
            let lines = wrap_lines(
                app.log_lines.iter().map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(NOTICE)))).collect(),
                body.width as usize,
            );
            render_scrolled(frame, body, lines);
            let title_y = area.y + area.height - 1;
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(app.log_title(), Style::default().fg(NOTICE)))),
                Rect { x: area.x, y: title_y, width: area.width, height: 1 },
            );
        }
        "settings" => render_settings(frame, app, area),
        _ => {}
    }
}

fn render_settings(frame: &mut Frame, app: &App, area: Rect) {
    let mut y = area.y;
    let row_h = 3u16;
    // language row
    let label_style = Style::default().fg(FG);
    let sel_style = Style::default().bg(CURSOR_ROW);
    let focused = app.focus == Focus::Panel;
    let lang_value = if app.lang == "zh-CN" { "中文" } else { "English" };
    let line1 = Line::from(vec![
        Span::styled(format!("{:22}", tr(app.lang, "界面语言", &[])), label_style),
        Span::styled(format!(" {lang_value} "), if focused && app.settings_row == 0 { sel_style } else { Style::default().fg(FG) }),
    ]);
    frame.render_widget(Paragraph::new(line1), Rect { x: area.x, y: y + 1, width: area.width, height: 1 });
    y += row_h;
    let switch = if app.animations { "■ on" } else { "□ off" };
    let line2 = Line::from(vec![
        Span::styled(format!("{:22}", tr(app.lang, "动效", &[])), label_style),
        Span::styled(format!(" {switch} "), if focused && app.settings_row == 1 { sel_style } else { Style::default().fg(FG) }),
    ]);
    frame.render_widget(Paragraph::new(line2), Rect { x: area.x, y: y + 1, width: area.width, height: 1 });
    y += row_h;
    let body: Vec<Line> = app
        .settings_lines()
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(NOTICE))))
        .collect();
    frame.render_widget(
        Paragraph::new(body),
        Rect { x: area.x, y, width: area.width, height: area.y + area.height - y },
    );
    if app.lang_open {
        // dropdown under the language value
        let opts = ["English", "中文"];
        let width = 12u16;
        let x = area.x + 22;
        let dy = area.y + 2;
        for (i, o) in opts.iter().enumerate() {
            let style = if i == app.lang_choice { sel_style } else { Style::default().fg(FG).bg(PANEL_BG) };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(format!(" {o:9}"), style))),
                Rect { x, y: dy + i as u16, width, height: 1 },
            );
        }
    }
}

fn render_chat(frame: &mut Frame, app: &mut App, area: Rect) {
    let live_lines = if app.stream_text.is_empty() { vec![] } else { md::render(&app.stream_text) };
    let live_h = (live_lines.len() as u16).min(8);
    let composer_w = area.width as usize;
    let composer_text_w = composer_w.saturating_sub(4).max(1);
    let composer_h = app.composer.widget_height(composer_text_w) as u16;
    let chunks = Layout::vertical([
        Constraint::Length(2),          // #activity
        Constraint::Fill(1),            // #chat-log
        Constraint::Length(live_h),     // #chat-live
        Constraint::Length(1),          // #composer-status
        Constraint::Length(composer_h), // #composer
        Constraint::Length(1),          // #composer-hint
    ])
    .split(area);

    // activity
    let (summary, color, latest) = app.activity_lines();
    let activity = vec![
        Line::from(Span::styled(summary, style_named(color))),
        Line::from(Span::styled(latest, Style::default().fg(NOTICE))),
    ];
    frame.render_widget(Paragraph::new(activity), chunks[0]);

    // chat log
    let log_area = chunks[1];
    let mut lines: Vec<Line<'static>> = vec![];
    for (who, text) in &app.chat {
        lines.extend(chat_entry_lines(app.lang, who, text));
    }
    let wrapped = wrap_lines(lines, log_area.width as usize);
    render_scrolled(frame, log_area, wrapped);

    // live stream preview
    if live_h > 0 {
        let wrapped = wrap_lines(live_lines, chunks[2].width as usize);
        render_scrolled(frame, chunks[2], wrapped);
    }

    // composer status + hint
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(app.composer_status(), Style::default().fg(NOTICE)))),
        chunks[3],
    );
    render_composer(frame, app, chunks[4]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            tr(app.lang, "Enter 发送 · Shift+Enter / Ctrl+J 换行 · ↑↓ 历史 · Esc 停止 Leader", &[]),
            Style::default().fg(NOTICE),
        ))),
        chunks[5],
    );
}

fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    // #composer: panel bg; "›" prefix at (0, 1); #prompt padding 1
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(PANEL_BG)),
        area,
    );
    if area.height < 3 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("›", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))),
        Rect { x: area.x, y: area.y + 1, width: 1, height: 1 },
    );
    let text_x = area.x + 3; // prefix width 2 + prompt padding-left 1
    let text_w = (area.width as usize).saturating_sub(4).max(1);
    let text_y = area.y + 1;
    let visible_h = (area.height as usize).saturating_sub(2);
    // wrap each logical line; cursor position in visual coords
    let mut visual: Vec<String> = vec![];
    let mut cursor_v = (0usize, 0usize); // (row, col-in-row)
    for (r, line) in app.composer.lines.iter().enumerate() {
        let s: String = line.iter().collect();
        if s.is_empty() {
            if r == app.composer.row {
                cursor_v = (visual.len(), 0);
            }
            visual.push(String::new());
            continue;
        }
        let chars: Vec<char> = s.chars().collect();
        let mut start = 0usize;
        loop {
            let mut w = 0usize;
            let mut end = start;
            while end < chars.len() {
                let cw = UnicodeWidthChar::width(chars[end]).unwrap_or(0);
                if w + cw > text_w {
                    break;
                }
                w += cw;
                end += 1;
            }
            if end == start {
                end += 1;
            }
            if r == app.composer.row && app.composer.col >= start && app.composer.col <= end {
                cursor_v = (visual.len(), app.composer.col - start);
            }
            visual.push(chars[start..end].iter().collect());
            start = end;
            if start >= chars.len() {
                break;
            }
        }
    }
    let scroll = if cursor_v.0 + 1 > visible_h { cursor_v.0 + 1 - visible_h } else { 0 };
    let visible: Vec<Line> = visual
        .iter()
        .skip(scroll)
        .take(visible_h)
        .map(|s| Line::from(Span::styled(s.clone(), Style::default().fg(FG).bg(PANEL_BG))))
        .collect();
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().bg(PANEL_BG)),
        Rect { x: text_x, y: text_y, width: text_w as u16, height: visible_h as u16 },
    );
    if app.focus == Focus::Composer {
        let cx = text_x as usize + cursor_v.1.min(text_w.saturating_sub(1));
        let cy = text_y as usize + cursor_v.0.saturating_sub(scroll).min(visible_h.saturating_sub(1));
        frame.set_cursor_position((cx as u16, cy as u16));
    }
}

/// RichLog pinned to the bottom.
fn render_scrolled(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let total = lines.len();
    let offset = total.saturating_sub(area.height as usize) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), area);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let keys: [(&str, &str); 9] = [
        ("ctrl+q", "退出"),
        ("ctrl+p", "暂停/继续"),
        ("ctrl+r", "刷新"),
        ("ctrl+f", "全自动"),
        ("ctrl+t", "切换面板"),
        ("ctrl+g", "批准"),
        ("esc", "停止 Leader"),
        ("ctrl+n", "输入"),
        ("ctrl+j", "换行"),
    ];
    let mut spans: Vec<Span> = vec![];
    for (key, label) in keys {
        spans.push(Span::styled(format!(" {key} "), Style::default().fg(FG).bg(CURSOR_ROW)));
        spans.push(Span::styled(format!(" {}  ", tr(app.lang, label, &[])), Style::default().fg(NOTICE).bg(PANEL_BG)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(PANEL_BG)),
        area,
    );
}

fn render_toasts(frame: &mut Frame, app: &mut App, status_area: Rect) {
    let now = std::time::Instant::now();
    let visible: Vec<&crate::app::Toast> = app.toasts.iter().filter(|t| t.until > now).collect();
    if visible.is_empty() {
        return;
    }
    let width = frame.area().width;
    let mut y = status_area.y + 1;
    for toast in visible.iter().take(3) {
        let color = match toast.severity {
            Severity::Warning => WARNING,
            Severity::Error => ERROR,
            Severity::Info => FG,
        };
        let text_w = UnicodeWidthStr::width(toast.text.as_str()).min(60);
        let w = (text_w + 4) as u16;
        let x = width.saturating_sub(w + 1);
        let lines = wrap_line(&Line::from(Span::styled(toast.text.clone(), Style::default().fg(color))), text_w);
        let h = (lines.len() + 2) as u16;
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(color))
            .style(Style::default().bg(BG));
        let area = Rect { x, y, width: w, height: h };
        frame.render_widget(ratatui::widgets::Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
        y += h;
    }
}
