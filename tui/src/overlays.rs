//! Interactive overlays (D-532): command/goto palette, rewind modal, MCP control modal,
//! approval detail drawer, explicit action confirmations, and OSC 52 clipboard encoding.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use serde_json::Value as Json;

use crate::{
    sessions::SessionRow,
    team,
    theme::*,
    v2app::{ApprovalInfo, V2App},
    v2ui::{fit, hint_line, panel, selection},
    viewport::{item_at, window},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalKey<T> {
    Ignored,
    Consumed,
    Close,
    Submit(T),
}

/// Format an OSC 52 clipboard write sequence for terminal-native copying without extra crates.
pub fn osc52_sequence(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = *chunk.get(1).unwrap_or(&0) as usize;
        let b2 = *chunk.get(2).unwrap_or(&0) as usize;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(TABLE[(triple >> 18) & 0x3f] as char);
        encoded.push(TABLE[(triple >> 12) & 0x3f] as char);
        encoded.push(if chunk.len() > 1 { TABLE[(triple >> 6) & 0x3f] as char } else { '=' });
        encoded.push(if chunk.len() > 2 { TABLE[triple & 0x3f] as char } else { '=' });
    }
    format!("\x1b]52;c;{encoded}\x07")
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height.saturating_sub(1));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(1).saturating_sub(height) / 2,
        width,
        height,
    )
}

fn render_scrollbar(frame: &mut Frame, area: Rect, total: usize, pos: usize) {
    let height = area.height as usize;
    if total <= height || area.width < 2 || height == 0 {
        return;
    }
    let max_scroll = total.saturating_sub(height);
    let mut state = ScrollbarState::new(max_scroll + 1).position(pos.min(max_scroll));
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(ACCENT))
            .thumb_symbol("▏")
            .track_symbol(None),
        area,
        &mut state,
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    SelectMember(String),
    OpenView(team::View),
    SelectTask(usize),
    FocusTreeNode(String),
    OpenSession(SessionRow),
    OpenRewind,
    OpenMcp,
    OpenModel,
    OpenQueue,
    OpenSessions,
    OpenHelp,
    ToggleZoom,
    ToggleSidebar,
    CopyLastCode,
    Compact,
    Interrupt,
}

struct PaletteItem {
    category: &'static str,
    label: String,
    detail: String,
    action: PaletteAction,
}

pub struct Palette {
    pub query: String,
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
    pub selected: usize,
}

impl Palette {
    pub fn open(app: &V2App) -> Self {
        let mut items = Vec::new();
        for row in &app.team.rows {
            let id = row["id"].as_str().unwrap_or("?").to_string();
            let label = if id == app.team.leader_id { format!("Leader ({id})") } else { id.clone() };
            let detail =
                format!("{} · {}", team::state(row), app.member_model_label(&id, row["model"].as_str().unwrap_or("")));
            items.push(PaletteItem { category: "Member", label, detail, action: PaletteAction::SelectMember(id) });
        }
        for (view, label) in team::VIEWS {
            items.push(PaletteItem {
                category: "View",
                label: label.to_string(),
                detail: "Switch workspace view".into(),
                action: PaletteAction::OpenView(view),
            });
        }
        let tasks = app.team.all_tasks.as_ref().unwrap_or(&app.team.tasks);
        for (idx, task) in tasks.iter().enumerate() {
            let id = task["id"].as_str().unwrap_or("?");
            let status = task["status"].as_str().unwrap_or("?");
            let assignee = task["assignee"].as_str().unwrap_or("?");
            items.push(PaletteItem {
                category: "Task",
                label: id.to_string(),
                detail: format!("{status} -> {assignee}"),
                action: PaletteAction::SelectTask(idx),
            });
        }
        if let Some(tree) = &app.team.tree {
            for node in tree["nodes"].as_array().into_iter().flatten() {
                let id = node["id"].as_str().unwrap_or("?");
                let title = node["title"].as_str().unwrap_or("Untitled");
                let status = node["status"].as_str().unwrap_or("open");
                items.push(PaletteItem {
                    category: "Tree",
                    label: title.to_string(),
                    detail: format!("{id} · {status}"),
                    action: PaletteAction::FocusTreeNode(format!("Focus on tree node {id} ({title}): ")),
                });
            }
        }
        for s in &app.live_sessions {
            let status = if s.live {
                if s.attention > 0 || s.approvals > 0 {
                    format!("running · !{} · {}", s.attention + s.approvals, s.root)
                } else {
                    format!("running · {}", s.root)
                }
            } else {
                format!("idle · {}", s.root)
            };
            items.push(PaletteItem {
                category: "Session",
                label: s.name.clone(),
                detail: status,
                action: PaletteAction::OpenSession(SessionRow {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    path: s.root.clone(),
                    archived: false,
                    live: s.live,
                }),
            });
        }
        for (label, detail, action) in [
            ("Rewind conversation or files", "/rewind · restore an earlier prompt", PaletteAction::OpenRewind),
            ("MCP services", "/mcp · start, stop, hide, expose, reconnect", PaletteAction::OpenMcp),
            ("Choose model & reasoning effort", "Ctrl+M · Leader / Workers", PaletteAction::OpenModel),
            ("Pending input queue", "Ctrl+P · edit or cancel held inputs", PaletteAction::OpenQueue),
            ("Browse all sessions", "Ctrl+S · search & history", PaletteAction::OpenSessions),
            ("Copy last code block", "Write most recent code block via OSC 52", PaletteAction::CopyLastCode),
            ("Toggle zoom pane", "Alt+Z · expand active workspace pane", PaletteAction::ToggleZoom),
            ("Toggle team sidebar", "Alt+B · show or hide team list", PaletteAction::ToggleSidebar),
            ("Request Leader compaction", "/compact · summarize older context", PaletteAction::Compact),
            ("Interrupt Leader request", "Ctrl+C · cancel in-flight model call", PaletteAction::Interrupt),
            ("Shortcuts & environment", "? / /help · keys and diagnostics", PaletteAction::OpenHelp),
        ] {
            items.push(PaletteItem { category: "Action", label: label.into(), detail: detail.into(), action });
        }
        let mut palette = Self { query: String::new(), items, filtered: Vec::new(), selected: 0 };
        palette.refilter();
        palette
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if words.is_empty() {
                    return true;
                }
                let hay = format!("{} {} {}", item.category, item.label, item.detail).to_lowercase();
                words.iter().all(|word| hay.contains(word))
            })
            .map(|(i, _)| i)
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    pub fn paste(&mut self, text: &str) {
        for c in text.chars() {
            if !c.is_control() && self.query.chars().count() < 128 {
                self.query.push(c);
            }
        }
        self.refilter();
    }

    pub fn key(&mut self, key: KeyEvent) -> ModalKey<PaletteAction> {
        if key.kind == KeyEventKind::Release {
            return ModalKey::Ignored;
        }
        if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
            return ModalKey::Ignored;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('u') {
            self.query.clear();
            self.refilter();
            return ModalKey::Consumed;
        }
        if !(key.modifiers.is_empty() || (key.modifiers == KeyModifiers::SHIFT && matches!(key.code, KeyCode::Char(_))))
        {
            return ModalKey::Consumed;
        }
        match key.code {
            KeyCode::Esc => ModalKey::Close,
            KeyCode::Up => {
                if !self.filtered.is_empty() {
                    self.selected = (self.selected + self.filtered.len() - 1) % self.filtered.len();
                }
                ModalKey::Consumed
            }
            KeyCode::Down => {
                if !self.filtered.is_empty() {
                    self.selected = (self.selected + 1) % self.filtered.len();
                }
                ModalKey::Consumed
            }
            KeyCode::Enter => {
                if let Some(&idx) = self.filtered.get(self.selected) {
                    ModalKey::Submit(self.items[idx].action.clone())
                } else {
                    ModalKey::Consumed
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                ModalKey::Consumed
            }
            KeyCode::Char(c) if !c.is_control() => {
                if self.query.chars().count() < 128 {
                    self.query.push(c);
                    self.refilter();
                }
                ModalKey::Consumed
            }
            _ => ModalKey::Consumed,
        }
    }

    fn layout(area: Rect) -> (Rect, Rect, Rect) {
        let popup = centered(area, 82, 24);
        let inner = popup.inner(Margin::new(1, 1));
        let list = Rect::new(inner.x, inner.y.saturating_add(2), inner.width, inner.height.saturating_sub(3));
        (popup, inner, list)
    }

    pub fn mouse(&mut self, area: Rect, mouse: MouseEvent) -> ModalKey<PaletteAction> {
        let (popup, _, list) = Self::layout(area);
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if !popup.contains(point) => ModalKey::Close,
            MouseEventKind::Down(MouseButton::Left) if list.contains(point) => {
                if let Some(idx) =
                    item_at(self.filtered.len(), self.selected, list.height as usize, (point.y - list.y) as usize)
                {
                    self.selected = idx;
                    if let Some(&item_idx) = self.filtered.get(idx) {
                        return ModalKey::Submit(self.items[item_idx].action.clone());
                    }
                }
                ModalKey::Consumed
            }
            MouseEventKind::ScrollUp if !self.filtered.is_empty() => {
                self.selected = self.selected.saturating_sub(1);
                ModalKey::Consumed
            }
            MouseEventKind::ScrollDown if !self.filtered.is_empty() => {
                self.selected = (self.selected + 1).min(self.filtered.len().saturating_sub(1));
                ModalKey::Consumed
            }
            _ => ModalKey::Consumed,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let (popup, inner, list) = Self::layout(area);
        frame.render_widget(Clear, popup);
        let block = panel("Go to / Command Palette")
            .title_bottom(Line::from(" Enter run · Esc close ").alignment(Alignment::Right))
            .border_style(Style::default().fg(ACCENT));
        frame.render_widget(block, popup);
        if inner.is_empty() {
            return;
        }
        let search_line = if self.query.is_empty() {
            Line::from(vec![
                Span::styled("❯ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("Type to filter {} items (members, tasks, sessions, actions)…", self.items.len()),
                    secondary(),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("❯ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Span::styled(self.query.clone(), Style::default().fg(FG)),
                Span::styled(format!("  ({}/{})", self.filtered.len(), self.items.len()), secondary()),
            ])
        };
        frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y, inner.width, 1));
        if inner.height >= 2 {
            frame.render_widget(
                Paragraph::new("─".repeat(inner.width as usize)).style(secondary()),
                Rect::new(inner.x, inner.y + 1, inner.width, 1),
            );
        }
        let (start, count) = window(self.filtered.len(), self.selected, list.height as usize);
        let w = list.width.saturating_sub(1) as usize;
        let rows: Vec<Line> = if self.filtered.is_empty() {
            vec![Line::styled("  No matching commands or targets. Ctrl+U clears search.", secondary())]
        } else {
            self.filtered[start..start + count]
                .iter()
                .enumerate()
                .map(|(offset, &item_idx)| {
                    let item = &self.items[item_idx];
                    let selected = start + offset == self.selected;
                    let marker = if selected { ">" } else { " " };
                    let text = format!("{marker} [{:7}] {}  ·  {}", item.category, item.label, item.detail);
                    if selected {
                        Line::styled(fit(&text, w), selection())
                    } else {
                        Line::from(vec![
                            Span::styled(format!("{marker} "), secondary()),
                            Span::styled(format!("[{:7}] ", item.category), Style::default().fg(ACCENT)),
                            Span::styled(fit(&item.label, w.saturating_sub(14) / 2 + 8), Style::default().fg(FG)),
                            Span::styled(
                                fit(&format!("  {}", item.detail), w.saturating_sub(item.label.len() + 14)),
                                secondary(),
                            ),
                        ])
                    }
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(rows), list);
        render_scrollbar(frame, list, self.filtered.len(), start);
        if inner.height >= 3 {
            let footer = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
            frame
                .render_widget(Paragraph::new(hint_line("↑/↓ select · Enter open · Ctrl+U clear · Esc close")), footer);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RewindAction {
    LoadMore(Option<i64>),
    Preview { key: String, files: bool },
    Apply(Json),
}

pub struct RewindModal {
    pub points: Vec<Json>,
    pub selected: usize,
    pub next_before: Option<i64>,
    pub files_mode: bool,
    pub preview: Option<Json>,
    pub confirming: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub scroll: usize,
}

impl RewindModal {
    pub fn loading() -> Self {
        Self {
            points: Vec::new(),
            selected: 0,
            next_before: None,
            files_mode: false,
            preview: None,
            confirming: false,
            loading: true,
            error: None,
            scroll: 0,
        }
    }

    pub fn selected_key(&self) -> Option<String> {
        self.points.get(self.selected).and_then(|p| p["envelope"].as_str()).map(str::to_string)
    }

    pub fn apply_points(&mut self, reply: &Json, append: bool) -> Option<RewindAction> {
        let rows = reply["points"].as_array().cloned().unwrap_or_default();
        if append {
            self.points.extend(rows);
        } else {
            self.points = rows;
            self.selected = 0;
        }
        self.next_before = reply["next_before"].as_i64();
        self.loading = false;
        self.error = None;
        if !append && self.preview.is_none() {
            return self.selected_key().map(|key| RewindAction::Preview { key, files: self.files_mode });
        }
        None
    }

    pub fn apply_preview(&mut self, reply: Json) {
        self.preview = Some(reply);
        self.loading = false;
        self.error = None;
        self.scroll = 0;
    }

    pub fn fail(&mut self, error: String) {
        self.loading = false;
        self.confirming = false;
        self.error = Some(error);
    }

    fn preview_applicable(&self) -> bool {
        let Some(preview) = &self.preview else { return false };
        if self.files_mode {
            preview["scope"] == "git_tracked_workspace" && preview["apply_available"] == true
        } else {
            preview["mode"] == "conversation"
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> ModalKey<RewindAction> {
        if key.kind == KeyEventKind::Release {
            return ModalKey::Ignored;
        }
        if key.kind == KeyEventKind::Repeat
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('y' | 'c' | 'f' | 'n'))
        {
            return ModalKey::Ignored;
        }
        if !key.modifiers.is_empty() {
            return ModalKey::Consumed;
        }
        if self.confirming {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    if let Some(preview) = self.preview.clone() {
                        self.confirming = false;
                        ModalKey::Submit(RewindAction::Apply(preview))
                    } else {
                        self.confirming = false;
                        ModalKey::Consumed
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.confirming = false;
                    ModalKey::Consumed
                }
                _ => ModalKey::Consumed,
            };
        }
        match key.code {
            KeyCode::Esc => ModalKey::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                    self.preview = None;
                    if let Some(k) = self.selected_key() {
                        self.loading = true;
                        return ModalKey::Submit(RewindAction::Preview { key: k, files: self.files_mode });
                    }
                }
                ModalKey::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.points.len() {
                    self.selected += 1;
                    self.preview = None;
                    if let Some(k) = self.selected_key() {
                        self.loading = true;
                        return ModalKey::Submit(RewindAction::Preview { key: k, files: self.files_mode });
                    }
                }
                ModalKey::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(4);
                ModalKey::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(4);
                ModalKey::Consumed
            }
            KeyCode::Char('c') => {
                self.files_mode = false;
                self.preview = None;
                if let Some(k) = self.selected_key() {
                    self.loading = true;
                    return ModalKey::Submit(RewindAction::Preview { key: k, files: false });
                }
                ModalKey::Consumed
            }
            KeyCode::Char('f') => {
                self.files_mode = true;
                self.preview = None;
                if let Some(k) = self.selected_key() {
                    self.loading = true;
                    return ModalKey::Submit(RewindAction::Preview { key: k, files: true });
                }
                ModalKey::Consumed
            }
            KeyCode::Char('n') if self.next_before.is_some() => {
                self.loading = true;
                ModalKey::Submit(RewindAction::LoadMore(self.next_before))
            }
            KeyCode::Enter => {
                let selected_matches =
                    self.preview.as_ref().is_some_and(|p| p["envelope"].as_str() == self.selected_key().as_deref());
                if selected_matches && self.preview_applicable() {
                    self.confirming = true;
                    ModalKey::Consumed
                } else if let Some(k) = self.selected_key() {
                    self.loading = true;
                    ModalKey::Submit(RewindAction::Preview { key: k, files: self.files_mode })
                } else {
                    ModalKey::Consumed
                }
            }
            _ => ModalKey::Consumed,
        }
    }

    fn rects(area: Rect) -> (Rect, Rect, Rect, Rect, Rect, Rect) {
        let popup = centered(area, 96, 28);
        let inner = popup.inner(Margin::new(1, 1));
        let top = Rect::new(inner.x, inner.y, inner.width, u16::from(inner.height >= 4));
        let mode_c = Rect::new(top.x, top.y, top.width.min(20), top.height);
        let mode_f = Rect::new(top.x.saturating_add(21), top.y, top.width.saturating_sub(21).min(24), top.height);
        let body = Rect::new(inner.x, inner.y.saturating_add(1), inner.width, inner.height.saturating_sub(2));
        let list_h = (body.height / 2).clamp(3, 10);
        let list = Rect::new(body.x, body.y, body.width, list_h.min(body.height));
        let preview = Rect::new(body.x, body.y + list.height, body.width, body.height.saturating_sub(list.height));
        (popup, mode_c, mode_f, list, preview, Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1))
    }

    pub fn mouse(&mut self, area: Rect, mouse: MouseEvent) -> ModalKey<RewindAction> {
        let (popup, mode_c, mode_f, list, preview, _) = Self::rects(area);
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if !popup.contains(point) => ModalKey::Close,
            MouseEventKind::Down(MouseButton::Left) if mode_c.contains(point) => {
                self.files_mode = false;
                self.confirming = false;
                self.preview = None;
                if let Some(k) = self.selected_key() {
                    self.loading = true;
                    return ModalKey::Submit(RewindAction::Preview { key: k, files: false });
                }
                ModalKey::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) if mode_f.contains(point) => {
                self.files_mode = true;
                self.confirming = false;
                self.preview = None;
                if let Some(k) = self.selected_key() {
                    self.loading = true;
                    return ModalKey::Submit(RewindAction::Preview { key: k, files: true });
                }
                ModalKey::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) if list.contains(point) => {
                let list_inner = list.inner(Margin::new(1, 1));
                if list_inner.contains(point) {
                    if let Some(idx) = item_at(
                        self.points.len(),
                        self.selected,
                        list_inner.height as usize,
                        (point.y - list_inner.y) as usize,
                    ) {
                        if idx != self.selected {
                            self.selected = idx;
                            self.confirming = false;
                            self.preview = None;
                            if let Some(k) = self.selected_key() {
                                self.loading = true;
                                return ModalKey::Submit(RewindAction::Preview { key: k, files: self.files_mode });
                            }
                        }
                    }
                }
                ModalKey::Consumed
            }
            MouseEventKind::ScrollUp if preview.contains(point) => {
                self.scroll = self.scroll.saturating_sub(3);
                ModalKey::Consumed
            }
            MouseEventKind::ScrollDown if preview.contains(point) => {
                self.scroll = self.scroll.saturating_add(3);
                ModalKey::Consumed
            }
            _ => ModalKey::Consumed,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let (popup, mode_c, mode_f, list, preview_area, footer) = Self::rects(area);
        frame.render_widget(Clear, popup);
        frame.render_widget(panel("Rewind · prompt & tracked files").border_style(Style::default().fg(ACCENT)), popup);
        frame.render_widget(
            Paragraph::new(" c Conversation only ").style(if !self.files_mode { selection() } else { secondary() }),
            mode_c,
        );
        frame.render_widget(
            Paragraph::new(" f + Tracked Git files ").style(if self.files_mode { selection() } else { secondary() }),
            mode_f,
        );

        let list_block = panel(format!("Leader prompts ({})", self.points.len()));
        let list_inner = list_block.inner(list);
        frame.render_widget(list_block, list);
        let (start, count) = window(self.points.len(), self.selected, list_inner.height as usize);
        let list_rows: Vec<Line> = if self.points.is_empty() {
            vec![Line::styled(
                if self.loading {
                    "Loading rewind points…"
                } else {
                    "No applicable Leader prompts in current epoch."
                },
                secondary(),
            )]
        } else {
            self.points[start..start + count]
                .iter()
                .enumerate()
                .map(|(offset, row)| {
                    let selected = start + offset == self.selected;
                    let key = row["envelope"].as_str().unwrap_or("?");
                    let pos = row["position"].as_i64().unwrap_or(0);
                    let text = row["preview"].as_str().unwrap_or("");
                    let line = format!("{} #{pos} {key}  {text}", if selected { ">" } else { " " });
                    Line::styled(
                        fit(&line, list_inner.width.saturating_sub(1) as usize),
                        if selected { selection() } else { Style::default().fg(FG) },
                    )
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(list_rows), list_inner);
        render_scrollbar(frame, list_inner, self.points.len(), start);

        let prev_block = panel(if self.files_mode { "Tracked files & prompt preview" } else { "Prompt preview" });
        let prev_inner = prev_block.inner(preview_area);
        frame.render_widget(prev_block, preview_area);
        let mut lines: Vec<Line> = Vec::new();
        if let Some(error) = &self.error {
            lines.push(Line::styled(error.clone(), Style::default().fg(ERROR).add_modifier(Modifier::BOLD)));
        } else if self.loading {
            lines.push(Line::styled("Loading preview…", secondary()));
        } else if let Some(preview) = &self.preview {
            if preview["scope"] == "git_tracked_workspace" {
                lines.push(Line::styled(
                    "Git-tracked workspace restoration preview (external edits included in snapshot_difference):",
                    Style::default().fg(FG).add_modifier(Modifier::BOLD),
                ));
                for file in preview["files"].as_array().into_iter().flatten() {
                    let st = file["status"].as_str().unwrap_or("unavailable");
                    let path = file["path"].as_str().unwrap_or("?");
                    let style = match st {
                        "restored" | "modified" | "snapshot_difference" => Style::default().fg(ACCENT),
                        "conflict" | "missing" | "unavailable" => {
                            Style::default().fg(ERROR).add_modifier(Modifier::BOLD)
                        }
                        _ => secondary(),
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("{st:20} "), style),
                        Span::styled(path.to_string(), Style::default().fg(FG)),
                    ]));
                }
                if preview["apply_available"] != true {
                    lines.push(Line::styled("Conflicts prevent file restoration.", Style::default().fg(ERROR)));
                }
            } else if preview["scope"] == "recorded_leader_edits" {
                lines.push(Line::styled(
                    format!("Read-only edit comparison (snapshot unavailable: {})", preview["snapshot_error"]),
                    Style::default().fg(ERROR),
                ));
                for file in preview["files"].as_array().into_iter().flatten() {
                    lines.push(Line::styled(
                        format!("{}  {}", file["status"].as_str().unwrap_or("?"), file["path"].as_str().unwrap_or("?")),
                        Style::default().fg(FG),
                    ));
                }
            } else {
                lines.push(Line::styled(
                    "Conversation only: keeps prefix before prompt and reopens it in the composer without modifying files.",
                    secondary(),
                ));
                lines.push(Line::default());
                let prompt = preview["draft"]["text"].as_str().unwrap_or("");
                for row in
                    crate::markdown::plain(prompt, prev_inner.width.saturating_sub(1) as usize, Style::default().fg(FG))
                {
                    lines.push(row);
                }
            }
        } else {
            lines.push(Line::styled("Press Enter to preview the selected prompt.", secondary()));
        }
        let max_scroll = lines.len().saturating_sub(prev_inner.height as usize);
        let scroll = self.scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines.iter().skip(scroll).take(prev_inner.height as usize).cloned().collect::<Vec<_>>()),
            prev_inner,
        );
        render_scrollbar(frame, prev_inner, lines.len(), scroll);

        let hint = if self.confirming {
            "Confirm rewind? y apply (reopens prompt in composer) · n / Esc keep current state"
        } else {
            "↑/↓ select · c conversation · f files · Enter preview/apply · n older · Esc close"
        };
        frame.render_widget(Paragraph::new(hint_line(hint)), footer);
    }
}

#[derive(Clone, Debug)]
struct McpRow {
    name: String,
    transport: String,
    required: bool,
    oauth: bool,
    credential: Option<String>,
    running: bool,
    exposed: bool,
    driver_state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpModalAction {
    Refresh,
    Change { action: String, service: String },
}

pub struct McpModal {
    rows: Vec<McpRow>,
    pub selected: usize,
    pub note: Option<String>,
}

impl McpModal {
    pub fn from_snapshot(mcp: &Json, policy: &Json, drivers: &Json) -> Self {
        let mut modal = Self { rows: Vec::new(), selected: 0, note: None };
        modal.refresh_from(mcp, policy, drivers);
        modal
    }

    pub fn refresh_from(&mut self, mcp: &Json, policy: &Json, drivers: &Json) {
        let prev_name = self.rows.get(self.selected).map(|r| r.name.clone());
        let services_policy = &policy["services"];
        self.rows = mcp
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                let name = row["name"].as_str().unwrap_or("?").to_string();
                let rule = &services_policy[&name];
                let running = rule["running"].as_bool().unwrap_or(true);
                let exposed = rule["exposed"].as_bool().unwrap_or(true);
                let driver_val = &drivers[&name];
                let driver_state = if let Some(s) = driver_val.as_str() {
                    s.to_string()
                } else if driver_val.is_object() {
                    driver_val["status"]
                        .as_str()
                        .or_else(|| driver_val["state"].as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| driver_val.to_string())
                } else {
                    "unobserved".into()
                };
                McpRow {
                    name,
                    transport: row["transport"].as_str().unwrap_or("?").to_string(),
                    required: row["required"].as_bool().unwrap_or(false),
                    oauth: row["oauth"].as_bool().unwrap_or(false),
                    credential: row["credential"].as_str().map(str::to_string),
                    running,
                    exposed,
                    driver_state,
                }
            })
            .collect();
        self.selected = prev_name
            .and_then(|name| self.rows.iter().position(|r| r.name == name))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
    }

    fn selected_row(&self) -> Option<&McpRow> {
        self.rows.get(self.selected)
    }

    pub fn key(&mut self, key: KeyEvent) -> ModalKey<McpModalAction> {
        if key.kind == KeyEventKind::Release {
            return ModalKey::Ignored;
        }
        if key.kind == KeyEventKind::Repeat
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('s' | 'h' | 'r'))
        {
            return ModalKey::Ignored;
        }
        if !key.modifiers.is_empty() {
            return ModalKey::Consumed;
        }
        match key.code {
            KeyCode::Esc => ModalKey::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ModalKey::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                ModalKey::Consumed
            }
            KeyCode::Enter => ModalKey::Submit(McpModalAction::Refresh),
            KeyCode::Char('s') => {
                let Some(row) = self.selected_row() else { return ModalKey::Consumed };
                let action = if row.running { "stop" } else { "start" };
                ModalKey::Submit(McpModalAction::Change { action: action.into(), service: row.name.clone() })
            }
            KeyCode::Char('h') => {
                let Some(row) = self.selected_row() else { return ModalKey::Consumed };
                let action = if row.exposed { "hide" } else { "expose" };
                ModalKey::Submit(McpModalAction::Change { action: action.into(), service: row.name.clone() })
            }
            KeyCode::Char('r') => {
                let Some(row) = self.selected_row() else { return ModalKey::Consumed };
                ModalKey::Submit(McpModalAction::Change { action: "reconnect".into(), service: row.name.clone() })
            }
            _ => ModalKey::Consumed,
        }
    }

    pub fn mouse(&mut self, area: Rect, mouse: MouseEvent) -> ModalKey<McpModalAction> {
        let popup = centered(area, 86, 22);
        let inner = popup.inner(Margin::new(1, 1));
        let list = Rect::new(inner.x, inner.y.saturating_add(1), inner.width, inner.height.saturating_sub(3));
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if !popup.contains(point) => ModalKey::Close,
            MouseEventKind::Down(MouseButton::Left) if list.contains(point) => {
                if let Some(idx) =
                    item_at(self.rows.len(), self.selected, list.height as usize, (point.y - list.y) as usize)
                {
                    self.selected = idx;
                }
                ModalKey::Consumed
            }
            MouseEventKind::ScrollUp => {
                self.selected = self.selected.saturating_sub(1);
                ModalKey::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                ModalKey::Consumed
            }
            _ => ModalKey::Consumed,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let popup = centered(area, 86, 22);
        frame.render_widget(Clear, popup);
        let block = panel("MCP services · session policy")
            .title_bottom(Line::from(" Esc close ").alignment(Alignment::Right))
            .border_style(Style::default().fg(ACCENT));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.is_empty() {
            return;
        }
        let header =
            self.note.as_deref().unwrap_or("Desired policy changes refuse while requests or operations are pending.");
        frame.render_widget(
            Paragraph::new(fit(header, inner.width as usize)).style(secondary()),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        let list = Rect::new(inner.x, inner.y.saturating_add(1), inner.width, inner.height.saturating_sub(3));
        let (start, count) = window(self.rows.len(), self.selected, list.height as usize);
        let lines: Vec<Line> = if self.rows.is_empty() {
            vec![Line::styled("No MCP services configured in this session.", secondary())]
        } else {
            self.rows[start..start + count]
                .iter()
                .enumerate()
                .map(|(offset, row)| {
                    let selected = start + offset == self.selected;
                    let run = if row.running { "● running" } else { "○ stopped" };
                    let vis = if row.exposed { "exposed" } else { "hidden" };
                    let req = if row.required { " · required" } else { "" };
                    let auth = match (&row.credential, row.oauth) {
                        (Some(cred), true) => format!(" · auth:{cred}"),
                        (_, true) => " · oauth".into(),
                        _ => String::new(),
                    };
                    let text = format!(
                        "{} {}  {run}  {vis}  [{}]{req}{auth}  driver:{}",
                        if selected { ">" } else { " " },
                        row.name,
                        row.transport,
                        row.driver_state
                    );
                    if selected {
                        Line::styled(fit(&text, list.width.saturating_sub(1) as usize), selection())
                    } else {
                        Line::from(vec![
                            Span::styled(
                                format!("  {}  ", row.name),
                                Style::default().fg(FG).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("{run}  "),
                                Style::default().fg(if row.running { SUCCESS } else { ERROR }),
                            ),
                            Span::styled(
                                format!("{vis}  "),
                                if row.exposed { Style::default().fg(ACCENT) } else { secondary() },
                            ),
                            Span::styled(
                                fit(
                                    &format!("[{}]{req}{auth}  driver:{}", row.transport, row.driver_state),
                                    list.width as usize,
                                ),
                                secondary(),
                            ),
                        ])
                    }
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), list);
        render_scrollbar(frame, list, self.rows.len(), start);
        if inner.height >= 3 {
            frame.render_widget(
                Paragraph::new(hint_line(
                    "↑/↓ select · s start/stop · h hide/expose · r reconnect · Enter refresh · Esc close",
                )),
                Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
            );
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve(String),
    Deny(String),
}

pub struct ApprovalDetail {
    pub approval_id: String,
    pub scroll: usize,
}

impl ApprovalDetail {
    pub fn key(&mut self, key: KeyEvent) -> ModalKey<ApprovalDecision> {
        if key.kind == KeyEventKind::Release {
            return ModalKey::Ignored;
        }
        if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Char('a' | 'd') | KeyCode::Esc) {
            return ModalKey::Ignored;
        }
        if !key.modifiers.is_empty() {
            return ModalKey::Consumed;
        }
        match key.code {
            KeyCode::Esc => ModalKey::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ModalKey::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                ModalKey::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(6);
                ModalKey::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(6);
                ModalKey::Consumed
            }
            KeyCode::Char('a') => ModalKey::Submit(ApprovalDecision::Approve(self.approval_id.clone())),
            KeyCode::Char('d') => ModalKey::Submit(ApprovalDecision::Deny(self.approval_id.clone())),
            _ => ModalKey::Consumed,
        }
    }

    fn rects(area: Rect) -> (Rect, Rect, Rect) {
        let popup = centered(area, 86, 24);
        let inner = popup.inner(Margin::new(1, 1));
        let footer = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, u16::from(!inner.is_empty()));
        let body = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(2));
        (popup, body, footer)
    }

    pub fn mouse(&mut self, area: Rect, mouse: MouseEvent) -> ModalKey<ApprovalDecision> {
        let (popup, _, footer) = Self::rects(area);
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if !popup.contains(point) => ModalKey::Close,
            MouseEventKind::Down(MouseButton::Left) if footer.contains(point) => {
                let rel = point.x.saturating_sub(footer.x);
                if rel < 16 {
                    ModalKey::Submit(ApprovalDecision::Approve(self.approval_id.clone()))
                } else if rel < 26 {
                    ModalKey::Submit(ApprovalDecision::Deny(self.approval_id.clone()))
                } else {
                    ModalKey::Close
                }
            }
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                ModalKey::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.scroll = self.scroll.saturating_add(3);
                ModalKey::Consumed
            }
            _ => ModalKey::Consumed,
        }
    }

    pub fn render(&self, frame: &mut Frame, approval: Option<&ApprovalInfo>, area: Rect) {
        let (popup, body, footer) = Self::rects(area);
        frame.render_widget(Clear, popup);
        let block = panel("Approval details").border_style(Style::default().fg(WARNING).add_modifier(Modifier::BOLD));
        frame.render_widget(block, popup);
        let Some(approval) = approval else {
            frame.render_widget(Paragraph::new("This approval is no longer pending.").style(secondary()), body);
            return;
        };
        let w = body.width.saturating_sub(1) as usize;
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Tool: ", secondary()),
                Span::styled(approval.tool.clone(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("Approval ID: ", secondary()),
                Span::styled(approval.id.clone(), Style::default().fg(FG)),
            ]),
            Line::from(vec![
                Span::styled("Operation ID: ", secondary()),
                Span::styled(approval.operation_id.clone(), Style::default().fg(FG)),
            ]),
        ];
        if !approval.instance_id.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Instance: ", secondary()),
                Span::styled(approval.instance_id.clone(), Style::default().fg(ACCENT)),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::styled("Arguments / intent:", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
        let args = if approval.args.is_empty() { &approval.preview } else { &approval.args };
        lines.extend(crate::markdown::plain(args, w, Style::default().fg(FG)));
        let max_scroll = lines.len().saturating_sub(body.height as usize);
        let scroll = self.scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines.iter().skip(scroll).take(body.height as usize).cloned().collect::<Vec<_>>()),
            body,
        );
        render_scrollbar(frame, body, lines.len(), scroll);
        frame.render_widget(
            Paragraph::new(hint_line("[a Approve once] · [d Deny] · Esc back · Up/Down scroll")),
            footer,
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    PauseMember(String),
    ResumeMember(String),
    TerminateMember(String),
    CancelTask(String),
}

impl ConfirmAction {
    pub fn key(&self, key: KeyEvent) -> ModalKey<Self> {
        if key.kind == KeyEventKind::Release || key.kind == KeyEventKind::Repeat || !key.modifiers.is_empty() {
            return ModalKey::Ignored;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => ModalKey::Submit(self.clone()),
            KeyCode::Char('n') | KeyCode::Esc => ModalKey::Close,
            _ => ModalKey::Consumed,
        }
    }

    pub fn mouse(&self, area: Rect, mouse: MouseEvent) -> ModalKey<Self> {
        let popup = centered(area, 68, 9);
        let point = Position::new(mouse.column, mouse.row);
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return ModalKey::Consumed;
        }
        if !popup.contains(point) {
            return ModalKey::Close;
        }
        let inner = popup.inner(Margin::new(1, 1));
        if point.y == inner.bottom().saturating_sub(1) {
            let rel = point.x.saturating_sub(inner.x);
            if rel < 12 {
                return ModalKey::Submit(self.clone());
            }
            return ModalKey::Close;
        }
        ModalKey::Consumed
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let popup = centered(area, 68, 9);
        frame.render_widget(Clear, popup);
        let (title, summary, danger) = match self {
            Self::PauseMember(id) => (
                format!("Pause {id}?"),
                "Pauses new turns for this member. Already running tool jobs keep their own lifecycle.",
                false,
            ),
            Self::ResumeMember(id) => {
                (format!("Resume {id}?"), "Returns this member to ACTIVE so its driver can resume processing.", false)
            }
            Self::TerminateMember(id) => (
                format!("Terminate {id}?"),
                "Permanent user-only stop: terminates this instance lifecycle. History and receipts stay readable.",
                true,
            ),
            Self::CancelTask(id) => (
                format!("Cancel task {id}?"),
                "Marks the task CANCELLED for the delegator. This alone does not prove its runner process stopped.",
                true,
            ),
        };
        let block = panel(title).border_style(Style::default().fg(if danger { ERROR } else { ACCENT }));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let mut lines = crate::markdown::plain(summary, inner.width as usize, Style::default().fg(FG));
        while lines.len() + 1 < inner.height as usize {
            lines.push(Line::default());
        }
        frame.render_widget(Paragraph::new(lines), inner);
        if !inner.is_empty() {
            let [confirm_area, cancel_area] = Layout::horizontal([Constraint::Length(12), Constraint::Min(0)])
                .areas(Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1));
            frame.render_widget(Paragraph::new("[y Confirm]").style(selection()), confirm_area);
            frame.render_widget(Paragraph::new("[Esc Cancel]").style(secondary()), cancel_area);
        }
    }
}
