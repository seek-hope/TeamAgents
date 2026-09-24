//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::ui::wrap_lines;
use crate::v2app::{ChatKind, Focus, V2App, View};

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
    let block = Block::default().borders(Borders::ALL).title("实例（● 对话目标）");
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
            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let lifecycle_style = match instance.lifecycle.as_str() {
                "ACTIVE" => Style::default().fg(Color::Green),
                "PAUSED" | "PARKED" => Style::default().fg(Color::Yellow),
                _ => Style::default().fg(Color::Red),
            };
            Line::from(vec![
                Span::styled(format!("{marker}{target} "), style),
                Span::styled(instance.id.clone(), style),
                Span::raw(" · "),
                Span::styled(instance.lifecycle.clone(), lifecycle_style),
                Span::raw(format!(" · {}", instance.phase)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tasks(frame: &mut Frame, app: &V2App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("任务");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line<'static>> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let selected = i == app.task_sel;
            let marker = if selected { "▶" } else { " " };
            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let status_style = match task.status.as_str() {
                "RUNNING" => Style::default().fg(Color::Cyan),
                "PENDING" => Style::default().fg(Color::Yellow),
                "SUCCEEDED" => Style::default().fg(Color::Green),
                "FAILED" | "BLOCKED" => Style::default().fg(Color::Red),
                _ => Style::default().fg(Color::DarkGray),
            };
            Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(task.id.clone(), style),
                Span::raw(" · "),
                Span::styled(task.status.clone(), status_style),
                Span::raw(format!(" · 承接 {} · 目标 {}", task.assignee, task.goal_id)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Topology as an edge list (§9: 拓扑先用边列表表达，不把图形画布作为执行
/// 正确性的依赖): active grant/channel edges, then task-delegation edges.
fn render_topology(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let active_grants = app.grants.iter().filter(|g| !g.revoked).count();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("拓扑 · 活跃授权 {active_grants} · 任务 {}", app.tasks.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled("授权与通道", bold))];
    if active_grants == 0 {
        lines.push(Line::from(Span::styled("  （无活跃授权）", dim)));
    }
    for grant in app.grants.iter().filter(|g| !g.revoked) {
        let kind = if grant.action == "message" { "通道" } else { "授权" };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(grant.subject.clone(), Style::default().fg(Color::Cyan)),
            Span::styled(format!(" ─{}→ ", grant.action), dim),
            Span::styled(grant.scope.clone(), Style::default().fg(Color::Cyan)),
            Span::styled(format!("  {kind}"), dim),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("任务委派", bold)));
    if app.tasks.is_empty() {
        lines.push(Line::from(Span::styled("  （无任务）", dim)));
    }
    for task in &app.tasks {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(task.id.clone(), Style::default().fg(Color::Yellow)),
            Span::styled(" ─→ ", dim),
            Span::styled(task.assignee.clone(), Style::default().fg(Color::Cyan)),
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
        Style::default().fg(Color::Black).bg(Color::Red)
    } else {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    };
    frame.render_widget(Paragraph::new(format!(" {} ", app.status_line())).style(style), area);
}

fn render_chat(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let width = area.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for entry in &app.entries {
        let (prefix, style) = match entry.kind {
            ChatKind::User => (entry.who.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            ChatKind::Assistant => (entry.who.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ChatKind::Tool => (entry.who.clone(), Style::default().fg(Color::Yellow)),
            ChatKind::Summary => (entry.who.clone(), Style::default().fg(Color::Magenta)),
            ChatKind::System => (entry.who.clone(), Style::default().fg(Color::DarkGray)),
            ChatKind::Error => (entry.who.clone(), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        };
        let head = Line::from(vec![Span::styled(prefix, style), Span::raw(" ")]);
        let body_style = match entry.kind {
            ChatKind::System => Style::default().fg(Color::DarkGray),
            ChatKind::Error => Style::default().fg(Color::Red),
            ChatKind::Tool => Style::default().fg(Color::Gray),
            ChatKind::Summary => Style::default().fg(Color::Magenta),
            _ => Style::default(),
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
    let title = if focused { "待批准（焦点）" } else { "待批准" };
    let block = Block::default().borders(Borders::ALL).title(title).border_style(if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line<'static>> = app
        .approvals
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(i, a)| {
            let marker = if focused && i == app.approval_sel { "▶" } else { " " };
            let style = if focused && i == app.approval_sel {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!("{marker} {} · {} · {}", a.id, a.tool, a.preview), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_composer(frame: &mut Frame, app: &V2App, area: Rect) {
    let target = app.active_instance().map(|i| i.id.clone()).unwrap_or_else(|| "…".into());
    let block = Block::default().borders(Borders::ALL).title(format!("发给 {target}"));
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
    frame.render_widget(Paragraph::new(app.footer_hint()).style(Style::default().fg(Color::DarkGray)), area);
}
