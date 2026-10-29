//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::theme::*;
use crate::v2app::{Focus, V2App};
use crate::viewport::{item_at, window};

/// Shared quiet panel chrome; focus is identified by the border and selected row.
pub(crate) fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(format!(" {} ", title.into()), secondary()))
        .border_style(border(false))
        .style(Style::default().bg(PANEL_BG).fg(FG))
}

/// A selected row keeps a textual marker; color and bold never work alone.
pub(crate) fn selection() -> Style {
    selected()
}

/// Screen regions shared by rendering and hit-testing.
pub struct V2Geometry {
    pub status: Rect,
    pub chat: Rect,
    /// Grok-style live activity row, shown only when there is something meaningful to report.
    pub activity: Rect,
    /// Pending-approvals box (chat view only; height 0 when nothing pends).
    pub approvals: Rect,
    /// Composer box (chat view only).
    pub composer: Rect,
    pub completion: Rect,
    pub footer: Rect,
}

pub fn geometry(app: &V2App, area: Rect) -> V2Geometry {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).split(area);
    let content = rows[1];
    let composer_h = (app.composer.widget_height(content.width.saturating_sub(4) as usize) as u16)
        .min(content.height.saturating_sub(1));
    let approvals_h = if app.approvals.is_empty() {
        0
    } else {
        (app.approvals.len().min(3) as u16 + 2).min(content.height.saturating_sub(composer_h).saturating_sub(1))
    };
    let completion_h = if app.focus == Focus::Composer {
        let count = if app.attachments.visible(&app.composer) {
            app.attachments.candidates.len().min(8)
        } else {
            app.completion.candidates(&app.composer).len()
        };
        (count as u16).min(content.height.saturating_sub(composer_h + approvals_h).saturating_sub(1))
    } else {
        0
    };
    let activity_h = u16::from(content.height >= 3 && !app.activity_line().starts_with("Ready"));
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(activity_h),
        Constraint::Length(approvals_h),
        Constraint::Length(completion_h),
        Constraint::Length(composer_h),
    ])
    .split(content);
    V2Geometry {
        status: rows[0],
        chat: chunks[0],
        activity: chunks[1],
        approvals: chunks[2],
        completion: chunks[3],
        composer: chunks[4],
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

/// Pointer routing uses the rendered rectangles; selecting an approval never decides it.
pub fn handle_mouse(app: &mut V2App, area: Rect, mouse: crossterm::event::MouseEvent) {
    use crossterm::event::{MouseButton, MouseEventKind};
    if app.help_open {
        match mouse.kind {
            MouseEventKind::ScrollUp => app.help_scroll = app.help_scroll.saturating_sub(3),
            MouseEventKind::ScrollDown => app.help_scroll = app.help_scroll.saturating_add(3),
            _ => {}
        }
        return;
    }
    if app.queue.is_some() || app.profile.is_some() {
        return;
    }
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
    app.pictures.clear();
    app.graphics.begin_frame();
    app.graphics.deferred = true;
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(queue) = &app.queue {
        crate::queue::render(frame, queue);
        return;
    }
    if let Some(browser) = app.browser.as_mut() {
        app.pictures = crate::sessions::render(frame, browser, &app.session_root, &mut app.graphics);
        return;
    }
    let geo = geometry(app, area);
    // the palette's screen background, drawn first so every widget sits on it
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    render_status(frame, app, geo.status);
    if geo.chat.height > 0 {
        render_chat(frame, app, geo.chat);
    }
    render_activity(frame, app, geo.activity);
    render_approvals(frame, app, geo.approvals);
    render_completion(frame, app, geo.completion);
    render_composer(frame, app, geo.composer);
    render_footer(frame, app, geo.footer);
    render_profile(frame, app, area);
    if app.profile.is_some() {
        app.pictures.clear();
    }
    if app.help_open {
        render_help(frame, app, area);
    }
}

/// D-473: a centered model/effort menu over the conversation; keys are routed by `V2App::profile_key`.
fn render_profile(frame: &mut Frame, app: &V2App, area: Rect) {
    let Some(menu) = &app.profile else { return };
    let popup = dialog_area(area, 72, (menu.models.len().min(7) + 7) as u16);
    frame.render_widget(Clear, popup);
    let block = panel("Model & effort").border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let effort = menu.efforts.get(menu.effort_ix).cloned().unwrap_or_else(|| "?".into());
    let list_height = inner.height.saturating_sub(4).max(1) as usize;
    let (start, count) = window(menu.models.len(), menu.model_ix, list_height);
    let mut lines: Vec<Line<'static>> = menu
        .models
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(i, model)| {
            let label =
                if model.key == model.model { model.key.clone() } else { format!("{} · {}", model.key, model.model) };
            Line::styled(
                fit(&format!("{} {}", if i == menu.model_ix { ">" } else { " " }, label), inner.width as usize),
                if i == menu.model_ix { selection() } else { secondary() },
            )
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("Effort  ", secondary()),
        Span::styled(fit(&format!("‹ {effort} ›"), inner.width.saturating_sub(8) as usize), selection()),
    ]));
    lines.extend(crate::markdown::plain(
        "Applies at the next turn. Enter applies; Esc cancels.",
        inner.width as usize,
        secondary(),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn compact_tokens(value: i64) -> String {
    let value = value.max(0) as f64;
    if value < 10_000.0 {
        return format!("{}", value as i64);
    }
    let (scaled, suffix) = if value < 1_000_000.0 { (value / 1_000.0, "k") } else { (value / 1_000_000.0, "M") };
    let mut text = format!("{scaled:.1}");
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    format!("{text}{suffix}")
}

/// Grok Build's quiet workspace header, with TeamAgents' authoritative goal usage on the right.
fn render_status(frame: &mut Frame, app: &V2App, area: Rect) {
    let status = app.activity_line();
    let status_style = if app.disconnected {
        Style::default().fg(ERROR)
    } else if !app.approvals.is_empty() {
        Style::default().fg(WARNING)
    } else if status.starts_with("Ready") {
        Style::default().fg(SUCCESS)
    } else {
        Style::default().fg(NOTICE)
    };
    let width = area.width as usize;
    let usage = app.goal.as_ref().map(|goal| {
        let limit = goal.limit_total.map(|n| format!("/{}", compact_tokens(n))).unwrap_or_default();
        format!("{}{limit} tokens{}", compact_tokens(goal.known_total), if goal.unknown { " + ?" } else { "" })
    });
    let right = fit(usage.as_deref().unwrap_or(""), width.saturating_div(3));
    let right_width = if right.is_empty() { 0 } else { unicode_width::UnicodeWidthStr::width(right.as_str()) + 2 };
    let left_width = width.saturating_sub(right_width);
    let mut left = Line::from(Span::styled(" ", Style::default()));
    left.spans.push(Span::styled(
        if left_width >= 20 { "◆ TeamAgents" } else { "TA" },
        Style::default().fg(CODEX).add_modifier(Modifier::BOLD),
    ));
    if left_width >= 12 {
        left.spans.push(Span::styled(" · ", secondary()));
        left.spans.push(Span::styled(fit(&status, 28), status_style));
    }
    if left_width >= 54 && !app.session_name.is_empty() {
        left.spans.push(Span::styled(" · ", secondary()));
        left.spans.push(Span::styled(fit(&app.session_name, 18), secondary()));
    }
    if left_width >= 76 {
        if let Some(workspace) = app.workspace() {
            left.spans.push(Span::styled("  ", Style::default()));
            left.spans.push(Span::styled(fit(workspace, 42), secondary()));
        }
    }
    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(PANEL_BG)),
        Rect::new(area.x, area.y, left_width as u16, 1),
    );
    if !right.is_empty() && area.width >= 2 {
        let right_area = Rect::new(area.right().saturating_sub(right.len() as u16 + 1), area.y, right.len() as u16, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(right, secondary())))
                .alignment(Alignment::Right)
                .style(Style::default().bg(PANEL_BG)),
            right_area,
        );
    }
}

/// One live status row between the transcript and composer. It intentionally reports
/// persisted/runtime state only; no spinner or elapsed prose is treated as progress evidence.
fn render_activity(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let status = app.activity_line();
    let warning = !app.approvals.is_empty() || status.starts_with("Needs") || status.starts_with("Approval");
    let stopped = app.disconnected
        || status.starts_with("Work blocked")
        || status.starts_with("Stopped")
        || status.starts_with("PARKED")
        || status.starts_with("TERMINATED");
    let busy = app.active_instance().is_some_and(|instance| {
        instance.lifecycle == "ACTIVE"
            && matches!(instance.phase.as_str(), "MODEL_PENDING" | "TOOLS_PENDING" | "COMPLETION_PENDING")
    }) || matches!(
        status.split(" · ").next(),
        Some("Working" | "Thinking" | "Checking" | "Connecting to the Leader")
    );
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let icon = if stopped {
        "!"
    } else if warning {
        "◆"
    } else if busy {
        spinner[(crate::now_epoch() * 8.0) as usize % spinner.len()]
    } else if status.starts_with("Waiting") {
        "◌"
    } else {
        "●"
    };
    let color = if stopped {
        ERROR
    } else if warning {
        WARNING
    } else if status.starts_with("Ready") {
        SUCCESS
    } else {
        ACCENT
    };
    let right = if !app.approvals.is_empty() {
        "Ctrl+A review"
    } else if app.active_instance().is_some_and(|instance| {
        instance.lifecycle == "ACTIVE" && instance.phase != "READY" && instance.phase != "WAITING"
    }) {
        "Ctrl+C interrupt"
    } else if app.chat_scroll > 0 {
        "PgDn latest"
    } else {
        ""
    };
    let right_width = if area.width >= 48 { right.len() + 2 } else { 0 };
    let left_width = (area.width as usize).saturating_sub(right_width + 3);
    let line = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(fit(&status, left_width), Style::default().fg(if stopped { ERROR } else { NOTICE })),
    ]);
    frame.render_widget(
        Paragraph::new(line),
        Rect::new(area.x, area.y, area.width.saturating_sub(right_width as u16), 1),
    );
    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(right, secondary()))).alignment(Alignment::Right),
            Rect::new(area.right().saturating_sub(right.len() as u16 + 1), area.y, right.len() as u16, 1),
        );
    }
}

fn render_chat(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let area = Rect::new(area.x + u16::from(area.width > 2), area.y, area.width.saturating_sub(2), area.height);
    let width = area.width as usize;
    let mut document = app
        .transcript
        .document(
            &app.entries,
            width,
            Some(if app.expanded_tools { 40 } else { 4 }),
            Some(&mut app.graphics),
            &app.session_root,
        )
        .clone();
    let lines = &mut document.lines;
    if lines.is_empty() && app.active_preview().is_none() {
        lines.push(Line::from(""));
        lines.push(Line::styled("TeamAgents", Style::default().fg(CODEX).add_modifier(Modifier::BOLD)));
        lines.push(Line::from(""));
        lines.extend(crate::markdown::plain(
            "A persistent coding team in your terminal.",
            width,
            Style::default().fg(FG),
        ));
        lines.push(Line::from(""));
        lines.extend(crate::markdown::plain(
            "Ask the Leader to explore, fix or create something. Work continues when you detach.",
            width,
            secondary(),
        ));
        lines.push(Line::from(""));
        lines.extend(crate::markdown::plain(
            "?  Shortcuts     /  Commands     Ctrl+S  Conversations",
            width,
            secondary(),
        ));
        if let Some(workspace) = app.workspace() {
            lines.push(Line::default());
            lines.extend(crate::markdown::plain(&format!("Workspace  {workspace}"), width, secondary()));
        }
    }
    if let Some(goal) = &app.goal {
        if goal.status != "ACTIVE" {
            if let Some(summary) = &goal.summary {
                lines.push(Line::default());
                lines.extend(crate::markdown::plain(&format!("Result · {}", goal.status), width, secondary()));
                let origin = crate::graphics::Origin {
                    session: app.session_root.clone().into(),
                    workspace: app.workspace().unwrap_or_default().into(),
                    member: "i-leader".into(),
                    position: usize::MAX - 1,
                };
                document.append(crate::transcript::assistant_document(
                    summary,
                    "i-leader",
                    width,
                    Some(&mut app.graphics),
                    &origin,
                    false,
                ));
            }
        }
    }
    if let Some(preview) = app.active_preview() {
        // The daemon bounds this transient text. Keep its beginning so Markdown and scrolling are stable.
        if !document.lines.is_empty() {
            document.lines.push(Line::default());
        }
        let preview = preview.to_string();
        let workspace =
            app.entries.iter().rev().find(|e| !e.workspace.is_empty()).map(|e| e.workspace.clone()).unwrap_or_default();
        let origin = crate::graphics::Origin {
            session: app.session_root.clone().into(),
            workspace: workspace.into(),
            member: "i-leader".into(),
            position: usize::MAX,
        };
        document.append(crate::transcript::assistant_document(
            &preview,
            "i-leader",
            width,
            Some(&mut app.graphics),
            &origin,
            true,
        ));
    }
    let total = document.lines.len();
    let height = area.height as usize;
    let previous = if app.last_chat_width == width { app.last_chat_lines } else { total };
    let (start, offset) = crate::viewport::transcript_window(total, previous, height, app.chat_scroll);
    app.chat_scroll = offset;
    app.last_chat_lines = total;
    app.last_chat_width = width;
    app.last_chat_height = height.max(1);
    let max_scroll = total.saturating_sub(height);
    document.resolve(&mut app.graphics, start, area);
    app.pictures = document.visible(start, area);
    let visible: Vec<Line<'static>> = document.lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(Paragraph::new(visible), area);
    if total > height && area.width > 0 {
        let mut state = ScrollbarState::new(max_scroll + 1).position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
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
    let title = if focused { "Approval required · focused" } else { "Approval required" };
    let approval_border =
        if focused { Style::default().fg(WARNING).add_modifier(Modifier::BOLD) } else { border(false) };
    let block = panel(title).title_bottom(" a approve once · d deny ").border_style(approval_border);
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
            let selected = focused && i == app.approval_sel;
            let marker = if selected { "▶" } else { " " };
            if selected {
                return Line::from(Span::styled(
                    fit(&format!("{marker} {} · {} · {}", a.id, a.tool, a.preview), inner.width as usize),
                    selection(),
                ));
            }
            Line::from(vec![
                Span::styled(marker.to_string(), Style::default().fg(ACCENT)),
                Span::styled(format!(" {} ", a.id), secondary()),
                Span::styled("· ", secondary()),
                Span::styled(a.tool.clone(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
                Span::styled(" · ", secondary()),
                Span::styled(
                    fit(&a.preview, (inner.width as usize).saturating_sub(a.id.len() + a.tool.len() + 12)),
                    secondary(),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_completion(frame: &mut Frame, app: &V2App, area: Rect) {
    if app.attachments.visible(&app.composer) {
        let (start, count) = window(app.attachments.candidates.len(), app.attachments.selected, area.height as usize);
        let lines = app
            .attachments
            .candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(count)
            .map(|(i, path)| {
                Line::styled(
                    format!("{} {path}", if i == app.attachments.selected { ">" } else { " " },),
                    if i == app.attachments.selected { selection() } else { secondary() },
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let candidates = app.completion.candidates(&app.composer);
    let selected = app.completion.selected(&app.composer).min(candidates.len().saturating_sub(1));
    let (start, count) = window(candidates.len(), selected, area.height as usize);
    let lines = candidates
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(index, (name, description))| {
            Line::styled(
                format!("{} {name}  {description}", if index == selected { ">" } else { " " }),
                if index == selected { selection() } else { secondary() },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Main conversation prompt: Grok's quiet border, leading prompt glyph and model info
/// integrated into the lower border. The queue editor keeps using `render_editor` below.
fn render_composer(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height < 3 || area.width < 3 {
        return;
    }
    let focused = app.focus == Focus::Composer && app.profile.is_none() && !app.help_open;
    let title = if app.session_name.is_empty() { " ".to_string() } else { format!(" {} ", fit(&app.session_name, 24)) };
    let block = panel(title).border_style(border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let prefix_width = if inner.width >= 4 { 2 } else { 0 };
    let (lines, row, col) = app.composer.visual_lines(inner.width.saturating_sub(prefix_width) as usize);
    let (start, count) = window(lines.len(), row, inner.height as usize);
    if app.composer.text().is_empty() {
        let placeholder = if app.disconnected { "Reconnecting · draft preserved" } else { "Message the Leader…" };
        let mut spans = Vec::new();
        if prefix_width > 0 {
            spans.push(Span::styled("❯ ", Style::default().fg(USER)));
        }
        spans.push(Span::styled(fit(placeholder, inner.width.saturating_sub(prefix_width) as usize), secondary()));
        frame.render_widget(Paragraph::new(Line::from(spans)), inner);
    } else {
        let visible = lines
            .into_iter()
            .skip(start)
            .take(count)
            .enumerate()
            .map(|(index, text)| {
                let mut spans = Vec::new();
                if prefix_width > 0 {
                    spans.push(Span::styled(if start + index == 0 { "❯ " } else { "  " }, Style::default().fg(USER)));
                }
                spans.push(Span::styled(text, Style::default().fg(FG)));
                Line::from(spans)
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(visible), inner);
    }
    if focused && count > 0 && inner.width > prefix_width {
        frame.set_cursor_position((inner.x + prefix_width + col as u16, inner.y + (row - start) as u16));
    }
    // Model/effort and attachment metadata live in the border, not another permanent status row.
    let bottom = Rect::new(inner.x, area.bottom().saturating_sub(1), inner.width, 1);
    let attachment = if !app.attachments.paths.is_empty() && bottom.width >= 16 {
        let text = if bottom.width >= 32 {
            format!("{} attached · /attach", app.attachments.paths.len())
        } else {
            format!("{} @", app.attachments.paths.len())
        };
        Some(fit(&text, (bottom.width as usize / 3).min(24)))
    } else {
        None
    };
    let reserved = attachment.as_ref().map_or(0, |text| unicode_width::UnicodeWidthStr::width(text.as_str()) + 2);
    if let Some(leader) = app.active_instance() {
        if !leader.model.is_empty() && bottom.width as usize > reserved + 10 {
            let profile = if leader.effort.is_empty() {
                leader.model.clone()
            } else {
                format!("{} · {}", leader.model, leader.effort)
            };
            let profile = format!(" {} ", fit(&profile, (bottom.width as usize).saturating_sub(reserved + 2)));
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(profile, secondary()))).alignment(Alignment::Right),
                bottom,
            );
        }
    }
    if let Some(label) = attachment {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(label, Style::default().fg(ACCENT)))), bottom);
    }
}

/// Shared cursor-following editor for messages and pending inputs.
pub(crate) fn render_editor(
    frame: &mut Frame,
    composer: &crate::text::Composer,
    area: Rect,
    title: &str,
    focused: bool,
    placeholder: &str,
) {
    let block = panel(title).border_style(border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (lines, row, col) = composer.visual_lines(inner.width as usize);
    let (start, count) = window(lines.len(), row, inner.height as usize);
    let visible = lines.into_iter().skip(start).take(count).map(Line::raw).collect::<Vec<_>>();
    if composer.text().is_empty() {
        frame.render_widget(Paragraph::new(placeholder).style(secondary()), inner);
    } else {
        frame.render_widget(Paragraph::new(visible), inner);
    }
    if focused && count > 0 && inner.width > 0 {
        frame.set_cursor_position((inner.x + col as u16, inner.y + (row - start) as u16));
    }
}

fn render_footer(frame: &mut Frame, app: &V2App, area: Rect) {
    let hint = app.footer_hint();
    // Keep complete actions in priority order rather than cutting a shortcut in half.
    let mut shown = String::new();
    for part in hint.split(" · ") {
        let candidate = if shown.is_empty() { part.to_string() } else { format!("{shown} · {part}") };
        if unicode_width::UnicodeWidthStr::width(candidate.as_str()) <= area.width as usize {
            shown = candidate;
        }
    }
    frame.render_widget(Paragraph::new(hint_line(&shown)).style(Style::default().bg(PANEL_BG)), area);
}

/// Clip by grapheme cells, sanitizing terminal controls before adding an ellipsis.
pub(crate) fn fit(text: &str, width: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let text = crate::markdown::clean(text).replace(['\n', '\r'], " ");
    if text.width() <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for glyph in text.graphemes(true) {
        if used + glyph.width() > width - 1 {
            break;
        }
        out.push_str(glyph);
        used += glyph.width();
    }
    out.push('…');
    out
}

fn dialog_area(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height.saturating_sub(1));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(1).saturating_sub(height) / 2,
        width,
        height,
    )
}

fn render_help(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let popup = dialog_area(area, 76, 30);
    frame.render_widget(Clear, popup);
    let block = panel("Shortcuts").border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let sections = [
        (
            "Input editing",
            &[
                "Enter send · Shift+Enter newline",
                "Ctrl+Left/Right / Alt+Left/Right move by word",
                "Ctrl+B/F move by character · Home/End line edges",
                "Ctrl+W / Alt+Backspace delete previous word",
                "Ctrl+U/K delete to line start/end · Ctrl+Y restore deletion",
                "Ctrl+E external editor · Up/Down input history",
            ][..],
        ),
        (
            "Conversation",
            &[
                "PgUp/PgDn / wheel read history · Ctrl+O tool details",
                "Ctrl+G graphics/source · / commands · Tab complete",
                "Ctrl+C interrupt model request; tools may continue",
                "Ctrl+Q detach; the session keeps running",
            ][..],
        ),
        (
            "Sessions and models",
            &[
                "Ctrl+S sessions · /sessions full stored output",
                "Ctrl+P pending inputs · Ctrl+M model and effort",
                "/attach PATH or @PATH + Tab · /detach INDEX",
            ][..],
        ),
        (
            "Approvals",
            &["Ctrl+A focus approvals · Up/Down select", "a approve once · d deny · Esc return to draft"][..],
        ),
        (
            "More",
            &[
                "/help environment, delivery and recovery details",
                "Esc close shortcuts · Up/Down scroll · PgUp/PgDn page",
            ][..],
        ),
    ];
    let mut lines = Vec::new();
    for (title, hints) in sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(crate::markdown::plain(title, inner.width as usize, selection()));
        for hint in hints {
            lines.extend(crate::wrap::wrap_line(&hint_line(hint), inner.width as usize));
        }
    }
    app.help_scroll = app.help_scroll.min(lines.len().saturating_sub(inner.height as usize));
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(app.help_scroll).take(inner.height as usize).collect::<Vec<_>>()),
        inner,
    );
    // Overlays own the caret as well as key, mouse and paste input.
    app.pictures.clear();
}

/// Style the key token of each `key action` pair consistently across views.
pub(crate) fn hint_line(hint: &str) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, part) in hint.split(" · ").enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", secondary()));
        }
        match part.split_once(' ') {
            Some((key, action)) => {
                spans.push(Span::styled(
                    key.to_string(),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD).remove_modifier(Modifier::DIM),
                ));
                spans.push(Span::styled(format!(" {action}"), secondary()));
            }
            None => {
                spans.push(Span::styled(part.to_string(), Style::default().fg(ACCENT).remove_modifier(Modifier::DIM)))
            }
        }
    }
    Line::from(spans)
}
