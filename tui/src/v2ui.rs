//! R2-P4 R19-b② v2 conversation interface rendering (plan §9). One geometry
//! source — `geometry()` — drives both the renderer and mouse hit-testing,
//! the same discipline as ui::geometry for the v1 interface.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::theme::*;
use crate::v2app::{DragTarget, Focus, ProfileMenu, ProfileRole, V2App, V2Effect};
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
    let sidebar_w = if !app.zoomed && !app.sidebar_hidden && area.width >= 96 && rows[1].height >= 8 {
        app.sidebar_width.clamp(18, (area.width / 3).clamp(18, 44))
    } else {
        0
    };
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
    if !app.zoomed && app.team.view == crate::team::View::Conversation && area.width >= 132 && chat.height >= 8 {
        let max_w = chat.width.saturating_sub(24);
        let min_w = 36u16.min(max_w);
        let width = ((chat.width as u32 * app.inspector_pct as u32) / 100) as u16;
        let width = width.clamp(min_w, max_w);
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
    leader_tab: Rect,
    workers_tab: Rect,
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
        let title_len = match menu.role {
            ProfileRole::Leader => 21u16,
            ProfileRole::Workers => 28u16,
        };
        let workers_w = 9u16;
        let leader_w = 8u16;
        let workers_x = popup.right().saturating_sub(2 + workers_w);
        let leader_x = workers_x.saturating_sub(1 + leader_w);
        let (leader_tab, workers_tab) = if popup.height >= 3 && leader_x >= popup.x.saturating_add(2 + title_len) {
            (Rect::new(leader_x, popup.y, leader_w, 1), Rect::new(workers_x, popup.y, workers_w, 1))
        } else {
            (Rect::default(), Rect::default())
        };
        Self { popup, list, save, back, leader_tab, workers_tab }
    }
}

fn chat_latest_button(chat: Rect, scroll: usize) -> Option<Rect> {
    if scroll == 0 || chat.height == 0 {
        return None;
    }
    let label = format!("↓ {scroll} below · latest");
    let w = unicode_width::UnicodeWidthStr::width(label.as_str()) as u16;
    if chat.width < 25 + w {
        return None;
    }
    Some(Rect::new(chat.right().saturating_sub(w + 1), chat.y, w, 1))
}

fn help_popup(area: Rect) -> Rect {
    dialog_area(area, 76, 30)
}

fn report_popup(area: Rect) -> Rect {
    dialog_area(area, 92, 32)
}

fn chat_body_rect(app: &V2App, area: Rect) -> Rect {
    let header_rows = u16::from(area.height > 0);
    let search_rows = u16::from(app.chat_search.is_some() && area.height >= 3);
    let top = header_rows + search_rows;
    let body = Rect::new(area.x, area.y + top, area.width, area.height.saturating_sub(top));
    Rect::new(body.x + u16::from(body.width > 2), body.y, body.width.saturating_sub(2), body.height)
}

/// Pointer routing uses the rendered rectangles; selecting an approval never decides it.
pub fn handle_mouse(app: &mut V2App, area: Rect, mouse: crossterm::event::MouseEvent) -> Option<V2Effect> {
    use crossterm::event::{MouseButton, MouseEventKind};
    let point = ratatui::layout::Position::new(mouse.column, mouse.row);
    let clicked = mouse.kind == MouseEventKind::Down(MouseButton::Left);
    if let Some(confirm) = app.confirm.clone() {
        match confirm.mouse(area, mouse) {
            crate::overlays::ModalKey::Close => app.confirm = None,
            crate::overlays::ModalKey::Submit(action) => {
                app.confirm = None;
                return app.apply_confirm_action(action);
            }
            _ => {}
        }
        return None;
    }
    if let Some(detail) = app.approval_detail.as_mut() {
        match detail.mouse(area, mouse) {
            crate::overlays::ModalKey::Close => app.approval_detail = None,
            crate::overlays::ModalKey::Submit(decision) => {
                app.approval_detail = None;
                return app.apply_approval_decision(decision);
            }
            _ => {}
        }
        return None;
    }
    if let Some(report) = app.report.as_mut() {
        match mouse.kind {
            MouseEventKind::ScrollUp => report.scroll = report.scroll.saturating_sub(3),
            MouseEventKind::ScrollDown => report.scroll = report.scroll.saturating_add(3),
            MouseEventKind::Down(MouseButton::Left) if !report_popup(area).contains(point) => app.report = None,
            _ => {}
        }
        return None;
    }
    if app.help_open {
        match mouse.kind {
            MouseEventKind::ScrollUp => app.help_scroll = app.help_scroll.saturating_sub(3),
            MouseEventKind::ScrollDown => app.help_scroll = app.help_scroll.saturating_add(3),
            MouseEventKind::Down(MouseButton::Left) if !help_popup(area).contains(point) => app.help_open = false,
            _ => {}
        }
        return None;
    }
    if app.queue.is_some() {
        return None;
    }
    if let Some(menu) = &mut app.profile {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dialog = ModelDialog::new(area, menu);
        let switch_role = clicked
            && ((!dialog.leader_tab.is_empty()
                && dialog.leader_tab.contains(point)
                && menu.role == ProfileRole::Workers)
                || (!dialog.workers_tab.is_empty()
                    && dialog.workers_tab.contains(point)
                    && menu.role == ProfileRole::Leader));
        let key = if clicked && dialog.save.contains(point) {
            KeyCode::Enter
        } else if clicked && dialog.back.contains(point) {
            KeyCode::Esc
        } else if switch_role {
            KeyCode::Tab
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
    if let Some(palette) = app.palette.as_mut() {
        match palette.mouse(area, mouse) {
            crate::overlays::ModalKey::Close => app.palette = None,
            crate::overlays::ModalKey::Submit(action) => {
                app.palette = None;
                return app.apply_palette_action(action);
            }
            _ => {}
        }
        return None;
    }
    if let Some(rewind) = app.rewind_modal.as_mut() {
        match rewind.mouse(area, mouse) {
            crate::overlays::ModalKey::Close => app.rewind_modal = None,
            crate::overlays::ModalKey::Submit(action) => return app.apply_rewind_action(action),
            _ => {}
        }
        return None;
    }
    if let Some(mcp) = app.mcp_modal.as_mut() {
        match mcp.mouse(area, mouse) {
            crate::overlays::ModalKey::Close => app.mcp_modal = None,
            crate::overlays::ModalKey::Submit(action) => return app.apply_mcp_action(action),
            _ => {}
        }
        return None;
    }
    let geo = geometry(app, area);
    match mouse.kind {
        MouseEventKind::Drag(MouseButton::Left) => {
            match app.drag {
                Some(DragTarget::Sidebar) => {
                    app.sidebar_width = mouse.column.saturating_sub(area.x).saturating_add(1).clamp(18, 44);
                }
                Some(DragTarget::Inspector) => {
                    let total = geo.chat.width + geo.inspector.width;
                    if total > 0 {
                        let desired = geo.inspector.right().saturating_sub(mouse.column);
                        app.inspector_pct = ((desired as u32 * 100) / total as u32).clamp(25, 65) as u16;
                    }
                }
                None => {}
            }
            return None;
        }
        MouseEventKind::Up(MouseButton::Left) if app.drag.take().is_some() => return None,
        _ => {}
    }
    if clicked && model_button(geo.composer).contains(point) {
        return app.open_profile();
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !geo.sidebar.is_empty()
                && mouse.column == geo.sidebar.right().saturating_sub(1)
                && mouse.row >= geo.sidebar.y
                && mouse.row < geo.sidebar.bottom()
            {
                app.drag = Some(DragTarget::Sidebar);
            } else if !geo.inspector.is_empty()
                && mouse.column == geo.inspector.x
                && mouse.row >= geo.inspector.y
                && mouse.row < geo.inspector.bottom()
            {
                app.drag = Some(DragTarget::Inspector);
            } else if geo.status.contains(point) && !app.approvals.is_empty() {
                app.focus = Focus::Approvals;
            } else if let Some(index) = approval_at(&geo, app, mouse.row, mouse.column) {
                if app.focus == Focus::Approvals && app.approval_sel == index {
                    if let Some(approval) = app.approvals.get(index) {
                        app.approval_detail =
                            Some(crate::overlays::ApprovalDetail { approval_id: approval.id.clone(), scroll: 0 });
                    }
                } else {
                    app.focus = Focus::Approvals;
                    app.approval_sel = index;
                }
            } else if geo.completion.contains(point) {
                let row = (mouse.row - geo.completion.y) as usize;
                if app.attachments.visible(&app.composer) {
                    if let Some(index) = item_at(
                        app.attachments.candidates.len(),
                        app.attachments.selected,
                        geo.completion.height as usize,
                        row,
                    ) {
                        app.attachments.apply_at(&mut app.composer, index);
                        if let Some(note) = app.attachments.take_notice() {
                            app.command_note(note);
                        }
                    }
                } else {
                    let candidates = app.completion.candidates(&app.composer);
                    let selected = app.completion.selected(&app.composer).min(candidates.len().saturating_sub(1));
                    if let Some(index) = item_at(candidates.len(), selected, geo.completion.height as usize, row) {
                        app.completion.apply_at(&mut app.composer, index);
                    }
                }
                app.focus = Focus::Composer;
            } else if geo.composer.contains(point) {
                app.focus = Focus::Composer;
            } else if geo.nav.contains(point) {
                if let Some(view) = crate::team::nav_at(geo.nav, mouse.column) {
                    app.team.show(view);
                    app.focus = if view == crate::team::View::Conversation { Focus::Composer } else { Focus::Panel };
                    if view == crate::team::View::Tree {
                        app.team.tree_dirty = true;
                        if !app.disconnected && app.team.tree.is_none() {
                            return Some(V2Effect::SearchTree);
                        }
                    }
                } else if let Some((_, action, _, _)) = crate::team::nav_actions(app, geo.nav, area.width)
                    .into_iter()
                    .find(|(rect, _, _, _)| rect.contains(point))
                {
                    match action {
                        crate::team::NavAction::Alert => app.jump_next_attention(),
                        crate::team::NavAction::Zoom => app.toggle_zoom(),
                        crate::team::NavAction::Sidebar => app.toggle_sidebar(),
                    }
                }
            } else if geo.sidebar.contains(point) {
                if let Some(sess_rect) = crate::team::live_sessions_window(app, geo.sidebar) {
                    if sess_rect.contains(point) && mouse.row > sess_rect.y {
                        let idx = (mouse.row - sess_rect.y - 1) as usize;
                        if let Some(s) = app.live_sessions.get(idx) {
                            return Some(V2Effect::Sessions(crate::sessions::SessionAction::Open(
                                crate::sessions::SessionRow {
                                    id: s.id.clone(),
                                    name: s.name.clone(),
                                    path: s.root.clone(),
                                    archived: false,
                                    live: s.live,
                                },
                            )));
                        }
                    }
                }
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
                app.focus =
                    if area.width < 96 || app.sidebar_hidden || app.zoomed { Focus::Panel } else { Focus::Team };
            } else if geo.inspector.contains(point)
                || (geo.chat.contains(point) && app.team.view != crate::team::View::Conversation)
            {
                let right_inspector = geo.inspector.contains(point);
                let panel_area = if right_inspector { geo.inspector } else { geo.chat };
                let is_inspect = right_inspector || app.team.view == crate::team::View::Inspect;
                if is_inspect {
                    if let Some((_, verb, _)) = crate::team::inspector_actions(app, panel_area, right_inspector)
                        .into_iter()
                        .find(|(rect, _, _)| rect.contains(point))
                    {
                        let target = app.team.inspected_id(right_inspector).to_string();
                        match verb {
                            crate::team::InspectorVerb::Message => {
                                app.direct_target = (target != app.team.leader_id).then_some(target);
                                app.focus = Focus::Composer;
                                return None;
                            }
                            crate::team::InspectorVerb::Pause => {
                                app.confirm = Some(crate::overlays::ConfirmAction::PauseMember(target));
                                return None;
                            }
                            crate::team::InspectorVerb::Resume => {
                                app.confirm = Some(crate::overlays::ConfirmAction::ResumeMember(target));
                                return None;
                            }
                            crate::team::InspectorVerb::Terminate => {
                                app.confirm = Some(crate::overlays::ConfirmAction::TerminateMember(target));
                                return None;
                            }
                            crate::team::InspectorVerb::Pin => {
                                if app.team.pinned.as_deref() == Some(target.as_str()) {
                                    app.team.pinned = None;
                                } else {
                                    app.team.pinned = Some(target.clone());
                                }
                                app.team.ensure_inspected_history(&target);
                            }
                        }
                    } else if app.team.inspected_id(right_inspector) != app.team.leader_id {
                        if let Some((details_tab, history_tab)) = crate::team::inspector_tabs(panel_area) {
                            if details_tab.contains(point) {
                                app.team.history_open = false;
                                app.team.scroll = 0;
                            } else if history_tab.contains(point) {
                                let target = app.team.inspected_id(right_inspector).to_string();
                                app.team.ensure_inspected_history(&target);
                                app.team.history_open = true;
                            }
                        }
                    }
                } else if app.team.view == crate::team::View::Activity {
                    if let Some(filter_rect) = crate::team::activity_filter_rect(panel_area) {
                        if filter_rect.contains(point) {
                            app.team.activity_filter_selected = !app.team.activity_filter_selected;
                            app.team.scroll = 0;
                        }
                    }
                } else if app.team.view == crate::team::View::Wall {
                    if let Some((_, id)) =
                        crate::team::wall_cards(app, panel_area).into_iter().find(|(rect, _)| rect.contains(point))
                    {
                        app.team.select(&id);
                    }
                } else if app.team.view == crate::team::View::Tree {
                    if let Some(list_area) = crate::team::tree_list_rect(panel_area) {
                        if list_area.contains(point) {
                            let nodes = app.team.visible_tree_nodes();
                            if let Some(index) = item_at(
                                nodes.len(),
                                app.team.tree_selected,
                                list_area.height as usize,
                                (point.y - list_area.y) as usize,
                            ) {
                                let was_selected = app.team.tree_selected == index;
                                let fold_id = if was_selected && nodes[index].2 {
                                    nodes[index].1["id"].as_str().map(str::to_string)
                                } else {
                                    None
                                };
                                app.team.tree_selected = index;
                                if let Some(id) = fold_id {
                                    if !app.team.tree_collapsed.remove(&id) {
                                        app.team.tree_collapsed.insert(id);
                                    }
                                }
                            }
                        }
                    }
                }
                app.focus = Focus::Panel;
            } else if geo.chat.contains(point) {
                if chat_latest_button(geo.chat, app.chat_scroll).is_some_and(|button| button.contains(point)) {
                    app.chat_scroll = 0;
                } else {
                    let body = chat_body_rect(app, geo.chat);
                    if body.contains(point) {
                        let start =
                            app.last_chat_lines.saturating_sub(app.last_chat_height).saturating_sub(app.chat_scroll);
                        let doc_line = start + (mouse.row - body.y) as usize;
                        if let Some((_, code)) =
                            app.transcript.code_blocks.iter().find(|(range, _)| range.contains(&doc_line))
                        {
                            return Some(V2Effect::CopyClipboard(code.clone()));
                        }
                        if let Some((entry_idx, _)) =
                            app.transcript.entry_ranges.iter().find(|(_, range)| range.contains(&doc_line))
                        {
                            if app.entries.get(*entry_idx).is_some_and(|e| e.kind == crate::v2app::ChatKind::Tool) {
                                let idx = *entry_idx;
                                if !app.expanded_entries.remove(&idx) {
                                    app.expanded_entries.insert(idx);
                                }
                            }
                        }
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
    app.team.tree_visible = false;
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
    crate::team::render_nav(frame, app, geo.nav, area.width);
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
    if app.help_open {
        render_help(frame, app, area);
    }
    render_report(frame, app, area);
    if let Some(mcp) = &app.mcp_modal {
        mcp.render(frame, area);
    }
    if let Some(rewind) = &app.rewind_modal {
        rewind.render(frame, area);
    }
    if let Some(palette) = &app.palette {
        palette.render(frame, area);
    }
    if let Some(detail) = &app.approval_detail {
        let approval = app.approvals.iter().find(|a| a.id == detail.approval_id);
        detail.render(frame, approval, area);
    }
    if let Some(confirm) = &app.confirm {
        confirm.render(frame, area);
    }
    if app.profile.is_some()
        || app.help_open
        || app.report.is_some()
        || app.mcp_modal.is_some()
        || app.rewind_modal.is_some()
        || app.palette.is_some()
        || app.approval_detail.is_some()
        || app.confirm.is_some()
    {
        app.pictures.clear();
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
    if !dialog.leader_tab.is_empty() && !dialog.workers_tab.is_empty() {
        let (l_text, l_style, w_text, w_style) = match menu.role {
            ProfileRole::Leader => ("[Leader]", selection(), " Workers ", secondary()),
            ProfileRole::Workers => (" Leader ", secondary(), "[Workers]", selection()),
        };
        frame.render_widget(Paragraph::new(l_text).style(l_style), dialog.leader_tab);
        frame.render_widget(Paragraph::new(w_text).style(w_style), dialog.workers_tab);
    }
    let effort = menu.efforts.get(menu.effort_ix).cloned().unwrap_or_else(|| "?".into());
    let (start, count) = window(menu.models.len(), menu.model_ix, dialog.list.height as usize);
    let search = if menu.search.is_empty() {
        "Type a name or provider to search".to_string()
    } else {
        format!("{} ({}/{})", menu.search, menu.models.len(), menu.all_models.len())
    };
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
    if menu.models.len() > dialog.list.height as usize && dialog.list.height >= 2 && dialog.list.width > 2 {
        let max_scroll = menu.models.len().saturating_sub(dialog.list.height as usize);
        let mut state = ScrollbarState::new(max_scroll + 1).position(start.min(max_scroll));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            dialog.list,
            &mut state,
        );
    }
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

fn budget_bar(used: i64, ceiling: i64) -> String {
    let ceiling = ceiling.max(1);
    let pct = ((used.max(0) as f64 / ceiling as f64) * 100.0).round().clamp(0.0, 999.0) as u64;
    let filled = (((used.max(0) as f64 / ceiling as f64) * 4.0).round() as usize).clamp(0, 4);
    format!("{}{} {}%", "▰".repeat(filled), "▱".repeat(4 - filled), pct)
}

/// Session identity and recorded work usage remain visible while browsing members.
fn render_status(frame: &mut Frame, app: &V2App, area: Rect) {
    let base_status = app.runtime_status_line();
    let status = if let Some(leader) = app.active_instance() {
        if leader.lifecycle == "ACTIVE"
            && matches!(leader.phase.as_str(), "MODEL_PENDING" | "TOOLS_PENDING" | "COMPLETION_PENDING")
        {
            if let Some(secs) = app.team.phase_elapsed(&leader.id).filter(|&s| s >= 1) {
                format!("{base_status} {secs}s")
            } else {
                base_status
            }
        } else {
            base_status
        }
    } else {
        base_status
    };
    let status_style = if app.disconnected {
        Style::default().fg(ERROR)
    } else if !app.approvals.is_empty() {
        Style::default().fg(WARNING).add_modifier(Modifier::BOLD)
    } else if status.starts_with("Ready") {
        Style::default().fg(SUCCESS)
    } else {
        Style::default().fg(NOTICE)
    };
    let width = area.width as usize;
    let usage = app.work.as_ref().map(|work| {
        let limit = work.limit_total.map(|n| format!("/{}", compact_tokens(n))).unwrap_or_default();
        let mut text =
            format!("{}{limit} tokens{}", compact_tokens(work.known_total), if work.unknown { " + ?" } else { "" });
        if let Some(usd) = work.cost_usd {
            let approx = if work.cost_complete { "" } else { "~" };
            let cost_str =
                if usd > 0.0 && usd < 0.01 { format!("${usd:.4}{approx}") } else { format!("${usd:.2}{approx}") };
            text.push_str(&format!(" · {cost_str}"));
        }
        if width >= 88 {
            if let Some(ceiling) = work.limit_total.or_else(|| app.active_context_window()).filter(|&n| n > 0) {
                text.push_str(&format!(" · {}", budget_bar(work.known_total, ceiling)));
            }
        }
        text
    });
    let right_cap = if width >= 88 { width / 2 } else { width / 3 };
    let right = fit(usage.as_deref().unwrap_or(""), right_cap);
    let right_cols = unicode_width::UnicodeWidthStr::width(right.as_str());
    let right_width = if right.is_empty() { 0 } else { right_cols + 2 };
    let left_width = width.saturating_sub(right_width);
    let mut left = Line::from(Span::styled(" ", Style::default()));
    left.spans.push(Span::styled(
        if left_width >= 20 { "◆ TeamAgents" } else { "TA" },
        Style::default().fg(CODEX).add_modifier(Modifier::BOLD),
    ));
    if app.zoomed && left_width >= 28 {
        left.spans.push(Span::styled(" [ZOOM]", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
    }
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
        let cols = right_cols as u16;
        let right_area = Rect::new(area.right().saturating_sub(cols + 1), area.y, cols, 1);
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
    let failed = app.shown_notice().is_some_and(|notice| notice.error);
    let left_width = (area.width as usize).saturating_sub(right_width + 3);
    let line = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(fit(&status, left_width), Style::default().fg(if stopped || failed { ERROR } else { NOTICE })),
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
    let header_style = if app.focus == Focus::Composer {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        secondary()
    };
    frame.render_widget(
        Paragraph::new(" Leader · conversation").style(header_style),
        Rect::new(area.x, area.y, area.width, area.height.min(1)),
    );
    if let Some(button) = chat_latest_button(area, app.chat_scroll) {
        frame.render_widget(
            Paragraph::new(format!("↓ {} below · latest", app.chat_scroll))
                .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            button,
        );
    }
    if let Some(search) = &app.chat_search {
        if area.height >= 3 && area.width > 2 {
            let count = if search.matches.is_empty() {
                if search.query.trim().is_empty() {
                    String::new()
                } else {
                    "0 matches".into()
                }
            } else {
                format!("{}/{}", search.current + 1, search.matches.len())
            };
            let state = if search.editing { "[editing]" } else { "n/N jump" };
            let bar = format!(" Find: {}  {count}  ({state} · Esc clear)", search.query);
            frame.render_widget(
                Paragraph::new(fit(&bar, area.width as usize))
                    .style(Style::default().bg(PANEL_BG).fg(ACCENT).add_modifier(Modifier::BOLD)),
                Rect::new(area.x, area.y + 1, area.width, 1),
            );
        }
    }
    let body = chat_body_rect(app, area);
    let width = body.width as usize;
    let preview = app.active_preview().map(str::to_owned);
    let workspace = app.workspace().map(str::to_owned);
    let base = app.transcript.document_with_expanded(
        &app.entries,
        width,
        Some(if app.expanded_tools { 40 } else { 4 }),
        &app.expanded_entries,
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
    let height = body.height as usize;
    let previous = if app.last_chat_width == width { app.last_chat_lines } else { total };
    let (start, offset) = crate::viewport::transcript_window(total, previous, height, app.chat_scroll);
    app.chat_scroll = offset;
    app.last_chat_lines = total;
    app.last_chat_width = width;
    app.last_chat_height = height.max(1);
    let max_scroll = total.saturating_sub(height);
    let (mut visible, pictures) = base.viewport(&document, &mut app.graphics, start, body);
    if let Some(search) = &app.chat_search {
        let q = search.query.trim().to_lowercase();
        if !q.is_empty() {
            let current_range = search
                .matches
                .get(search.current)
                .and_then(|entry_ix| app.transcript.entry_ranges.iter().find(|(ix, _)| ix == entry_ix))
                .map(|(_, r)| r.clone());
            for (row_offset, line) in visible.iter_mut().enumerate() {
                let doc_line = start + row_offset;
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                if text.to_lowercase().contains(&q) {
                    let is_current = current_range.as_ref().is_some_and(|r| r.contains(&doc_line));
                    let hl = if is_current {
                        selection()
                    } else {
                        Style::default().fg(WARNING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    };
                    for span in &mut line.spans {
                        span.style = span.style.patch(hl);
                    }
                }
            }
        }
    }
    app.pictures = pictures;
    frame.render_widget(Paragraph::new(visible), body);
    if total > height && body.width > 0 {
        let mut state = ScrollbarState::new(max_scroll + 1).position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            Rect::new(body.right(), body.y, 1, body.height),
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
    let overlay_open = app.profile.is_some()
        || app.help_open
        || app.palette.is_some()
        || app.rewind_modal.is_some()
        || app.mcp_modal.is_some()
        || app.approval_detail.is_some()
        || app.confirm.is_some()
        || app.chat_search.as_ref().is_some_and(|s| s.editing);
    let focused = app.focus == Focus::Composer && !overlay_open;
    let (title, composer_border) = if let Some(target) = &app.direct_target {
        (
            format!("Direct to {} · Esc back to Leader", fit(target, 24)),
            if focused { Style::default().fg(CODEX).add_modifier(Modifier::BOLD) } else { border(false) },
        )
    } else if app.team.selected != app.team.leader_id {
        (format!("Message Leader · viewing {}", fit(&app.team.selected, 24)), border(focused))
    } else {
        ("Message Leader".to_string(), border(focused))
    };
    let block = panel(title).border_style(composer_border);
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
    let popup = help_popup(area);
    frame.render_widget(Clear, popup);
    let sections = crate::v2app::HELP_SECTIONS;
    let inner_approx = popup.inner(ratatui::layout::Margin::new(1, 1));
    let mut lines = Vec::new();
    for &(title, hints) in sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(crate::markdown::plain(title, inner_approx.width as usize, selection()));
        for hint in hints {
            lines.extend(crate::wrap::wrap_line(&hint_line(hint), inner_approx.width as usize));
        }
    }
    let total = lines.len();
    let height = inner_approx.height as usize;
    let max_scroll = total.saturating_sub(height);
    app.help_scroll = app.help_scroll.min(max_scroll);
    let footer_label = if popup.width >= 26 && total > height && height > 0 {
        let start = app.help_scroll + 1;
        let end = (app.help_scroll + height).min(total);
        format!(" {start}-{end}/{total} · Esc close ")
    } else {
        " Esc close ".to_string()
    };
    let block = panel("Shortcuts")
        .title_bottom(Line::from(footer_label).alignment(Alignment::Right))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(app.help_scroll).take(inner.height as usize).collect::<Vec<_>>()),
        inner,
    );
    if total > height && inner.width > 1 {
        let mut state = ScrollbarState::new(max_scroll + 1).position(app.help_scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            inner,
            &mut state,
        );
    }
    // Overlays own the caret as well as key, mouse and paste input.
    app.pictures.clear();
}

/// A command's readable output in its own dismissible overlay, so reading it never costs the transcript.
fn render_report(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let Some(mut report) = app.report.take() else { return };
    let popup = report_popup(area);
    frame.render_widget(Clear, popup);
    let inner_approx = popup.inner(ratatui::layout::Margin::new(1, 1));
    let lines = crate::markdown::plain(&report.text, inner_approx.width as usize, Style::default().fg(FG));
    let total = lines.len();
    let height = inner_approx.height as usize;
    let max_scroll = total.saturating_sub(height);
    report.scroll = report.scroll.min(max_scroll);
    let footer_label = if popup.width >= 26 && total > height && height > 0 {
        let start = report.scroll + 1;
        let end = (report.scroll + height).min(total);
        format!(" {start}-{end}/{total} · Esc close ")
    } else {
        " Esc close ".to_string()
    };
    let block = panel(report.title.clone())
        .title_bottom(Line::from(footer_label).alignment(Alignment::Right))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let shown: Vec<_> = lines.into_iter().skip(report.scroll).take(inner.height as usize).collect();
    frame.render_widget(Paragraph::new(shown), inner);
    if total > height && inner.width > 1 {
        let mut state = ScrollbarState::new(max_scroll + 1).position(report.scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            inner,
            &mut state,
        );
    }
    app.report = Some(report);
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
