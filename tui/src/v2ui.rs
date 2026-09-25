//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::theme::*;
use crate::v2app::{ChatKind, Focus, V2App, View};
use crate::wrap::wrap_lines;

/// Panel chrome shared by every bordered box: accent border, panel surface and
/// an accent title, the v1 palette's look.
fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title.into(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(PANEL_BG).fg(FG))
}

/// A selected list row: dark text on the accent surface (v1's selection look).
fn selection() -> Style {
    Style::default().fg(PANEL_BG).bg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Screen regions shared by rendering and hit-testing.
pub struct V2Geometry {
    pub status: Rect,
    /// Middle region: the conversation in the chat view, the active panel
    /// (instances/tasks/topology) otherwise.
    pub body: Rect,
    /// Pending-approvals box (chat view only; height 0 when nothing pends).
    pub approvals: Rect,
    /// Composer box (chat view only).
    pub composer: Rect,
    pub footer: Rect,
}

pub fn geometry(app: &V2App, area: Rect) -> V2Geometry {
    let composer_text_w = (area.width as usize).saturating_sub(4).max(1);
    let chat_view = app.view == View::Chat;
    let composer_h = if chat_view { app.composer.widget_height(composer_text_w) as u16 } else { 0 };
    let approvals_h = if chat_view && !app.approvals.is_empty() { (app.approvals.len() as u16).min(3) + 2 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),           // status
            Constraint::Min(1),              // body (chat or panel)
            Constraint::Length(approvals_h), // approvals box
            Constraint::Length(composer_h),  // composer box
            Constraint::Length(1),           // footer
        ])
        .split(area);
    V2Geometry { status: chunks[0], body: chunks[1], approvals: chunks[2], composer: chunks[3], footer: chunks[4] }
}

/// Row inside the approvals box a click maps to (border + padding), or None.
pub fn approval_at(geo: &V2Geometry, app: &V2App, row: u16, col: u16) -> Option<usize> {
    let area = geo.approvals;
    if area.height == 0
        || col < area.x
        || col >= area.x + area.width
        || row <= area.y
        || row >= area.y + area.height - 1
    {
        return None;
    }
    let index = (row - area.y - 1) as usize;
    (index < app.approvals.len()).then_some(index)
}

/// List row inside a panel body a click maps to (border + one line per
/// row), or None. The view decides what the index means.
pub fn panel_row_at(geo: &V2Geometry, row: u16, col: u16) -> Option<usize> {
    let area = geo.body;
    if area.height < 2 || col < area.x || col >= area.x + area.width || row <= area.y || row >= area.y + area.height - 1
    {
        return None;
    }
    Some((row - area.y - 1) as usize)
}

pub fn render(frame: &mut Frame, app: &mut V2App) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let geo = geometry(app, area);
    // the palette's screen background, drawn first so every widget sits on it
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    render_status(frame, app, geo.status);
    match app.view {
        View::Chat => {
            render_chat(frame, app, geo.body);
            render_approvals(frame, app, geo.approvals);
            render_composer(frame, app, geo.composer);
        }
        View::Instances => render_instances(frame, app, geo.body),
        View::Tasks => render_tasks(frame, app, geo.body),
        View::Topology => render_topology(frame, app, geo.body),
    }
    render_footer(frame, app, geo.footer);
}

fn render_instances(frame: &mut Frame, app: &V2App, area: Rect) {
    let block = panel("instances (● conversation target)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line<'static>> = app
        .instances
        .iter()
        .enumerate()
        .map(|(i, instance)| {
            let selected = i == app.instance_sel;
            let marker = if selected { "▶" } else { " " };
            let target = if i == app.active { "●" } else { " " };
            let style = if selected { selection() } else { Style::default().fg(FG) };
            let lifecycle_style = match instance.lifecycle.as_str() {
                "ACTIVE" => Style::default().fg(SUCCESS),
                "PAUSED" | "PARKED" => Style::default().fg(WARNING),
                _ => Style::default().fg(ERROR),
            };
            Line::from(vec![
                Span::styled(format!("{marker}{target} "), style),
                Span::styled(instance.id.clone(), style),
                Span::styled(" · ", Style::default().fg(GREY)),
                Span::styled(instance.lifecycle.clone(), lifecycle_style),
                Span::styled(format!(" · {}", instance.phase), Style::default().fg(NOTICE)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tasks(frame: &mut Frame, app: &V2App, area: Rect) {
    let block = panel("tasks");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line<'static>> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let selected = i == app.task_sel;
            let marker = if selected { "▶" } else { " " };
            let style = if selected { selection() } else { Style::default().fg(FG) };
            let status_style = match task.status.as_str() {
                "RUNNING" => Style::default().fg(ACCENT),
                "PENDING" => Style::default().fg(WARNING),
                "SUCCEEDED" => Style::default().fg(SUCCESS),
                "FAILED" | "BLOCKED" => Style::default().fg(ERROR),
                _ => Style::default().fg(NOTICE),
            };
            Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(task.id.clone(), style),
                Span::styled(" · ", Style::default().fg(GREY)),
                Span::styled(task.status.clone(), status_style),
                Span::styled(
                    format!(" · assignee {} · goal {}", task.assignee, task.goal_id),
                    Style::default().fg(NOTICE),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Topology as an edge list (§9: the panel is an edge list; no canvas is required
/// for execution correctness): active grant/channel edges, then task-delegation edges.
fn render_topology(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let active_grants = app.grants.iter().filter(|g| !g.revoked).count();
    let block = panel(format!("topology · active grants {active_grants} · tasks {}", app.tasks.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let dim = Style::default().fg(GREY);
    let bold = Style::default().fg(FG).add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled("grants and channels", bold))];
    if active_grants == 0 {
        lines.push(Line::from(Span::styled("  (no active grants)", dim)));
    }
    for grant in app.grants.iter().filter(|g| !g.revoked) {
        let kind = if grant.action == "message" { "channel" } else { "grant" };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(grant.subject.clone(), Style::default().fg(FG)),
            Span::styled(format!(" ─{}→ ", grant.action), Style::default().fg(ACCENT)),
            Span::styled(grant.scope.clone(), Style::default().fg(NOTICE)),
            Span::styled(format!("  {kind}"), dim),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("task delegation", bold)));
    if app.tasks.is_empty() {
        lines.push(Line::from(Span::styled("  (no tasks)", dim)));
    }
    for task in &app.tasks {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(task.id.clone(), Style::default().fg(FG)),
            Span::styled(" ─→ ", Style::default().fg(ACCENT)),
            Span::styled(task.assignee.clone(), Style::default().fg(NOTICE)),
            Span::styled(format!("  [{}]", task.status), dim),
        ]));
    }
    let height = inner.height as usize;
    app.last_chat_height = height.max(1); // topology page scrolling reuses it
    let max_scroll = lines.len().saturating_sub(height);
    if app.topo_scroll > max_scroll {
        app.topo_scroll = max_scroll;
    }
    let visible: Vec<Line<'static>> = lines.into_iter().skip(app.topo_scroll).take(height).collect();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn render_status(frame: &mut Frame, app: &V2App, area: Rect) {
    let style = if app.disconnected {
        Style::default().fg(BG).bg(ERROR).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG).bg(PANEL_BG)
    };
    frame.render_widget(Paragraph::new(format!(" {} ", app.status_line())).style(style), area);
}

fn render_chat(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let width = area.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for entry in &app.entries {
        // v1 palette convention: grey bold labels, white body text, accent for
        // the assistant, notice grey for everything machine-generated
        let (prefix, style) = match entry.kind {
            ChatKind::User => (entry.who.to_string(), Style::default().fg(GREY).add_modifier(Modifier::BOLD)),
            ChatKind::Assistant => (entry.who.clone(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            ChatKind::Tool => (entry.who.clone(), Style::default().fg(GREY).add_modifier(Modifier::BOLD)),
            ChatKind::Summary => (entry.who.clone(), Style::default().fg(ACCENT)),
            ChatKind::System => (entry.who.clone(), Style::default().fg(GREY)),
            ChatKind::Error => (entry.who.clone(), Style::default().fg(ERROR).add_modifier(Modifier::BOLD)),
        };
        let head = Line::from(vec![Span::styled(prefix, style), Span::styled(" ", Style::default().fg(GREY))]);
        let body_style = match entry.kind {
            ChatKind::System => Style::default().fg(NOTICE),
            ChatKind::Error => Style::default().fg(ERROR),
            ChatKind::Tool => Style::default().fg(NOTICE),
            ChatKind::Summary => Style::default().fg(NOTICE),
            _ => Style::default().fg(FG),
        };
        let mut first = true;
        for text_line in entry.text.lines() {
            let line = if first {
                first = false;
                Line::from(vec![head.spans[0].clone(), Span::raw(" "), Span::styled(text_line.to_string(), body_style)])
            } else {
                Line::from(Span::styled(format!("  {text_line}"), body_style))
            };
            lines.extend(wrap_lines(vec![line], width));
        }
        if entry.text.is_empty() {
            lines.push(head);
        }
    }
    let total = lines.len();
    let height = area.height as usize;
    app.last_chat_lines = total;
    app.last_chat_height = height.max(1);
    // clamp after new content; scroll == 0 follows the tail
    let max_scroll = total.saturating_sub(height);
    if app.chat_scroll > max_scroll {
        app.chat_scroll = max_scroll;
    }
    let start = total.saturating_sub(height + app.chat_scroll);
    let visible: Vec<Line<'static>> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(Paragraph::new(visible).wrap(Wrap { trim: false }), area);
}

fn render_approvals(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Approvals;
    let title = if focused { "approvals (focused)" } else { "approvals" };
    let block = panel(title).border_style(Style::default().fg(if focused { WARNING } else { ACCENT }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line<'static>> = app
        .approvals
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(i, a)| {
            let marker = if focused && i == app.approval_sel { "▶" } else { " " };
            let style = if focused && i == app.approval_sel { selection() } else { Style::default().fg(FG) };
            Line::from(Span::styled(format!("{marker} {} · {} · {}", a.id, a.tool, a.preview), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_composer(frame: &mut Frame, app: &V2App, area: Rect) {
    let target = app.active_instance().map(|i| i.id.clone()).unwrap_or_else(|| "…".into());
    let block = panel(format!("to {target}")).border_style(Style::default().fg(if app.focus == Focus::Composer {
        ACCENT
    } else {
        GREY
    }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = app.composer.text();
    frame.render_widget(Paragraph::new(text), inner);
    if app.focus == Focus::Composer {
        let row = app.composer.row.min(app.composer.lines.len().saturating_sub(1));
        let col = app.composer.col.min(app.composer.lines.get(row).map(|l| l.len()).unwrap_or(0));
        let prefix: String = app.composer.lines.get(row).map(|l| l.iter().take(col).collect()).unwrap_or_default();
        let x = inner.x + UnicodeWidthStr::width(prefix.as_str()) as u16;
        let y = inner.y + row as u16;
        if x < inner.x + inner.width && y < inner.y + inner.height {
            frame.set_cursor_position((x, y));
        }
    }
}

fn render_footer(frame: &mut Frame, app: &V2App, area: Rect) {
    frame.render_widget(Paragraph::new(app.footer_hint()).style(Style::default().fg(GREY)), area);
}
