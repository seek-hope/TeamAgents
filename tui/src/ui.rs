//! ratatui rendering — Rust-native shell:
//! status chips / responsive body (chat + sidebar box) / dim footer.
//! The companion pane keeps the terminal-native feel: one accent, subtle
//! surfaces, scrollbars only where content overflows.

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

fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

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

// ------------------------------------------------------------------ chat log

/// ChatLog::_render_entry — one chat entry to styled (unwrapped) lines.
pub fn chat_entry_lines(lang: &str, who: &str, text: &str, width: usize) -> Vec<Line<'static>> {
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
        lines.extend(md::render(text, width));
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
    let w: Vec<usize> = (0..n)
        .map(|i| {
            let mut m = UnicodeWidthStr::width(header[i]);
            for row in rows {
                if let Some(c) = row.get(i) {
                    m = m.max(UnicodeWidthStr::width(c.0.as_str()));
                }
            }
            m.clamp(3, 100)
        })
        .collect();
    // DataTable keeps natural widths and scrolls horizontally; long rows are
    // clipped at the viewport edge rather than squeezing columns.
    let _ = avail;
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
//
//   status(1)  chips on the right
//   body: wide → chat | sidebar(box)      narrow → sidebar(box) over chat
//   footer(1)  dim keys, focus label on the right

/// One source of truth for the shell's rectangles, shared by the renderer and
/// the mouse hit-test. The shell is always a top/bottom split (status row, panel
/// box, chat, footer): the pane keys stay put no matter how wide the terminal is.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub status: Rect,
    /// chat area (without the status line)
    pub chat: Rect,
    /// the panel box, top border included
    pub side: Rect,
    /// the box interior (`Block::inner`) — tab strip, table and hit-test share it
    pub side_inner: Rect,
    pub footer: Rect,
    /// first row of the tab strip (inside the box)
    pub tabs_y: u16,
    /// the panel table's body rows (below the rule, above the hint). A click
    /// maps a screen row to a table row through this rect, so its height must
    /// match what `render_table` draws — including the wrapped hint rows.
    pub rows: Rect,
}

/// The hint line pinned to the bottom of the panel box (message id; translated
/// on render). Its wrapped height is part of the table geometry.
pub fn panel_hint(panel: &str) -> &'static str {
    match panel {
        "team" => "高亮成员=筛选日志 · Enter 取消筛选",
        "tasks" => "c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）",
        "approvals" => "待批准操作：a=本次批准  s=会话内批准  d=拒绝",
        "sessions" => "本目录会话：s=切换  n=新建  a=归档  d=删除（再按 d 确认，删当前会话后退出）",
        "shared" => "共享空间条目：作者 / 类型 / 内容或引用",
        "log" => "↑↓ 选择成员筛选 · Enter 取消 · PgUp/PgDn 滚动",
        _ => "高亮成员=筛选日志 · Enter 取消筛选",
    }
}

/// Display rows the panel hint wraps to (same wrap the renderer uses).
fn hint_rows(app: &App, inner: Rect) -> usize {
    let hint = tr(app.lang, panel_hint(PANELS[app.panel]), &[]);
    wrap_lines(
        vec![Line::from(hint)],
        (inner.width as usize).saturating_sub(1).max(1),
    )
    .len()
}

pub fn geometry(app: &App, area: Rect) -> Geometry {
    let footer = Rect { y: area.height.saturating_sub(1), height: 1, ..area };
    let body = Rect { height: area.height.saturating_sub(1), ..area };
    let status = Rect { height: 1, ..body };
    let stacked = Rect { y: body.y + 1, height: body.height.saturating_sub(1), ..body };
    let side_h = ((stacked.height as usize) * 2 / 5).max(6).min(stacked.height as usize) as u16;
    let side = Rect { height: side_h, ..stacked };
    let chat = Rect {
        y: stacked.y + side_h,
        height: stacked.height.saturating_sub(side_h),
        ..stacked
    };
    let side_inner = Rect {
        x: side.x + 1,
        y: side.y + 1,
        width: side.width.saturating_sub(2),
        height: side.height.saturating_sub(2),
    };
    // `render_sidebar` draws the table only when the box has room (width ≥ 10,
    // inner height ≥ 4); the row rect is empty when it does not.
    let rows = if side.width >= 10 && side.height >= 6 {
        let hint_h = (hint_rows(app, side_inner) as u16).min(side_inner.height.saturating_sub(2));
        Rect {
            y: side.y + 5, // border, tab row, divider, header, rule
            height: side.height.saturating_sub(6).saturating_sub(hint_h),
            ..side
        }
    } else {
        Rect { y: side.y + 5, height: 0, ..side }
    };
    Geometry { status, chat, side, side_inner, footer, tabs_y: side.y + 1, rows }
}

/// Where the mouse wheel scrolls: the pane under the pointer, not the focused
/// one (D-20 #7) — over the panel box it is the log stream, elsewhere the chat.
/// Table panels turn the wheel into cursor movement before asking this.
pub fn wheel_target(geo: &Geometry, row: u16, col: u16) -> &'static str {
    let on_side = col >= geo.side.x
        && col < geo.side.x + geo.side.width
        && row >= geo.side.y
        && row < geo.side.y + geo.side.height;
    if on_side {
        "log"
    } else {
        "chat"
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(ratatui::widgets::Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let geo = geometry(app, area);
    render_status(frame, app, geo.status);
    render_sidebar(frame, app, &geo);
    render_chat(frame, app, geo.chat);
    render_footer(frame, app, geo.footer);
    if app.settings_open {
        render_settings_overlay(frame, app, area);
    }
    render_toasts(frame, app, geo.status);
}

fn chip(text: &str, fg: ratatui::style::Color, bg: Option<ratatui::style::Color>) -> Span<'static> {
    let mut style = Style::default().fg(fg);
    if let Some(bg) = bg {
        style = style.bg(bg);
    }
    Span::styled(format!(" {text} "), style)
}

/// #chat padding: one column of left padding.
fn pad_left_line(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(line.spans);
    Line::from(spans)
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let session = app
        .state
        .as_ref()
        .and_then(|s| s.pointer("/session/session_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let mut spans: Vec<Span> = vec![
        Span::styled(" TeamAgents", Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {session}"), Style::default().fg(GREY)),
    ];
    let paused = app
        .state
        .as_ref()
        .and_then(|s| s.pointer("/session/status"))
        .and_then(|v| v.as_str())
        .map(|status| status == "PAUSED")
        .unwrap_or(false);
    // chips in priority order; anything that does not fit is dropped (never clipped)
    let mut chips: Vec<Span> = vec![];
    if app.disconnected {
        chips.push(chip(
            tr(app.lang, "[界面刷新失败] ", &[]).trim(),
            BG,
            Some(ERROR),
        ));
    }
    if paused {
        chips.push(chip(&tr(app.lang, "已暂停", &[]), BG, Some(WARNING)));
    }
    let approvals = app
        .state
        .as_ref()
        .and_then(|s| s.get("pending_approvals"))
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if approvals > 0 {
        let count = approvals.to_string();
        chips.push(chip(
            &format!("⚠ {}", app.t("待批准 {count}", &[("count", &count)])),
            BG,
            Some(WARNING),
        ));
    }
    let open_tasks = app
        .state
        .as_ref()
        .and_then(|s| s.get("tasks"))
        .and_then(|v| v.as_array())
        .map(|tasks| {
            tasks
                .iter()
                .filter(|t| {
                    matches!(
                        t.get("status").and_then(|v| v.as_str()),
                        Some("PENDING") | Some("RUNNING") | Some("BLOCKED")
                    )
                })
                .count()
        })
        .unwrap_or(0);
    if open_tasks > 0 {
        let count = open_tasks.to_string();
        chips.push(chip(
            &format!("▸ {}", app.t("未完成任务 {count}", &[("count", &count)])),
            FG,
            Some(PANEL_BG),
        ));
    }
    let mut used: usize = spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
    for c in chips {
        let w = UnicodeWidthStr::width(c.content.as_ref());
        if used + w > area.width as usize {
            break;
        }
        used += w;
        spans.push(c);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Public form used by the mouse hit-test in main.rs.
pub fn tab_badge_for(app: &App, index: usize) -> Option<String> {
    tab_badge(app, PANELS[index])
}

/// Counts shown next to the tab labels (only when non-zero).
fn tab_badge(app: &App, panel: &str) -> Option<String> {
    let count = match panel {
        "tasks" => app
            .state
            .as_ref()
            .and_then(|s| s.get("tasks"))
            .and_then(|v| v.as_array())
            .map(|tasks| {
                tasks
                    .iter()
                    .filter(|t| {
                        matches!(
                            t.get("status").and_then(|v| v.as_str()),
                            Some("PENDING") | Some("RUNNING") | Some("BLOCKED")
                        )
                    })
                    .count()
            })
            .unwrap_or(0),
        "approvals" => app
            .state
            .as_ref()
            .and_then(|s| s.get("pending_approvals"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
        "shared" => app.shared.len(),
        _ => 0,
    };
    if count == 0 {
        None
    } else {
        Some(count.to_string())
    }
}

/// One rendered tab: its spans, display width, panel index and the absolute
/// column range it occupies. `hovered` is set for the tab under the pointer, so
/// the renderer, the hover style and the hit-test all agree by construction.
pub struct TabPiece {
    pub spans: Vec<Span<'static>>,
    pub width: usize,
    pub index: usize,
    pub x: u16,
    pub hovered: bool,
}

/// Lay out the tab strip for a pane of `inner_width` starting at `inner_x`:
/// returns the visible pieces (already windowed around the active tab) and
/// whether tabs are hidden on either side.
pub fn tab_layout(app: &App, inner_x: u16, inner_width: u16) -> (Vec<TabPiece>, bool, bool) {
    let mut raw: Vec<(Vec<Span<'static>>, usize, usize)> = vec![];
    for (i, panel) in PANELS.iter().enumerate() {
        let label = panel_tab_label(app.lang, i);
        let active = i == app.panel;
        let mut spans: Vec<Span> = vec![Span::styled(
            if active { format!("▍{label}") } else { format!(" {label}") },
            if active {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(GREY)
            },
        )];
        match tab_badge(app, panel) {
            Some(badge) => spans.push(Span::styled(
                format!(" {badge}"),
                Style::default()
                    .fg(if active { ACCENT } else { NOTICE })
                    .add_modifier(Modifier::DIM),
            )),
            None => spans.push(Span::raw(" ")),
        }
        let w: usize = spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
        raw.push((spans, w, i));
    }
    let budget = inner_width as usize;
    let total: usize = raw.iter().map(|(_, w, _)| *w).sum();
    let (lo, hi, offset) = if total + 1 <= budget {
        (0, raw.len().saturating_sub(1), 1usize)
    } else {
        let widths: Vec<usize> = raw.iter().map(|(_, w, _)| *w).collect();
        let (lo, hi) = tab_window(&widths, app.panel, budget.saturating_sub(3));
        (lo, hi, 1 + usize::from(lo > 0))
    };

    let mut pieces = vec![];
    let mut x = inner_x as usize + offset;
    for (spans, width, index) in raw.into_iter().skip(lo).take(hi + 1 - lo) {
        let hovered = matches!(app.pointer, Some((row, col))
            if !app.settings_open
                && row == app.tab_row
                && (col as usize) >= x
                && (col as usize) < x + width);
        let spans = if hovered {
            spans
                .into_iter()
                .map(|s| {
                    let style = s.style.bg(HOVER_BG).fg(FG);
                    Span::styled(s.content.into_owned(), style)
                })
                .collect()
        } else {
            spans
        };
        pieces.push(TabPiece { spans, width, index, x: x as u16, hovered });
        x += width;
    }
    (pieces, lo > 0, hi + 1 < PANELS.len())
}

/// Which panel a click at `col` hits, using the same layout the renderer used.
pub fn tab_at(app: &App, inner_x: u16, inner_width: u16, col: u16) -> Option<usize> {
    let (pieces, _, _) = tab_layout(app, inner_x, inner_width);
    pieces
        .into_iter()
        .find(|p| col >= p.x && (col as usize) < p.x as usize + p.width)
        .map(|p| p.index)
}

/// Visible tab range for a strip that cannot fit: the active tab is always
/// included, neighbours fill the budget outward, and the cut edges get markers
/// (the caller prints ‹ / › from the returned bounds).
pub fn tab_window(widths: &[usize], active: usize, budget: usize) -> (usize, usize) {
    if widths.is_empty() {
        return (0, 0);
    }
    let active = active.min(widths.len() - 1);
    let (mut lo, mut hi) = (active, active);
    let mut used = widths[active];
    loop {
        let left = lo.checked_sub(1).map(|i| widths[i] + 1);
        let right = if hi + 1 < widths.len() { Some(widths[hi + 1] + 1) } else { None };
        match (left, right) {
            (Some(l), _) if used + l <= budget => {
                lo -= 1;
                used += l;
            }
            (_, Some(r)) if used + r <= budget => {
                hi += 1;
                used += r;
            }
            _ => break,
        }
    }
    (lo, hi)
}

fn render_sidebar(frame: &mut Frame, app: &mut App, geo: &Geometry) {
    let area = geo.side;
    if area.width < 10 || area.height < 5 {
        return;
    }
    let focused = app.focus == Focus::Panel;
    let border = if focused { ACCENT } else { GREY };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(border));
    let inner = geo.side_inner; // == block.inner(area); the geometry owns it
    frame.render_widget(block, area);
    if inner.height < 4 {
        return;
    }

    // tab strip: one row, windowed around the active tab (a tab bar never
    // wraps); ‹ › mark tabs that are scrolled out of view. The layout (and the
    // hover state) comes from `tab_layout`, so a click can never miss its tab.
    let (pieces, hidden_left, hidden_right) = tab_layout(app, inner.x, inner.width);
    let mut shown: Vec<Span> = vec![Span::raw(" ")];
    if hidden_left {
        shown.push(Span::styled("‹", Style::default().fg(ACCENT)));
    }
    for piece in &pieces {
        shown.extend(piece.spans.clone());
    }
    if hidden_right {
        shown.push(Span::styled("›", Style::default().fg(ACCENT)));
    }
    frame.render_widget(Paragraph::new(Line::from(shown)), Rect { height: 1, ..inner });
    app.tab_row = inner.y;
    let divider_y = inner.y + 1;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(PANEL_BG),
        ))),
        Rect { y: divider_y, height: 1, ..inner },
    );

    // the hint is a wrapping, dim line pinned to the bottom of the box
    let hint = panel_hint(PANELS[app.panel]);
    let hint_rows = wrap_lines(
        vec![Line::from(Span::styled(tr(app.lang, hint, &[]), Style::default().fg(GREY)))],
        (inner.width as usize).saturating_sub(1).max(1),
    );
    let hint_h = (hint_rows.len() as u16).min(inner.height.saturating_sub(2));
    let content = Rect {
        y: divider_y + 1,
        height: inner.height.saturating_sub(2 + hint_h),
        ..inner
    };
    render_panel(frame, app, content);
    let hints: Vec<Line> = hint_rows.into_iter().map(pad_left_line).collect();
    frame.render_widget(
        Paragraph::new(hints),
        Rect { y: inner.y + inner.height - hint_h, height: hint_h, ..inner },
    );
}

fn render_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let panel = PANELS[app.panel];
    let focused = app.focus == Focus::Panel;
    match panel {
        "team" | "tasks" | "approvals" | "sessions" | "shared" => {
            let (header, keyed, empty) = match panel {
                "team" => (table_headers("team"), app.team_rows(), "没有成员"),
                "tasks" => (table_headers("tasks"), app.tasks_rows(), "没有任务"),
                "approvals" => (table_headers("approvals"), app.approvals_rows(), "没有待批准操作"),
                "sessions" => (table_headers("sessions"), app.sessions_rows(), "没有会话记录"),
                _ => (table_headers("shared"), app.shared_rows(), "没有共享条目"),
            };
            let header: Vec<String> = header.iter().map(|h| tr(app.lang, h, &[])).collect();
            let header_refs: Vec<&str> = header.iter().map(|s| s.as_str()).collect();
            let saved = app.table_cursors.get(panel).cloned().unwrap_or((None, 0));
            let sel = saved
                .0
                .and_then(|k| keyed.iter().position(|(rk, _)| rk == &k))
                .unwrap_or_else(|| saved.1.min(keyed.len().saturating_sub(1)));
            let sel = if keyed.is_empty() { None } else { Some(sel) };
            render_table(
                app,
                frame,
                area,
                &header_refs,
                &keyed.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(),
                sel,
                focused,
                &tr(app.lang, empty, &[]),
                drop_order(panel),
            );
        }
        "log" => {
            let body = Rect { height: area.height.saturating_sub(1), ..area };
            let wrap_w = (body.width as usize).min(80);
            let lines = wrap_lines(
                app.log_lines
                    .iter()
                    .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(NOTICE))))
                    .collect(),
                wrap_w,
            );
            // the renderer owns the wrapped height: Ctrl+Home asks for "as far as
            // it goes" and the clamp here turns that into the real offset
            app.log_scroll = app.log_scroll.min(lines.len().saturating_sub(body.height as usize));
            render_scrolled_lang(frame, body, lines, app.log_scroll, true, app.lang);
            let title = Rect { y: area.y + area.height - 1, height: 1, ..area };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    tr(app.lang, "事件流{v0}", &[("v0", &app.log_filter_suffix())]),
                    Style::default().fg(GREY),
                ))),
                title,
            );
        }
        _ => {}
    }
}

/// `/settings` overlay: a centred box over the chat, Esc closes it.
fn render_settings_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let info = app.settings_lines();
    // stay inside the frame: ratatui's Clear writes every cell of its rect, and
    // a rect that pokes out of the buffer panics on narrow terminals
    let width = ((area.width as usize * 7 / 10).clamp(48, 92))
        .min(area.width.saturating_sub(2).max(1) as usize) as u16;
    let height = ((info.len() + 5) as u16).min(area.height.saturating_sub(2)).max(1);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let box_area = Rect { x, y, width, height };
    // ratatui's buffer diff never emits a cell that follows a wide grapheme, so a
    // CJK chat line ending right under the frame would silently erase the border.
    // Blank the column just left of the box and keep the inner text one column
    // short of the right border; the borders then always reach the terminal.
    frame.render_widget(
        ratatui::widgets::Clear,
        Rect {
            x: box_area.x.saturating_sub(1),
            y: box_area.y,
            width: 1,
            height: box_area.height,
        },
    );
    frame.render_widget(ratatui::widgets::Clear, box_area);
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(PANEL_BG))
        .title(Line::from(Span::styled(
            format!(" {} ", tr(app.lang, "设置", &[])),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        )))
        .title_alignment(ratatui::layout::Alignment::Left);
    let inner = block.inner(box_area);
    let text = Rect {
        width: inner.width.saturating_sub(1),
        ..inner
    };
    frame.render_widget(block, box_area);
    if inner.height < 4 || inner.width < 4 {
        return;
    }
    let label = |text: String| Span::styled(format!(" {text}"), Style::default().fg(GREY));
    let value = |text: String, selected: bool| {
        Span::styled(
            format!(" {text} "),
            if selected {
                Style::default().fg(BG).bg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FG).bg(PANEL_BG)
            },
        )
    };
    // only the interface language is configurable here (the spinner is always on)
    let name = tr(app.lang, "界面语言", &[]);
    let value_text = if app.lang == "zh-CN" { "中文".to_string() } else { "English".to_string() };
    // the value cell starts at a fixed display column so the dropdown can line
    // up with it in either language (the label pads by display width, not chars)
    let value_col = (1 + UnicodeWidthStr::width(name.as_str())).max(17);
    let pad = value_col.saturating_sub(1 + UnicodeWidthStr::width(name.as_str()));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            label(format!("{name}{}", " ".repeat(pad))),
            value(value_text, true),
        ])),
        Rect { y: text.y, height: 1, ..text },
    );
    // the dropdown unfolds onto the blank row under the language row and pushes
    // the info body down while it is open (it used to overlap the first info line)
    let dropdown_h = if app.lang_open
        && inner.height >= 6
        && (text.width as usize) >= value_col + 9
    {
        2u16
    } else {
        0
    };
    if dropdown_h > 0 {
        let dd = Rect {
            x: text.x + value_col as u16,
            y: inner.y + 1, // the blank row under the language line
            width: ((text.width as usize) - value_col).min(14) as u16,
            height: 2,
        };
        frame.render_widget(ratatui::widgets::Clear, dd);
        for (i, option) in ["English", "中文"].iter().enumerate() {
            let style = if i == app.lang_choice {
                Style::default().fg(BG).bg(ACCENT)
            } else {
                Style::default().fg(FG).bg(PANEL_BG)
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(format!(" {option} "), style))),
                Rect { y: inner.y + 1 + i as u16, width: dd.width, ..dd },
            );
        }
    }
    let body_y = inner.y + 2 + dropdown_h; // one blank line under the language row
    let body: Vec<Line> = info
        .iter()
        .map(|l| Line::from(Span::styled(format!(" {l}"), Style::default().fg(NOTICE))))
        .collect();
    frame.render_widget(
        Paragraph::new(body),
        Rect {
            y: body_y,
            height: text.y + text.height.saturating_sub(1).saturating_sub(body_y),
            ..text
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                " {}",
                tr(app.lang, "Enter 选择语言 · Esc 关闭", &[])
            ),
            Style::default().fg(GREY),
        ))),
        Rect { y: text.y + text.height - 1, height: 1, ..text },
    );
}

/// A table in the Rust spirit: dim header with a rule under it, subtle zebra,
/// an accent bar on the selected row, centred empty state.
/// The active panel's table: translated headers, rows, and its empty-state id.
pub fn panel_table(app: &App) -> (Vec<String>, Vec<Vec<Cell>>, &'static str) {
    match PANELS[app.panel] {
        "tasks" => (
            table_headers("tasks").iter().map(|h| tr(app.lang, h, &[])).collect(),
            app.tasks_rows().into_iter().map(|(_, r)| r).collect(),
            "没有任务",
        ),
        "approvals" => (
            table_headers("approvals").iter().map(|h| tr(app.lang, h, &[])).collect(),
            app.approvals_rows().into_iter().map(|(_, r)| r).collect(),
            "没有待批准操作",
        ),
        "sessions" => (
            table_headers("sessions").iter().map(|h| tr(app.lang, h, &[])).collect(),
            app.sessions_rows().into_iter().map(|(_, r)| r).collect(),
            "没有会话记录",
        ),
        "shared" => (
            table_headers("shared").iter().map(|h| tr(app.lang, h, &[])).collect(),
            app.shared_rows().into_iter().map(|(_, r)| r).collect(),
            "没有共享条目",
        ),
        _ => (
            table_headers("team").iter().map(|h| tr(app.lang, h, &[])).collect(),
            app.team_rows().into_iter().map(|(_, r)| r).collect(),
            "没有成员",
        ),
    }
}

/// Which columns a table gives up first when the pane is narrow.
fn drop_order(panel: &str) -> &'static [usize] {
    match panel {
        "team" => &[2, 5, 6, 3, 1],          // Type, Workspace, Access, Model, Role
        "tasks" => &[7, 6, 5, 1, 2],         // Created, Result, Dependencies, Requester, Assignee
        "approvals" => &[3, 1],              // Scope, Operation
        "sessions" => &[5, 4, 3, 2, 6],      // Updated, Size, Events, Goal, Flags
        _ => &[4, 2, 0, 1],                  // Shared: Sequence, Kind, Space, Author
    }
}

/// First visible row index: keeps the selection on screen, preferring to
/// centre it (the click hit-test reuses this).
pub fn table_start(rows: usize, sel: usize, view: usize) -> usize {
    if view == 0 || rows <= view {
        return 0;
    }
    let max_start = rows - view;
    sel.saturating_sub(view / 2).min(max_start)
}

fn render_table(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    header: &[&str],
    rows: &[Vec<Cell>],
    sel: Option<usize>,
    focused: bool,
    empty: &str,
    drop: &[usize],
) {
    if area.height == 0 || area.width < 6 {
        return;
    }
    if rows.is_empty() {
        let y = area.y + (area.height / 3).max(1);
        let text = Line::from(Span::styled(
            format!("— {empty} —"),
            Style::default().fg(GREY).add_modifier(Modifier::DIM),
        ))
        .centered();
        frame.render_widget(Paragraph::new(text), Rect { y, height: 1, ..area });
        return;
    }
    let mut widths = col_widths(header, rows, area.width as usize);
    let mut keep: Vec<usize> = (0..header.len()).collect();
    let fits = |keep: &[usize], widths: &[usize]| -> bool {
        let text: usize = keep.iter().map(|i| widths[*i]).sum();
        text + 2 * keep.len().saturating_sub(1) + 1 <= area.width as usize
    };
    for index in drop {
        if keep.len() <= 2 || fits(&keep, &widths) {
            break;
        }
        keep.retain(|c| c != index);
    }
    if !fits(&keep, &widths) {
        // still too wide: shrink the widest kept column until it fits
        while !fits(&keep, &widths) {
            let Some((i, _)) = keep
                .iter()
                .enumerate()
                .max_by_key(|(_, index)| widths[**index])
            else {
                break;
            };
            let index = keep[i];
            if widths[index] <= 4 {
                break;
            }
            widths[index] -= 1;
        }
    }
    let header_style = Style::default().fg(GREY).add_modifier(Modifier::BOLD);
    let visible: Vec<usize> = keep.clone();
    let header_line: Vec<Span> = std::iter::once(Span::styled(" ", header_style))
        .chain(
            visible
                .iter()
                .map(|i| Span::styled(ellipsize(header[*i], widths[*i]), header_style))
                .collect::<Vec<_>>()
                .join(Span::styled("  ", header_style)),
        )
        .collect();
    frame.render_widget(Paragraph::new(Line::from(header_line)), Rect { height: 1, ..area });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(PANEL_BG),
        ))),
        Rect { y: area.y + 1, height: 1, ..area },
    );

    let body = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
    let start = table_start(rows.len(), sel.unwrap_or(0), body.height as usize);
    let mut lines: Vec<Line> = vec![];
    for (i, row) in rows.iter().enumerate().skip(start).take((body.height as usize).max(1)) {
        let selected = sel == Some(i);
        let first_row = body.y;
        let hovered = matches!(app.pointer, Some((row, _))
            if row >= first_row && (row - first_row) as usize == i - start && !app.settings_open);
        let bg = if selected {
            SELECT_BG
        } else if hovered {
            HOVER_BG
        } else if i % 2 == 1 {
            ZEBRA_BG
        } else {
            BG
        };
        let marker = if selected {
            Span::styled("▌", Style::default().fg(ACCENT).bg(bg))
        } else {
            Span::styled(" ", Style::default().bg(bg))
        };
        let cells: Vec<Span> = visible
            .iter()
            .map(|j| {
                let (text, style) = row.get(*j).cloned().unwrap_or((String::new(), None));
                let fg = match style {
                    Some(name) => style_named(name),
                    None => Style::default().fg(if selected || hovered { FG } else { NOTICE }),
                };
                let modifier = if selected { Modifier::BOLD } else { Modifier::empty() };
                Span::styled(
                    ellipsize(&text, widths.get(*j).copied().unwrap_or(6)),
                    fg.bg(bg).add_modifier(modifier),
                )
            })
            .collect::<Vec<_>>()
            .join(Span::styled("  ", Style::default().bg(bg)));
        lines.push(Line::from(
            std::iter::once(marker).chain(cells).collect::<Vec<_>>(),
        ));
    }
    let _ = focused;
    let view = body.height as usize;
    let text_area = if rows.len() > view && body.width > 3 {
        Rect { width: body.width.saturating_sub(1), ..body }
    } else {
        body
    };
    frame.render_widget(Paragraph::new(lines), text_area);
    if rows.len() > view && body.width > 3 {
        let max_scroll = rows.len() - view;
        let mut state = ratatui::widgets::ScrollbarState::new(max_scroll + 1).position(start);
        frame.render_stateful_widget(
            ratatui::widgets::Scrollbar::new(ratatui::widgets::ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(GREY))
                .thumb_symbol("┃")
                .track_symbol(Some(" "))
                .thumb_style(Style::default().fg(GREY))
                .begin_symbol(None)
                .end_symbol(None),
            Rect { x: body.x + body.width - 1, width: 1, ..body },
            &mut state,
        );
    }
}

fn render_chat(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 6 || area.width < 12 {
        return;
    }
    let live_width = (area.width as usize).saturating_sub(3).max(1);
    let live_lines = if app.stream_text.is_empty() {
        vec![]
    } else {
        md::render(&app.stream_text, live_width)
    };
    let live_h = (live_lines.len() as u16 + 1).min(9);
    let composer_text_w = (area.width as usize).saturating_sub(5).max(1);
    let composer_h = app.composer.widget_height(composer_text_w) as u16;
    // the key hint lives on the composer's bottom border, so the column ends
    // exactly where the sidebar box does (their bottom borders line up)
    let chunks = Layout::vertical([
        Constraint::Length(2),          // activity chips + latest
        Constraint::Fill(1),            // chat log
        Constraint::Length(live_h),     // streaming preview
        Constraint::Length(composer_h), // composer box
    ])
    .split(area);

    // activity: status chips, then the latest-activity text dimmed on the right
    let (chips, latest) = app.activity_chips();
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (i, (text, style)) in chips.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(PANEL_BG)));
        }
        spans.push(Span::styled(text.clone(), style_named(style)));
    }
    let used: usize = spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
    let latest_width = (area.width as usize).saturating_sub(used + 4);
    if !latest.is_empty() && latest_width > 12 {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            head_chars(&latest, latest_width),
            Style::default().fg(GREY).add_modifier(Modifier::DIM),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);

    // chat log: wrapped entries, scrolled by app.chat_scroll, with a scrollbar
    let mut lines: Vec<Line<'static>> = vec![];
    for (who, text) in &app.chat {
        lines.extend(chat_entry_lines(app.lang, who, text, chunks[1].width as usize));
    }
    let wrapped = wrap_lines(lines, (chunks[1].width as usize).saturating_sub(3).max(1));
    let wrapped: Vec<Line> = wrapped.into_iter().map(pad_left_line).collect();
    // Ctrl+Home sets the offset to "the top"; the wrapped height is only known
    // here (it depends on the live width), so clamp the request to it.
    app.chat_scroll = app.chat_scroll.min(wrapped.len().saturating_sub(chunks[1].height as usize));
    render_scrolled_lang(frame, chunks[1], wrapped, app.chat_scroll, true, app.lang);

    // streaming preview carries an accent bar so it reads as "in progress"
    if live_h > 0 {
        let live_area = Rect { x: chunks[2].x + 1, height: live_h, ..chunks[2] };
        let wrapped = wrap_lines(live_lines, live_area.width.saturating_sub(3) as usize);
        let lines: Vec<Line> = wrapped
            .into_iter()
            .map(|line| {
                let mut spans = vec![Span::styled("▌ ", Style::default().fg(ACCENT))];
                spans.extend(line.spans);
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), live_area);
    }

    render_composer(frame, app, chunks[3]);
    render_slash_menu(frame, app, area, chunks[3]);
}

/// `/` completion menu: matches listed above the composer, the highlighted one
/// on a lighter surface (same hover language as the tabs).
fn render_slash_menu(frame: &mut Frame, app: &App, area: Rect, composer: Rect) {
    if !app.slash_open() || area.width < 16 {
        // too narrow for a bordered menu; the composer still runs the command
        return;
    }
    let matches = app.slash_matches();
    let selected = app.slash_index.min(matches.len().saturating_sub(1));
    let name_w = matches
        .iter()
        .map(|c| UnicodeWidthStr::width(c.name))
        .max()
        .unwrap_or(0);
    let desc_w = matches
        .iter()
        .map(|c| UnicodeWidthStr::width(tr(app.lang, c.description, &[]).as_str()))
        .max()
        .unwrap_or(0);
    // min must stay ≤ max (a 12–27 col terminal used to panic here)
    let width = ((name_w + desc_w + 6) as u16).clamp(8, area.width.saturating_sub(4).max(8));
    let height = (matches.len() as u16 + 2).min(8);
    let y = composer.y.saturating_sub(height).max(area.y + 1);
    let x = area.x + 2;
    let box_area = Rect { x, y, width, height }.intersection(area);
    // ratatui's buffer diff never emits a cell that follows a wide grapheme, so a
    // CJK chat line ending right under the frame would silently erase the border.
    // Blank the column just left of the box and keep the inner text one column
    // short of the right border; the borders then always reach the terminal.
    frame.render_widget(
        ratatui::widgets::Clear,
        Rect {
            x: box_area.x.saturating_sub(1),
            y: box_area.y,
            width: 1,
            height: box_area.height,
        },
    );
    frame.render_widget(ratatui::widgets::Clear, box_area);
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(GREY))
        .style(Style::default().bg(BG))
        .title(Line::from(Span::styled(
            format!(" {} ", tr(app.lang, "命令", &[])),
            Style::default().fg(GREY).add_modifier(Modifier::DIM),
        )));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    let rows: Vec<Line> = matches
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(i, command)| {
            let style = if i == selected {
                Style::default().fg(FG).bg(HOVER_BG)
            } else {
                Style::default().fg(GREY).bg(BG)
            };
            Line::from(Span::styled(
                format!(" {:<name_w$}  {}", command.name, tr(app.lang, command.description, &[])),
                style,
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), inner);
}

fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    if area.height < 3 {
        return;
    }
    let focused = app.focus == Focus::Composer;
    let border = if focused { ACCENT } else { GREY };
    let title = format!(" {} ", app.composer_title());
    let title: String = if UnicodeWidthStr::width(title.as_str()) + 4 > area.width as usize {
        String::new()
    } else {
        title
    };
    // the key hint rides the bottom border and drops parts that do not fit
    let hint_parts = [
        tr(app.lang, "Enter 发送", &[]),
        tr(app.lang, "Shift+Enter 换行", &[]),
        tr(app.lang, "↑↓ 历史", &[]),
        tr(app.lang, "PgUp/PgDn 滚动", &[]),
        tr(app.lang, "Esc 停止 Leader", &[]),
    ];
    let mut hint = String::new();
    for (i, part) in hint_parts.iter().enumerate() {
        let candidate = if hint.is_empty() { part.clone() } else { format!("{hint} · {part}") };
        if UnicodeWidthStr::width(candidate.as_str()) + 6 > area.width as usize {
            break;
        }
        hint = candidate;
        let _ = i;
    }
    let hint: String = if hint.is_empty() { String::new() } else { format!(" {hint} ") };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(Line::from(Span::styled(
            title,
            Style::default().fg(GREY).add_modifier(Modifier::DIM),
        )))
        .title_alignment(ratatui::layout::Alignment::Right)
        .title_bottom(Line::from(Span::styled(
            hint,
            Style::default().fg(GREY).add_modifier(Modifier::DIM),
        )))
        .title_alignment(ratatui::layout::Alignment::Right)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 6 {
        return;
    }
    let text_x = inner.x + 2;
    let text_w = (inner.width as usize).saturating_sub(3).max(1);
    let visible_h = inner.height as usize;

    let empty = app.composer.lines.iter().all(|l| l.is_empty());
    let mut visual: Vec<String> = vec![];
    let mut cursor_v = (0usize, 0usize);
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
                // the caret is placed by display column: CJK glyphs take two cells
                let col_w: usize = chars[start..app.composer.col]
                    .iter()
                    .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
                    .sum();
                cursor_v = (visual.len(), col_w);
            }
            visual.push(chars[start..end].iter().collect());
            start = end;
            if start >= chars.len() {
                break;
            }
        }
    }
    let scroll = if cursor_v.0 + 1 > visible_h { cursor_v.0 + 1 - visible_h } else { 0 };
    let mut rendered: Vec<Line> = vec![];
    for (i, text) in visual.iter().skip(scroll).take(visible_h).enumerate() {
        let first = i == 0 && scroll == 0;
        rendered.push(Line::from(vec![
            Span::styled(
                if first { "› " } else { "  " }.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            if text.is_empty() && empty && first {
                Span::styled(
                    tr(app.lang, "输入你的目标，或向 Leader 补充要求（/settings 打开设置）", &[]),
                    Style::default().fg(GREY).add_modifier(Modifier::DIM),
                )
            } else {
                Span::styled(text.clone(), Style::default().fg(FG))
            },
        ]));
    }
    frame.render_widget(
        Paragraph::new(rendered),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: inner.height },
    );
    if focused {
        let cx = text_x as usize + cursor_v.1.min(text_w);
        let cy = inner.y as usize + cursor_v.0.saturating_sub(scroll).min(visible_h.saturating_sub(1));
        frame.set_cursor_position((cx as u16, cy as u16));
    }
}

/// A scrolled log: pinned to the bottom unless `scroll` lines are requested,
/// with a slim scrollbar whenever the content does not fit.
fn render_scrolled_lang(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: usize,
    bar: bool,
    lang: &str,
) {
    if area.height == 0 {
        return;
    }
    let mut lines = lines;
    let view = area.height as usize;
    if lines.len() < view {
        // a chat grows from the bottom: pad the top instead of the bottom
        let pad = view - lines.len();
        let mut padded: Vec<Line<'static>> = (0..pad).map(|_| Line::raw("")).collect();
        padded.append(&mut lines);
        lines = padded;
    }
    let total = lines.len();
    let max_scroll = total.saturating_sub(view);
    let scroll = scroll.min(max_scroll);
    let offset = max_scroll.saturating_sub(scroll);
    let text_area = if bar && total > view {
        Rect { width: area.width.saturating_sub(1), ..area }
    } else {
        area
    };
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), text_area);
    if bar && total > view && area.width > 2 {
        let mut state =
            ratatui::widgets::ScrollbarState::new(max_scroll.max(1)).position(offset);
        frame.render_stateful_widget(
            ratatui::widgets::Scrollbar::new(ratatui::widgets::ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(GREY))
                .thumb_symbol("┃")
                .track_symbol(Some(" "))
                .thumb_style(Style::default().fg(if scroll > 0 { ACCENT } else { GREY }))
                .begin_symbol(None)
                .end_symbol(None),
            Rect { x: area.x + area.width - 1, width: 1, ..area },
            &mut state,
        );
    }
    if scroll > 0 && area.height > 0 {
        let marker = Line::from(Span::styled(
            format!("{} ", tr(lang, "已上翻{count}行 · Ctrl+End 回到底部", &[("count", &scroll.to_string())])),
            Style::default().fg(BG).bg(WARNING),
        ));
        frame.render_widget(
            Paragraph::new(marker).alignment(ratatui::layout::Alignment::Right),
            Rect { y: area.y + area.height - 1, height: 1, ..area },
        );
    }
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let keys: [(&str, &str); 8] = [
        ("^q", "退出"),
        ("^p", "暂停/继续"),
        ("^f", "全自动"),
        ("^t", "切换面板"),
        ("^g", "批准"),
        ("^n", "输入"),
        ("esc", "停止 Leader"),
        ("PgUp", "滚动"),
    ];
    // bottom-right: the session's permission mode, plain dim text (no chip)
    let mode = app
        .state
        .as_ref()
        .and_then(|s| s.pointer("/session/permissions_mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("approved_scope");
    let (mode_label, mode_style) = if mode == "full_auto" {
        (tr(app.lang, "全自动", &[]), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
    } else {
        (tr(app.lang, "预授权", &[]), Style::default().fg(GREY).add_modifier(Modifier::DIM))
    };
    let mode_w = UnicodeWidthStr::width(mode_label.as_str());
    let budget = (area.width as usize).saturating_sub(mode_w + 3);

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let mut used = 1usize;
    let mut shown = 0usize;
    for (i, (key, label)) in keys.iter().enumerate() {
        let label = tr(app.lang, label, &[]);
        let piece_w = UnicodeWidthStr::width(*key) + 1 + UnicodeWidthStr::width(label.as_str());
        let extra = if i == 0 { piece_w } else { piece_w + 2 };
        if used + extra > budget {
            break;
        }
        if i > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled(format!(" {label}"), Style::default().fg(GREY)));
        used += extra;
        shown += 1;
    }
    if shown < keys.len() {
        spans.push(Span::styled(" …", Style::default().fg(GREY)));
        used += 2;
    }
    let pad = (area.width as usize).saturating_sub(used + mode_w + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(mode_label, mode_style));
    spans.push(Span::raw(" "));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_toasts(frame: &mut Frame, app: &mut App, status_area: Rect) {
    let now = std::time::Instant::now();
    let visible: Vec<&crate::app::Toast> = app.toasts.iter().filter(|t| t.until > now).collect();
    if visible.is_empty() {
        return;
    }
    let area_frame = frame.area();
    let width = area_frame.width;
    let mut y = status_area.y + 1;
    for toast in visible.iter().take(3) {
        let (icon, color) = match toast.severity {
            Severity::Warning => ("!", WARNING),
            Severity::Error => ("✖", ERROR),
            Severity::Info => ("✔", ACCENT),
        };
        // the box must fit the terminal: 58 is the comfortable cap, but a narrow
        // frame wins (Clear outside the buffer panics)
        let avail = (width as usize).saturating_sub(2).max(8);
        let text_w = UnicodeWidthStr::width(toast.text.as_str())
            .min(58)
            .min(avail.saturating_sub(6).max(4));
        let w = (text_w + 6) as u16;
        let x = width.saturating_sub(w + 1);
        let body = wrap_line(
            &Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(toast.text.clone(), Style::default().fg(FG)),
            ]),
            text_w,
        );
        let h = (body.len() + 2) as u16;
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(color))
            .style(Style::default().bg(PANEL_BG));
        let area = Rect { x, y, width: w, height: h }.intersection(area_frame);
        if area.width < 4 || area.height < 2 {
            break; // no room left below the status row
        }
        frame.render_widget(ratatui::widgets::Clear, area);
        frame.render_widget(Paragraph::new(body).block(block), area);
        y += h;
    }
}
