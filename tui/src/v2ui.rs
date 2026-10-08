//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::theme::*;
use crate::v2app::{ChatKind, Focus, V2App, View};
use crate::viewport::{item_at, window};
use crate::wrap::wrap_lines;

/// Quiet panel chrome; focus is identified by the title and selected row.
fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title.into(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
        .border_style(Style::default().fg(GREY))
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
    /// Conversation remains visible beside an open panel on a wide terminal.
    pub chat: Rect,
    /// Pending-approvals box (chat view only; height 0 when nothing pends).
    pub approvals: Rect,
    /// Composer box (chat view only).
    pub composer: Rect,
    pub footer: Rect,
}

pub fn geometry(app: &V2App, area: Rect) -> V2Geometry {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).split(area);
    // ponytail: a fixed breakpoint keeps a useful 72-column conversation plus
    // a 48-column team panel. A draggable split can replace it if needed.
    let sidebar = app.view != View::Chat && area.width >= 120 && area.height >= 12;
    let columns = Layout::horizontal([Constraint::Min(72), Constraint::Length(48)]).split(rows[1]);
    let content = if sidebar { columns[0] } else { rows[1] };
    let chat_visible = app.view == View::Chat || sidebar;
    let composer_h = if chat_visible {
        (app.composer.widget_height(content.width.saturating_sub(2) as usize) as u16)
            .min(content.height.saturating_sub(1))
    } else {
        0
    };
    let approvals_h = if chat_visible && !app.approvals.is_empty() {
        (app.approvals.len().min(3) as u16 + 2).min(content.height.saturating_sub(composer_h).saturating_sub(1))
    } else {
        0
    };
    let chunks =
        Layout::vertical([Constraint::Min(0), Constraint::Length(approvals_h), Constraint::Length(composer_h)])
            .split(content);
    V2Geometry {
        status: rows[0],
        body: if sidebar { columns[1] } else { chunks[0] },
        chat: if chat_visible { chunks[0] } else { Rect::default() },
        approvals: chunks[1],
        composer: chunks[2],
        footer: rows[2],
    }
}

/// Row inside the approvals box a click maps to (border + padding), or None.
pub fn approval_at(geo: &V2Geometry, app: &V2App, row: u16, col: u16) -> Option<usize> {
    let area = geo.approvals;
    if area.height < 3
        || col <= area.x
        || col >= area.right().saturating_sub(1)
        || row <= area.y
        || row >= area.y + area.height - 1
    {
        return None;
    }
    let index = (row - area.y - 1) as usize;
    item_at(app.approvals.len(), app.approval_sel, area.height.saturating_sub(2) as usize, index)
}

/// List row inside a panel body a click maps to (border + one line per
/// row), or None. The view decides what the index means.
pub fn panel_row_at(geo: &V2Geometry, row: u16, col: u16) -> Option<usize> {
    let area = geo.body;
    if area.height < 3
        || col <= area.x
        || col >= area.right().saturating_sub(1)
        || row <= area.y
        || row >= area.y + area.height - 1
    {
        return None;
    }
    Some((row - area.y - 1) as usize)
}

/// Route the pointer through the same rectangles and list windows as rendering.
/// Clicking a row only selects it; approvals still require an explicit key.
pub fn handle_mouse(app: &mut V2App, area: Rect, mouse: crossterm::event::MouseEvent) {
    use crossterm::event::{MouseButton, MouseEventKind};
    if app.confirm.is_some() {
        return;
    }
    let geo = geometry(app, area);
    let point = ratatui::layout::Position::new(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) = approval_at(&geo, app, mouse.row, mouse.column) {
                app.view = View::Chat;
                app.focus = Focus::Approvals;
                app.approval_sel = index;
            } else if geo.composer.contains(point) {
                app.view = View::Chat;
                app.focus = Focus::Composer;
            } else if app.view != View::Chat {
                if let Some(row) = panel_row_at(&geo, mouse.row, mouse.column) {
                    let height = geo.body.height.saturating_sub(2) as usize;
                    match app.view {
                        View::Instances => {
                            if let Some(index) = item_at(app.instances.len(), app.instance_sel, height, row) {
                                app.instance_sel = index;
                            }
                        }
                        View::Tasks => {
                            if let Some(index) = item_at(app.tasks.len(), app.task_sel, height, row) {
                                app.task_sel = index;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = mouse.kind == MouseEventKind::ScrollUp;
            if geo.approvals.contains(point) {
                app.approval_sel = if up {
                    app.approval_sel.saturating_sub(1)
                } else {
                    (app.approval_sel + 1).min(app.approvals.len().saturating_sub(1))
                };
            } else if geo.chat.contains(point) {
                if up {
                    app.scroll_chat(3);
                } else {
                    app.scroll_chat_back(3);
                }
            } else if app.view != View::Chat && geo.body.contains(point) {
                app.wheel(up);
            }
        }
        _ => {}
    }
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
    if geo.chat.height > 0 {
        render_chat(frame, app, geo.chat);
    }
    render_approvals(frame, app, geo.approvals);
    render_composer(frame, app, geo.composer);
    match app.view {
        View::Chat => {}
        View::Instances => render_instances(frame, app, geo.body),
        View::Tasks => render_tasks(frame, app, geo.body),
        View::Topology => render_topology(frame, app, geo.body),
    }
    render_footer(frame, app, geo.footer);
}

fn render_instances(frame: &mut Frame, app: &V2App, area: Rect) {
    let block = panel("instances (● conversation target)").title_bottom(
        app.instances
            .get(app.instance_sel)
            .map(|i| format!(" {} · {} ", i.phase, i.model))
            .unwrap_or_else(|| " No team members yet ".into()),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (start, count) = window(app.instances.len(), app.instance_sel, inner.height as usize);
    let lines: Vec<Line<'static>> = app
        .instances
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
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
            let mut spans = vec![
                Span::styled(format!("{marker}{target} "), style),
                Span::styled(instance.id.clone(), style),
                Span::styled(" · ", Style::default().fg(GREY)),
                Span::styled(instance.lifecycle.clone(), lifecycle_style),
                Span::styled(format!(" · {}", instance.phase), Style::default().fg(NOTICE)),
            ];
            // which model a member runs on (D-69): a team can span providers
            if !instance.model.is_empty() && inner.width >= 60 {
                spans.push(Span::styled(format!(" · {}", instance.model), Style::default().fg(GREY)));
            }
            // D-442: a member that cannot run commands says so where the user looks, instead of leaving them to
            // wonder why it is idle. The lever is one CLI command away and the guide names it.
            if !instance.can_run_shell && instance.lifecycle == "ACTIVE" {
                spans.push(Span::styled(" · no shell", Style::default().fg(WARNING)));
            }
            // why it is not running, in the runtime's own words (D-165): the panel said `PARKED` and left the
            // user to open a log. An ACTIVE member's last transition carries nothing worth a column.
            if instance.lifecycle != "ACTIVE" && !instance.reason.is_empty() {
                spans.push(Span::styled(format!("  — {}", instance.reason), Style::default().fg(NOTICE)));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tasks(frame: &mut Frame, app: &V2App, area: Rect) {
    let block = panel("tasks").title_bottom(
        app.tasks
            .get(app.task_sel)
            .map(|t| format!(" {} · {} ", t.assignee, t.goal_id))
            .unwrap_or_else(|| " No delegated tasks yet ".into()),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (start, count) = window(app.tasks.len(), app.task_sel, inner.height as usize);
    let lines: Vec<Line<'static>> = app
        .tasks
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
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
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(grant.subject.clone(), Style::default().fg(FG)),
            Span::styled(format!(" ─{}→ ", grant.action), Style::default().fg(ACCENT)),
            Span::styled(grant.scope.clone(), Style::default().fg(NOTICE)),
            Span::styled(format!("  {kind}"), dim),
        ];
        // the id is what `teamagents authority revoke --grant …` takes (D-61):
        // show its short form so the user does not have to look it up first
        if !grant.id.is_empty() {
            spans.push(Span::styled(format!("  {}", grant.short_id()), dim));
        }
        lines.push(Line::from(spans));
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
    app.last_topo_height = height.max(1);
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
    let instance =
        app.active_instance().map(|i| format!("{} [{}]", i.id, i.phase)).unwrap_or_else(|| "no instance".into());
    let state = if app.disconnected {
        "DISCONNECTED · reconnecting".to_string()
    } else if !app.approvals.is_empty() {
        format!("{} pending approvals", app.approvals.len())
    } else {
        app.view.name().to_string()
    };
    let lines = vec![
        Line::from(format!(" {state} · {instance} · session {}", app.session_id)),
        Line::styled(format!(" {}", app.goal_status()), Style::default().fg(NOTICE)),
    ];
    frame.render_widget(Paragraph::new(lines).style(style), area);
}

fn render_chat(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let area = Rect::new(area.x + u16::from(area.width > 2), area.y, area.width.saturating_sub(2), area.height);
    let width = area.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.entries.is_empty() && app.active_preview().is_none() {
        lines.push(Line::from(""));
        lines.push(Line::styled("TeamAgents", Style::default().fg(FG).add_modifier(Modifier::BOLD)));
        lines.push(Line::from(""));
        lines.extend(wrap_lines(
            vec![Line::styled(
                "Start a conversation below. Ctrl+N opens the team panels; /help lists commands.",
                Style::default().fg(GREY),
            )],
            width,
        ));
    }
    for entry in &app.entries {
        if !lines.is_empty()
            && matches!(entry.kind, ChatKind::User | ChatKind::Assistant | ChatKind::Command | ChatKind::Error)
        {
            lines.push(Line::from(""));
        }
        // v1 palette convention: grey bold labels, white body text, accent for
        // the assistant, notice grey for everything machine-generated
        let (prefix, style) = match entry.kind {
            ChatKind::User => (entry.who.to_string(), Style::default().fg(GREY).add_modifier(Modifier::BOLD)),
            ChatKind::Assistant => (entry.who.clone(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            ChatKind::Tool => (entry.who.clone(), Style::default().fg(GREY).add_modifier(Modifier::BOLD)),
            ChatKind::Runtime => (entry.who.clone(), Style::default().fg(GREY)),
            ChatKind::Summary => (entry.who.clone(), Style::default().fg(ACCENT)),
            ChatKind::System => (entry.who.clone(), Style::default().fg(GREY)),
            // D-433: a command's answer stands out from runtime chatter (grey bold label) and stays on screen
            ChatKind::Command => (entry.who.clone(), Style::default().fg(NOTICE).add_modifier(Modifier::BOLD)),
            ChatKind::Error => (entry.who.clone(), Style::default().fg(ERROR).add_modifier(Modifier::BOLD)),
        };
        let head = Line::from(vec![Span::styled(prefix, style), Span::styled(" ", Style::default().fg(GREY))]);
        let body_style = match entry.kind {
            ChatKind::System => Style::default().fg(NOTICE),
            ChatKind::Command => Style::default().fg(FG),
            ChatKind::Error => Style::default().fg(ERROR),
            ChatKind::Tool => Style::default().fg(NOTICE),
            ChatKind::Runtime => Style::default().fg(NOTICE),
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
    if let Some(preview) = app.active_preview() {
        // §9: a preview is never an authoritative fact — it renders as a transient dimmed line and disappears the
        // moment the complete response lands. Only the tail is shown, so a long stream stays bounded.
        let tail: String = preview.chars().rev().take(400).collect::<String>().chars().rev().collect();
        let mut first = true;
        for text_line in tail.lines() {
            let line = if first {
                first = false;
                Line::from(vec![
                    Span::styled("…", Style::default().fg(GREY).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(text_line.to_string(), Style::default().fg(NOTICE)),
                ])
            } else {
                Line::from(Span::styled(format!("  {text_line}"), Style::default().fg(NOTICE)))
            };
            lines.extend(wrap_lines(vec![line], width));
        }
    }
    let total = lines.len();
    let height = area.height as usize;
    if app.chat_scroll > 0 && app.last_chat_width == width {
        app.chat_scroll = app.chat_scroll.saturating_add(total.saturating_sub(app.last_chat_lines));
    }
    app.last_chat_lines = total;
    app.last_chat_width = width;
    app.last_chat_height = height.max(1);
    // clamp after new content; scroll == 0 follows the tail
    let max_scroll = total.saturating_sub(height);
    if app.chat_scroll > max_scroll {
        app.chat_scroll = max_scroll;
    }
    let start = total.saturating_sub(height + app.chat_scroll);
    let visible: Vec<Line<'static>> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(Paragraph::new(visible), area);
    if total > height && area.width > 0 {
        let mut state = ScrollbarState::new(max_scroll + 1).position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(GREY))
                .track_symbol(None),
            Rect::new(area.right(), area.y, 1, area.height),
            &mut state,
        );
    }
}

fn render_approvals(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let focused = app.view == View::Chat && app.focus == Focus::Approvals;
    let title = if focused { "approvals (focused)" } else { "approvals" };
    let block = panel(title).border_style(Style::default().fg(if focused { WARNING } else { ACCENT }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (start, count) = window(app.approvals.len(), app.approval_sel, inner.height as usize);
    let lines: Vec<Line<'static>> = app
        .approvals
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(i, a)| {
            let marker = if focused && i == app.approval_sel { "▶" } else { " " };
            let style = if focused && i == app.approval_sel { selection() } else { Style::default().fg(FG) };
            Line::from(Span::styled(format!("{marker} {} · {} · {}", a.id, a.tool, a.preview), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_composer(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height < 3 || area.width < 3 {
        return;
    }
    let focused = app.view == View::Chat && app.focus == Focus::Composer;
    let target = app
        .active_instance()
        .map(|i| if i.model.is_empty() { i.id.clone() } else { format!("{} · {}", i.id, i.model) })
        .unwrap_or_else(|| "…".into());
    let block = panel(format!("to {target}")).border_style(Style::default().fg(if focused { ACCENT } else { GREY }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (lines, row, col) = app.composer.visual_lines(inner.width as usize);
    let (start, count) = window(lines.len(), row, inner.height as usize);
    let visible = lines.into_iter().skip(start).take(count).map(Line::raw).collect::<Vec<_>>();
    if app.composer.text().is_empty() {
        frame.render_widget(Paragraph::new("Message the team, or /help").style(Style::default().fg(GREY)), inner);
    } else {
        frame.render_widget(Paragraph::new(visible), inner);
    }
    if focused && count > 0 {
        frame.set_cursor_position((inner.x + col as u16, inner.y + (row - start) as u16));
    }
}

fn render_footer(frame: &mut Frame, app: &V2App, area: Rect) {
    let hint = if app.chat_scroll > 0 && app.view == View::Chat {
        format!("History · {} lines above latest · PgDn newer · {}", app.chat_scroll, app.footer_hint())
    } else {
        app.footer_hint()
    };
    let style = Style::default().fg(if app.confirm.is_some() { WARNING } else { GREY });
    frame.render_widget(Paragraph::new(hint).style(style), area);
}
