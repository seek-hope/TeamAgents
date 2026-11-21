//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::theme::*;
use crate::v2app::{Focus, ProfileMenu, V2App, V2Effect};
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
    pub sidebar: Rect,
    pub nav: Rect,
    pub inspector: Rect,
    /// Live activity row, shown when there is something meaningful to report.
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
    let sidebar_w = if area.width >= 96 && rows[1].height >= 8 { 26 } else { 0 };
    let mut sidebar = Rect::new(rows[1].x, rows[1].y, sidebar_w, rows[1].height);
    let content = Rect::new(rows[1].x + sidebar_w, rows[1].y, rows[1].width.saturating_sub(sidebar_w), rows[1].height);
    let nav_h = u16::from(content.height >= 8);
    let nav = Rect::new(content.x, content.y, content.width, nav_h);
    let content = Rect::new(content.x, content.y + nav_h, content.width, content.height.saturating_sub(nav_h));
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
    let mut chat = chunks[0];
    let mut inspector = Rect::new(chat.right(), chat.y, 0, chat.height);
    if app.team.view == crate::team::View::Conversation && area.width >= 132 && chat.height >= 8 {
        let width = (chat.width * 2 / 5).max(36);
        chat.width -= width;
        inspector = Rect::new(chat.right(), chat.y, width, chat.height);
    }
    if sidebar_w == 0 && app.focus == Focus::Team {
        sidebar = chat;
        chat = Rect::new(sidebar.right(), sidebar.y, 0, sidebar.height);
    }
    V2Geometry {
        status: rows[0],
        chat,
        sidebar,
        nav,
        inspector,
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

fn model_button(composer: Rect) -> Rect {
    if composer.width < 32 || composer.height < 3 {
        return Rect::default();
    }
    Rect::new(composer.right() - 10, composer.y, 7, 1)
}

struct ModelDialog {
    popup: Rect,
    list: Rect,
    save: Rect,
    back: Rect,
}

impl ModelDialog {
    fn new(area: Rect, menu: &ProfileMenu) -> Self {
        let popup = dialog_area(area, 76, (menu.models.len().clamp(1, 8) + 8) as u16);
        let inner = popup.inner(ratatui::layout::Margin::new(1, 1));
        // On short screens keep the selected row visible before showing the settings hints.
        let rows = inner.height.saturating_sub(5).max(1).min(inner.height.saturating_sub(2));
        let list = Rect::new(inner.x, inner.y.saturating_add(1), inner.width, rows);
        let buttons = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, u16::from(inner.height >= 3));
        let save = Rect::new(buttons.x, buttons.y, buttons.width.min(6), buttons.height);
        let back =
            Rect::new(buttons.x.saturating_add(8), buttons.y, buttons.width.saturating_sub(8).min(6), buttons.height);
        Self { popup, list, save, back }
    }
}

/// Pointer routing uses the rendered rectangles; selecting an approval never decides it.
pub fn handle_mouse(app: &mut V2App, area: Rect, mouse: crossterm::event::MouseEvent) -> Option<V2Effect> {
    use crossterm::event::{MouseButton, MouseEventKind};
    if app.help_open {
        match mouse.kind {
            MouseEventKind::ScrollUp => app.help_scroll = app.help_scroll.saturating_sub(3),
            MouseEventKind::ScrollDown => app.help_scroll = app.help_scroll.saturating_add(3),
            _ => {}
        }
        return None;
    }
    if app.queue.is_some() {
        return None;
    }
    let point = ratatui::layout::Position::new(mouse.column, mouse.row);
    let clicked = mouse.kind == MouseEventKind::Down(MouseButton::Left);
    if let Some(menu) = &mut app.profile {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dialog = ModelDialog::new(area, menu);
        let key = if clicked && dialog.save.contains(point) {
            KeyCode::Enter
        } else if clicked && dialog.back.contains(point) {
            KeyCode::Esc
        } else if dialog.list.contains(point) {
            match mouse.kind {
                MouseEventKind::ScrollDown => KeyCode::Down,
                MouseEventKind::ScrollUp => KeyCode::Up,
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(index) = item_at(
                        menu.models.len(),
                        menu.model_ix,
                        dialog.list.height as usize,
                        (point.y - dialog.list.y) as usize,
                    ) {
                        crate::v2app::select_model(menu, index, &app.efforts);
                    }
                    return None;
                }
                _ => return None,
            }
        } else {
            return None;
        };
        return app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
    }
    let geo = geometry(app, area);
    if clicked && model_button(geo.composer).contains(point) {
        return app.open_profile();
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) = approval_at(&geo, app, mouse.row, mouse.column) {
                app.focus = Focus::Approvals;
                app.approval_sel = index;
            } else if geo.composer.contains(point) {
                app.focus = Focus::Composer;
            } else if geo.nav.contains(point) {
                if let Some(view) = crate::team::nav_at(geo.nav, mouse.column) {
                    app.team.show(view);
                    app.focus = if view == crate::team::View::Conversation { Focus::Composer } else { Focus::Panel };
                }
            } else if geo.sidebar.contains(point) {
                let (list, start, count) = crate::team::member_window(app, geo.sidebar);
                if list.contains(point) {
                    let index = (mouse.row - list.y) as usize / 2;
                    if index < count {
                        let id = app.team.rows[start + index]["id"].as_str().unwrap_or("").to_string();
                        app.team.select(&id);
                        if area.width < 132 {
                            app.team.show(crate::team::View::Inspect);
                        }
                    }
                }
                app.focus = if area.width < 96 { Focus::Panel } else { Focus::Team };
            } else if geo.inspector.contains(point)
                || (geo.chat.contains(point) && app.team.view != crate::team::View::Conversation)
            {
                app.focus = Focus::Panel;
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
            } else if geo.sidebar.contains(point) {
                app.team.move_selection(!up);
            } else if geo.inspector.contains(point)
                || (geo.chat.contains(point) && app.team.view != crate::team::View::Conversation)
            {
                app.team.scroll_by(!up, 3);
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
    None
}

pub fn render(frame: &mut Frame, app: &mut V2App) {
    app.pictures.clear();
    app.team.member_visible = false;
    app.team.tasks_visible = false;
    app.graphics.begin_frame();
    app.graphics.deferred = true;
    render_content(frame, app);
    app.graphics.end_frame();
}

fn render_content(frame: &mut Frame, app: &mut V2App) {
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
    let mut geo = geometry(app, area);
    if app.focus == Focus::Panel && app.team.view == crate::team::View::Conversation && geo.inspector.is_empty() {
        app.team.show(crate::team::View::Inspect);
        geo = geometry(app, area);
    }
    // the palette's screen background, drawn first so every widget sits on it
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    render_status(frame, app, geo.status);
    crate::team::render_sidebar(frame, app, geo.sidebar);
    crate::team::render_nav(frame, app, geo.nav);
    if !geo.chat.is_empty() {
        if app.team.view == crate::team::View::Conversation {
            render_chat(frame, app, geo.chat);
        } else {
            crate::team::render_panel(frame, app, geo.chat, app.team.view == crate::team::View::Inspect);
        }
    }
    crate::team::render_panel(frame, app, geo.inspector, true);
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

/// Both roles use the same local catalogue; only confirmation submits a selection.
fn render_profile(frame: &mut Frame, app: &V2App, area: Rect) {
    let Some(menu) = &app.profile else { return };
    let title = match menu.role {
        crate::v2app::ProfileRole::Leader => "Choose a chat model",
        crate::v2app::ProfileRole::Workers => "Choose a model for Workers",
    };
    let dialog = ModelDialog::new(area, menu);
    let popup = dialog.popup;
    frame.render_widget(Clear, popup);
    let block = panel(title).border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let effort = menu.efforts.get(menu.effort_ix).cloned().unwrap_or_else(|| "?".into());
    let (start, count) = window(menu.models.len(), menu.model_ix, dialog.list.height as usize);
    let search = if menu.search.is_empty() { "Type a name or provider to search" } else { &menu.search };
    let mut lines = vec![Line::styled(fit(&format!("Search  {search}"), inner.width as usize), secondary())];
    if menu.models.is_empty() {
        let hint = if menu.all_models.is_empty() {
            "No models available. Run teamagents doctor."
        } else {
            "No matches. Press Ctrl+U to try again."
        };
        lines.extend(crate::markdown::plain(hint, inner.width as usize, secondary()));
    }
    lines.extend(menu.models.iter().enumerate().skip(start).take(count).map(|(i, model)| {
        let label = model.label();
        Line::styled(
            fit(&format!("{} {}", if i == menu.model_ix { ">" } else { " " }, label), inner.width as usize),
            if i == menu.model_ix { selection() } else { secondary() },
        )
    }));
    let (current, option, other_role) = if menu.role == crate::v2app::ProfileRole::Leader {
        (app.model_label(&app.leader_catalog_key), format!("←/→ Thinking effort  {effort}"), "Workers")
    } else {
        let current = match app.worker_policy_mode.as_str() {
            "follow" => "Same as chat".into(),
            "pinned" => app.model_label(&app.worker_catalog_key),
            _ => "".into(),
        };
        let option = match menu.workers_mode {
            crate::v2app::WorkersMode::Follow => "[← Same as chat]   → Use selected model",
            crate::v2app::WorkersMode::Pinned => "← Same as chat   [→ Use selected model]",
            crate::v2app::WorkersMode::Unresolved => "← Same as chat   → Use selected model",
        };
        (current, option.into(), "chat model")
    };
    let current = if current.is_empty() { "Unknown" } else { &current };
    lines.push(Line::styled(fit(&format!("Currently using  {current}"), inner.width as usize), secondary()));
    lines.push(Line::styled(fit(&option, inner.width as usize), secondary()));
    lines.push(Line::styled(format!("Tab for {other_role}"), secondary()));
    frame.render_widget(Paragraph::new(lines), inner);
    frame.render_widget(Paragraph::new("[Save]").style(selection()), dialog.save);
    frame.render_widget(Paragraph::new("[Back]").style(secondary()), dialog.back);
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

/// Session identity and recorded work usage remain visible while browsing members.
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
    let usage = app.work.as_ref().map(|work| {
        let limit = work.limit_total.map(|n| format!("/{}", compact_tokens(n))).unwrap_or_default();
        format!("{}{limit} tokens{}", compact_tokens(work.known_total), if work.unknown { " + ?" } else { "" })
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
    frame.render_widget(
        Paragraph::new(" Leader · conversation").style(Style::default().fg(ACCENT)),
        Rect::new(area.x, area.y, area.width, area.height.min(1)),
    );
    let area = Rect::new(area.x, area.y + u16::from(area.height > 0), area.width, area.height.saturating_sub(1));
    let area = Rect::new(area.x + u16::from(area.width > 2), area.y, area.width.saturating_sub(2), area.height);
    let width = area.width as usize;
    let preview = app.active_preview().map(str::to_owned);
    let workspace = app.workspace().map(str::to_owned);
    let base = app.transcript.document(
        &app.entries,
        width,
        Some(if app.expanded_tools { 40 } else { 4 }),
        Some(&mut app.graphics),
        &app.session_root,
    );
    let mut document = crate::graphics::Document::default();
    let lines = &mut document.lines;
    if base.lines.is_empty() && preview.is_none() {
        lines.push(Line::styled("What would you like to do?", Style::default().fg(CODEX).add_modifier(Modifier::BOLD)));
        lines.extend(crate::markdown::plain("Type a question below and press Enter.", width, Style::default().fg(FG)));
        lines.extend(crate::markdown::plain("Try: What does this project do?", width, secondary()));
        lines.push(Line::default());
        lines.extend(crate::markdown::plain("/model  Choose a model     ?  Shortcuts", width, secondary()));
        if let Some(workspace) = &workspace {
            lines.push(Line::default());
            lines.extend(crate::markdown::plain(&format!("Workspace  {workspace}"), width, secondary()));
        }
    }
    if let Some(work) = &app.work {
        if work.status != "ACTIVE" {
            if let Some(summary) = &work.summary {
                lines.push(Line::default());
                lines.extend(crate::markdown::plain(&format!("Result · {}", work.status), width, secondary()));
                let origin = crate::graphics::Origin {
                    session: app.session_root.clone().into(),
                    workspace: workspace.as_deref().unwrap_or_default().into(),
                    member: app.team.leader_id.clone(),
                    position: usize::MAX - 1,
                };
                document.append(crate::transcript::assistant_document(
                    summary,
                    "",
                    width,
                    Some(&mut app.graphics),
                    &origin,
                    false,
                ));
            }
        }
    }
    if let Some(preview) = preview {
        // The daemon bounds this transient text. Keep its beginning so Markdown and scrolling are stable.
        if !base.lines.is_empty() || !document.lines.is_empty() {
            document.lines.push(Line::default());
        }
        let workspace =
            app.entries.iter().rev().find(|e| !e.workspace.is_empty()).map(|e| e.workspace.clone()).unwrap_or_default();
        let origin = crate::graphics::Origin {
            session: app.session_root.clone().into(),
            workspace: workspace.into(),
            member: app.team.leader_id.clone(),
            position: usize::MAX,
        };
        document.append(crate::transcript::assistant_document(
            &preview,
            "",
            width,
            Some(&mut app.graphics),
            &origin,
            true,
        ));
    }
    let total = base.lines.len() + document.lines.len();
    let height = area.height as usize;
    let previous = if app.last_chat_width == width { app.last_chat_lines } else { total };
    let (start, offset) = crate::viewport::transcript_window(total, previous, height, app.chat_scroll);
    app.chat_scroll = offset;
    app.last_chat_lines = total;
    app.last_chat_width = width;
    app.last_chat_height = height.max(1);
    let max_scroll = total.saturating_sub(height);
    let (visible, pictures) = base.viewport(&document, &mut app.graphics, start, area);
    app.pictures = pictures;
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

/// Leader prompt with an explicit recipient, independent of member navigation.
/// The queue editor keeps using `render_editor` below.
fn render_composer(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.height < 3 || area.width < 3 {
        return;
    }
    let focused = app.focus == Focus::Composer && app.profile.is_none() && !app.help_open;
    let title = if app.team.selected != app.team.leader_id {
        format!("Message Leader · viewing {}", fit(&app.team.selected, 24))
    } else {
        "Message Leader".to_string()
    };
    let block = panel(title).border_style(border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new("[Model]").style(Style::default().fg(ACCENT)), model_button(area));
    let prefix_width = if inner.width >= 4 { 2 } else { 0 };
    let (lines, row, col) = app.composer.visual_lines(inner.width.saturating_sub(prefix_width) as usize);
    let (start, count) = window(lines.len(), row, inner.height as usize);
    if app.composer.text().is_empty() {
        let placeholder = if app.disconnected { "Reconnecting · draft preserved" } else { "Type a question…" };
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
            let label = app.member_model_label(&leader.id, &leader.model);
            let profile = if leader.effort.is_empty() { label } else { format!("{label} · {}", leader.effort) };
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
            "Team workspace",
            &[
                "F6 cycles team, inspection and composer focus",
                "Alt+1 chat · Alt+2 tasks · Alt+3 activity · Alt+4 inspect",
                "Team: Up/Down member · Enter inspect · h history",
                "Inspect: d details · h member history · Esc compose",
                "Browsing is read-only; messages and Ctrl+C target Leader",
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
