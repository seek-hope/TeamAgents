//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::theme::*;
use crate::v2app::{ChatKind, Focus, V2App};
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
    pub chat: Rect,
    /// Pending-approvals box (chat view only; height 0 when nothing pends).
    pub approvals: Rect,
    /// Composer box (chat view only).
    pub composer: Rect,
    pub footer: Rect,
}

pub fn geometry(app: &V2App, area: Rect) -> V2Geometry {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).split(area);
    let content = rows[1];
    let composer_h = (app.composer.widget_height(content.width.saturating_sub(2) as usize) as u16)
        .min(content.height.saturating_sub(1));
    let approvals_h = if app.approvals.is_empty() {
        0
    } else {
        (app.approvals.len().min(3) as u16 + 2).min(content.height.saturating_sub(composer_h).saturating_sub(1))
    };
    let chunks =
        Layout::vertical([Constraint::Min(0), Constraint::Length(approvals_h), Constraint::Length(composer_h)])
            .split(content);
    V2Geometry { status: rows[0], chat: chunks[0], approvals: chunks[1], composer: chunks[2], footer: rows[2] }
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

/// Pointer routing uses the rendered rectangles; selecting an approval never decides it.
pub fn handle_mouse(app: &mut V2App, area: Rect, mouse: crossterm::event::MouseEvent) {
    use crossterm::event::{MouseButton, MouseEventKind};
    let geo = geometry(app, area);
    let point = ratatui::layout::Position::new(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) = approval_at(&geo, app, mouse.row, mouse.column) {
                app.focus = Focus::Approvals;
                app.approval_sel = index;
            } else if geo.composer.contains(point) {
                app.focus = Focus::Composer;
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = mouse.kind == MouseEventKind::ScrollUp;
            if geo.approvals.contains(point) {
                app.approval_sel = if up {
                    app.approval_sel.saturating_sub(1)
                } else {
                    app.approval_sel.saturating_add(1).min(app.approvals.len().saturating_sub(1))
                };
            } else if geo.chat.contains(point) {
                if up {
                    app.scroll_chat(3);
                } else {
                    app.scroll_chat_back(3);
                }
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
    if let Some(browser) = app.browser.as_mut() {
        crate::sessions::render(frame, browser, &app.session_root);
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
    render_footer(frame, app, geo.footer);
}

fn render_status(frame: &mut Frame, app: &V2App, area: Rect) {
    let style = Style::default().fg(if app.disconnected { ERROR } else { NOTICE });
    let name: String = app.session_name.chars().take(32).collect();
    let text =
        if name.is_empty() { format!(" {}", app.status_line()) } else { format!(" {name} · {}", app.status_line()) };
    frame.render_widget(Paragraph::new(text).style(style), area);
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
                "Tell the Leader what you want to do. /help lists the few controls.",
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
    if let Some(goal) = &app.goal {
        if goal.status != "ACTIVE" {
            if let Some(summary) = &goal.summary {
                lines.extend(wrap_lines(
                    vec![Line::styled(format!("Result: {summary}"), Style::default().fg(NOTICE))],
                    width,
                ));
            }
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
    let focused = app.focus == Focus::Approvals;
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
    let focused = app.focus == Focus::Composer;
    let block = panel("Message").border_style(Style::default().fg(if focused { ACCENT } else { GREY }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (lines, row, col) = app.composer.visual_lines(inner.width as usize);
    let (start, count) = window(lines.len(), row, inner.height as usize);
    let visible = lines.into_iter().skip(start).take(count).map(Line::raw).collect::<Vec<_>>();
    if app.composer.text().is_empty() {
        frame.render_widget(Paragraph::new("Message the Leader").style(Style::default().fg(GREY)), inner);
    } else {
        frame.render_widget(Paragraph::new(visible), inner);
    }
    if focused && count > 0 {
        frame.set_cursor_position((inner.x + col as u16, inner.y + (row - start) as u16));
    }
}

fn render_footer(frame: &mut Frame, app: &V2App, area: Rect) {
    let hint = if app.chat_scroll > 0 {
        format!("History · {} lines above latest · PgDn newer · {}", app.chat_scroll, app.footer_hint())
    } else {
        app.footer_hint()
    };
    let style = Style::default().fg(GREY);
    frame.render_widget(Paragraph::new(hint).style(style), area);
}
