//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Constraint, Layout, Rect};
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
        .title(Span::styled(format!(" {} ", title.into()), Style::default().fg(GREY)))
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL_BG).fg(FG))
}

/// A selected row keeps a textual marker; color and bold never work alone.
pub(crate) fn selection() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Screen regions shared by rendering and hit-testing.
pub struct V2Geometry {
    pub status: Rect,
    pub chat: Rect,
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
    let composer_h = (app.composer.widget_height(content.width.saturating_sub(2) as usize) as u16)
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
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(approvals_h),
        Constraint::Length(completion_h),
        Constraint::Length(composer_h),
    ])
    .split(content);
    V2Geometry {
        status: rows[0],
        chat: chunks[0],
        approvals: chunks[1],
        completion: chunks[2],
        composer: chunks[3],
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
                if i == menu.model_ix { selection() } else { Style::default().fg(GREY) },
            )
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("Effort  ", Style::default().fg(GREY)),
        Span::styled(fit(&format!("‹ {effort} ›"), inner.width.saturating_sub(8) as usize), selection()),
    ]));
    lines.extend(crate::markdown::plain(
        "Applies at the next turn. Enter applies; Esc cancels.",
        inner.width as usize,
        Style::default().fg(GREY),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_status(frame: &mut Frame, app: &V2App, area: Rect) {
    let status = app.status_line();
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
    let mut line = Line::from(Span::styled(" ", Style::default()));
    if width >= 100 {
        line.spans.push(Span::styled("TeamAgents · ", Style::default().fg(CODEX).add_modifier(Modifier::BOLD)));
    }
    line.spans.push(Span::styled(fit(&status, width.saturating_sub(line.width())), status_style));
    let mut extras = Vec::new();
    if !app.session_name.is_empty() {
        extras.push(fit(&app.session_name, if width >= 100 { 20 } else { 12 }));
    }
    if let Some(leader) = app.active_instance() {
        if !leader.model.is_empty() {
            let profile = if leader.effort.is_empty() {
                leader.model.clone()
            } else {
                format!("{} / {}", leader.model, leader.effort)
            };
            extras.push(profile);
        }
    }
    for extra in extras {
        let part = format!(" · {extra}");
        if line.width() + unicode_width::UnicodeWidthStr::width(part.as_str()) <= width {
            line.spans.push(Span::styled(part, Style::default().fg(GREY)));
        }
    }
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(PANEL_BG)), area);
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
        lines.push(Line::styled(
            "What would you like to build?",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(""));
        lines.extend(crate::markdown::plain(
            "Ask the Leader to explore, fix or create something in your project.",
            width,
            Style::default().fg(GREY),
        ));
        lines.push(Line::from(""));
        lines.extend(crate::markdown::plain(
            "?  Shortcuts     /  Commands     Ctrl+S  Conversations",
            width,
            Style::default().fg(GREY),
        ));
        if let Some(workspace) = app.workspace() {
            lines.push(Line::default());
            lines.extend(crate::markdown::plain(&format!("Workspace  {workspace}"), width, Style::default().fg(GREY)));
        }
    }
    if let Some(goal) = &app.goal {
        if goal.status != "ACTIVE" {
            if let Some(summary) = &goal.summary {
                lines.push(Line::default());
                lines.extend(crate::markdown::plain(&format!("Result: {summary}"), width, Style::default().fg(NOTICE)));
            }
        }
    }
    if let Some(preview) = app.active_preview() {
        // The daemon bounds this transient text. Keep its beginning so Markdown and scrolling are stable.
        if !lines.is_empty() {
            lines.push(Line::default());
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
    let title = if focused { "approvals (focused)" } else { "approvals" };
    let block = panel(title).border_style(Style::default().fg(if focused { WARNING } else { BORDER }));
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
                    format!("{marker} {} · {} · {}", a.id, a.tool, a.preview),
                    selection(),
                ));
            }
            Line::from(vec![
                Span::styled(marker.to_string(), Style::default().fg(ACCENT)),
                Span::styled(format!(" {} ", a.id), Style::default().fg(MUTED)),
                Span::styled("· ", Style::default().fg(MUTED)),
                Span::styled(a.tool.clone(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
                Span::styled(" · ", Style::default().fg(MUTED)),
                Span::styled(a.preview.clone(), Style::default().fg(GREY)),
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
                    if i == app.attachments.selected { selection() } else { Style::default().fg(GREY) },
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
                if index == selected { selection() } else { Style::default().fg(GREY) },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_composer(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height < 3 || area.width < 3 {
        return;
    }
    let focused = app.focus == Focus::Composer && app.profile.is_none() && !app.help_open;
    let title = if app.attachments.paths.is_empty() {
        "Message".to_string()
    } else {
        format!("Message · {} attachments (/attach lists)", app.attachments.paths.len())
    };
    let placeholder =
        if app.disconnected { "Reconnecting · your draft stays here" } else { "Ask, build, or fix something…" };
    render_editor(frame, &app.composer, area, &title, focused, placeholder);
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
    let block = panel(title).border_style(Style::default().fg(if focused { ACCENT } else { BORDER }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (lines, row, col) = composer.visual_lines(inner.width as usize);
    let (start, count) = window(lines.len(), row, inner.height as usize);
    let visible = lines.into_iter().skip(start).take(count).map(Line::raw).collect::<Vec<_>>();
    if composer.text().is_empty() {
        frame.render_widget(Paragraph::new(placeholder).style(Style::default().fg(GREY)), inner);
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
                "Enter send · Ctrl+J / Shift+Enter newline",
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
                "/attach PATH add a file · /detach INDEX remove it",
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
            spans.push(Span::styled(" · ", Style::default().fg(MUTED)));
        }
        match part.split_once(' ') {
            Some((key, action)) => {
                spans.push(Span::styled(key.to_string(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
                spans.push(Span::styled(format!(" {action}"), Style::default().fg(GREY)));
            }
            None => spans.push(Span::styled(part.to_string(), Style::default().fg(ACCENT))),
        }
    }
    Line::from(spans)
}
