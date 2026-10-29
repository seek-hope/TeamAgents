//! D-446: the optional session page. Selection is never a delete confirmation.
use crate::{
    theme::*,
    viewport::{item_at, window},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Paragraph},
    Frame,
};
use serde_json::Value as Json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub name: String,
    pub path: String,
    pub archived: bool,
    pub live: bool,
}

impl SessionRow {
    pub fn attached_path(&self) -> String {
        if self.archived {
            if let Some(base) = std::path::Path::new(&self.path).parent().and_then(std::path::Path::parent) {
                return base.join("sessions").join(&self.id).to_string_lossy().into_owned();
            }
        }
        self.path.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionAction {
    List,
    Search {
        query: String,
        generation: u64,
    },
    /// Cancel a browser search without running another session CLI command.
    CancelSearch,
    History {
        id: String,
        before: Option<i64>,
    },
    Open(SessionRow),
    Delete(SessionRow),
}

pub struct SessionBrowser {
    pub rows: Vec<SessionRow>,
    pub selected: usize,
    pub entries: Vec<Json>,
    transcript: crate::transcript::Transcript,
    pub next_before: Option<i64>,
    pub before: Option<i64>,
    pub newer: Vec<Option<i64>>,
    pub scroll: usize,
    pub last_height: usize,
    pub last_lines: usize,
    pub confirmation: Option<SessionRow>,
    pub busy: bool,
    pub error: Option<String>,
    /// Browser-local search state. Searching never changes the active session; only Open does.
    pub search_query: String,
    pub search_editing: bool,
    pub search_hits: Vec<Json>,
    pub search_selected: usize,
    pub search_skipped: usize,
    pub search_pending: bool,
    search_generation: u64,
}

impl Default for SessionBrowser {
    fn default() -> Self {
        Self {
            rows: vec![],
            selected: 0,
            entries: vec![],
            transcript: crate::transcript::Transcript::default(),
            next_before: None,
            before: None,
            newer: vec![],
            scroll: 0,
            last_height: 1,
            last_lines: 0,
            confirmation: None,
            busy: true,
            error: None,
            search_query: String::new(),
            search_editing: false,
            search_hits: vec![],
            search_selected: 0,
            search_skipped: 0,
            search_pending: false,
            search_generation: 0,
        }
    }
}

impl SessionBrowser {
    pub fn apply_list(&mut self, result: &Json, current: &str) -> Option<SessionAction> {
        let selected = self.rows.get(self.selected).map(|row| row.id.clone());
        self.rows = result["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| {
                row["id"] != "default" || row["has_database"].as_bool().unwrap_or(true) || row["path"] == current
            })
            .map(|row| SessionRow {
                id: row["id"].as_str().unwrap_or("").into(),
                name: row["name"].as_str().unwrap_or("").into(),
                path: row["path"].as_str().unwrap_or("").into(),
                archived: row["archived"].as_bool().unwrap_or(false),
                live: row["live"].as_bool().unwrap_or(false),
            })
            .collect();
        self.selected = self
            .rows
            .iter()
            .position(|row| selected.as_ref().map_or(row.attached_path() == current, |id| &row.id == id))
            .unwrap_or(0);
        self.load_selected()
    }

    fn load_selected(&mut self) -> Option<SessionAction> {
        self.entries.clear();
        self.newer.clear();
        self.before = None;
        self.next_before = None;
        self.scroll = 0;
        self.confirmation = None;
        self.error = None;
        let row = self.rows.get(self.selected)?;
        self.busy = true;
        Some(SessionAction::History { id: row.id.clone(), before: None })
    }

    pub fn apply_search(&mut self, result: &Json, query: &str, generation: u64) {
        // A cancelled or superseded worker may still finish; both the query and generation identify its result.
        if self.search_query != query || self.search_generation != generation {
            return;
        }
        self.search_hits = result["hits"].as_array().cloned().unwrap_or_default();
        self.search_selected = self.search_selected.min(self.search_hits.len().saturating_sub(1));
        self.search_skipped = result["skipped"].as_array().map_or(0, Vec::len);
        self.search_pending = false;
        self.search_editing = false;
        self.busy = false;
        self.error = None;
    }

    pub fn begin_search(&mut self) {
        self.search_editing = true;
        self.error = None;
    }

    pub fn cancel_search(&mut self) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_query.clear();
        self.search_editing = false;
        self.search_hits.clear();
        self.search_selected = 0;
        self.search_skipped = 0;
        self.search_pending = false;
        self.busy = false;
        self.error = None;
    }

    pub fn search_active(&self) -> bool {
        self.search_editing || !self.search_query.is_empty() || !self.search_hits.is_empty()
    }

    pub fn paste_search(&mut self, text: &str) {
        if !self.search_editing || self.busy {
            return;
        }
        for character in text.chars() {
            if !character.is_control() && self.search_query.chars().count() < 256 {
                self.search_query.push(character);
            }
        }
    }

    fn search_row(&self) -> Option<SessionRow> {
        let id = self.search_hits.get(self.search_selected)?["session_id"].as_str()?;
        self.rows.iter().find(|row| row.id == id).cloned()
    }

    pub fn apply_history(&mut self, result: &Json, before: Option<i64>) {
        if before != self.before {
            if self.newer.last().is_some_and(|cursor| *cursor == before) {
                self.newer.pop();
            } else {
                self.newer.push(self.before);
            }
            self.before = before;
        }
        self.error = None;
        self.entries = result["entries"].as_array().cloned().unwrap_or_default();
        self.next_before = result["next_before"].as_i64();
        self.scroll = 0;
        self.busy = false;
    }

    pub fn fail(&mut self, error: String) {
        self.busy = false;
        self.confirmation = None;
        self.error = Some(error);
    }

    pub fn fail_search(&mut self, error: String, query: &str, generation: u64) {
        if self.search_query == query && self.search_generation == generation {
            self.search_pending = false;
            self.busy = false;
            self.error = Some(error);
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Option<SessionAction> {
        if self.busy {
            if key.code == KeyCode::Esc && self.search_pending {
                self.cancel_search();
                return Some(SessionAction::CancelSearch);
            }
            return None;
        }
        if self.search_editing {
            match key.code {
                KeyCode::Esc => self.cancel_search(),
                KeyCode::Backspace if key.modifiers.is_empty() => {
                    self.search_query.pop();
                    self.error = None;
                }
                KeyCode::Enter if key.modifiers.is_empty() => {
                    if self.search_query.trim().is_empty() {
                        self.error = Some("Search needs a query.".into());
                    } else {
                        self.search_generation = self.search_generation.wrapping_add(1);
                        self.search_hits.clear();
                        self.search_selected = 0;
                        self.search_skipped = 0;
                        self.search_pending = true;
                        self.search_editing = false;
                        self.busy = true;
                        self.error = None;
                        return Some(SessionAction::Search {
                            query: self.search_query.clone(),
                            generation: self.search_generation,
                        });
                    }
                }
                KeyCode::Char(character)
                    if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !character.is_control()
                        && self.search_query.chars().count() < 256 =>
                {
                    self.search_query.push(character);
                    self.error = None;
                }
                _ => {}
            }
            return None;
        }
        if let Some(row) = self.confirmation.clone() {
            // Only the explicit y key confirms this captured identity; navigation cannot retarget it.
            match key.code {
                KeyCode::Char('y') if key.modifiers.is_empty() => {
                    self.busy = true;
                    self.confirmation = None;
                    return Some(SessionAction::Delete(row));
                }
                KeyCode::Esc | KeyCode::Char('n') => self.confirmation = None,
                _ => {}
            }
            return None;
        }
        if key.code == KeyCode::Esc && self.search_active() {
            let cancel_pending = self.search_pending;
            self.cancel_search();
            if cancel_pending {
                return Some(SessionAction::CancelSearch);
            }
            return None;
        }
        let previous = self.selected;
        match key.code {
            KeyCode::Char('/') if key.modifiers.is_empty() => self.begin_search(),
            KeyCode::Char('d') | KeyCode::Delete if self.search_active() => {}
            KeyCode::Char('[') | KeyCode::Char(']') if self.search_active() => {}
            KeyCode::Up if self.search_active() => {
                self.search_selected = self.search_selected.saturating_sub(1);
            }
            KeyCode::Down if self.search_active() => {
                self.search_selected = (self.search_selected + 1).min(self.search_hits.len().saturating_sub(1));
            }
            KeyCode::Home if self.search_active() => self.search_selected = 0,
            KeyCode::End if self.search_active() => self.search_selected = self.search_hits.len().saturating_sub(1),
            KeyCode::Enter if self.search_active() => {
                if let Some(row) = self.search_row() {
                    self.busy = true;
                    self.error = None;
                    self.search_pending = false;
                    return Some(SessionAction::Open(row));
                }
                self.error = Some(if self.search_hits.is_empty() {
                    "No matching stored messages.".into()
                } else {
                    "The selected search result is not in the session list.".into()
                });
            }
            KeyCode::Char('r') if self.search_active() && !self.search_query.trim().is_empty() => {
                self.search_generation = self.search_generation.wrapping_add(1);
                self.search_hits.clear();
                self.search_selected = 0;
                self.search_skipped = 0;
                self.search_pending = true;
                self.busy = true;
                self.error = None;
                return Some(SessionAction::Search {
                    query: self.search_query.clone(),
                    generation: self.search_generation,
                });
            }
            KeyCode::PageUp if self.search_active() => {
                self.search_selected = self.search_selected.saturating_sub(10);
            }
            KeyCode::PageDown if self.search_active() => {
                self.search_selected = (self.search_selected + 10).min(self.search_hits.len().saturating_sub(1));
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1)),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.rows.len().saturating_sub(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.last_height),
            KeyCode::PageDown => {
                self.scroll = (self.scroll + self.last_height).min(self.last_lines.saturating_sub(self.last_height))
            }
            KeyCode::Char('r') => {
                self.busy = true;
                self.error = None;
                return Some(SessionAction::List);
            }
            KeyCode::Char('d') | KeyCode::Delete => self.confirmation = self.rows.get(self.selected).cloned(),
            KeyCode::Enter => {
                let row = self.rows.get(self.selected)?.clone();
                self.busy = true;
                self.error = None;
                return Some(SessionAction::Open(row));
            }
            KeyCode::Char('[') if self.next_before.is_some() => {
                self.busy = true;
                return Some(SessionAction::History {
                    id: self.rows.get(self.selected)?.id.clone(),
                    before: self.next_before,
                });
            }
            KeyCode::Char(']') if !self.newer.is_empty() => {
                self.busy = true;
                return Some(SessionAction::History {
                    id: self.rows.get(self.selected)?.id.clone(),
                    before: self.newer.last().copied().flatten(),
                });
            }
            _ => {}
        }
        if self.selected != previous {
            return self.load_selected();
        }
        None
    }

    pub fn mouse(&mut self, area: Rect, mouse: MouseEvent) -> Option<SessionAction> {
        if self.busy || self.confirmation.is_some() {
            return None;
        }
        let chunks = geometry(area);
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        let inner = chunks[1].inner(ratatui::layout::Margin::new(1, 1));
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if inner.contains(point) => {
                if self.search_active() {
                    if let Some(index) = item_at(
                        self.search_hits.len(),
                        self.search_selected,
                        inner.height as usize,
                        (mouse.row - inner.y) as usize,
                    ) {
                        self.search_selected = index;
                    }
                } else if let Some(index) =
                    item_at(self.rows.len(), self.selected, inner.height as usize, (mouse.row - inner.y) as usize)
                {
                    if self.selected != index {
                        self.selected = index;
                        return self.load_selected();
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if chunks[1].contains(point) {
                    return self.key(KeyEvent::from(if up { KeyCode::Up } else { KeyCode::Down }));
                }
                if chunks[2].contains(point) {
                    self.scroll = if up {
                        self.scroll.saturating_sub(3)
                    } else {
                        (self.scroll + 3).min(self.last_lines.saturating_sub(self.last_height))
                    };
                }
            }
            _ => {}
        }
        None
    }
}

fn geometry(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length((area.height / 3).clamp(3, 9)),
        Constraint::Min(0),
        Constraint::Length(if area.width < 60 { 5 } else { 3 }),
    ])
    .split(area)
}

pub fn render(
    frame: &mut Frame,
    browser: &mut SessionBrowser,
    current: &str,
    graphics: &mut crate::graphics::Graphics,
) -> Vec<crate::graphics::Placement> {
    let chunks = geometry(frame.area());
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), frame.area());
    frame.render_widget(
        Paragraph::new(format!(
            " Sessions{}",
            if browser.search_active() {
                format!("  ·  search: {}{}", browser.search_query, if browser.search_editing { "▌" } else { "" })
            } else {
                String::new()
            }
        ))
        .style(Style::default().fg(CODEX).add_modifier(ratatui::style::Modifier::BOLD)),
        chunks[0],
    );
    let list = crate::v2ui::panel(if browser.search_active() { "Search results" } else { "All sessions" });
    let inner = list.inner(chunks[1]);
    frame.render_widget(list, chunks[1]);
    let (start, count) = if browser.search_active() {
        window(browser.search_hits.len(), browser.search_selected, inner.height as usize)
    } else {
        window(browser.rows.len(), browser.selected, inner.height as usize)
    };
    let rows: Vec<Line> = if browser.search_active() {
        browser.search_hits[start..start + count]
            .iter()
            .enumerate()
            .map(|(offset, hit)| {
                let session = hit["session_id"].as_str().unwrap_or("?");
                let instance = hit["instance_id"].as_str().unwrap_or("?");
                let idx = hit["idx"].as_i64().unwrap_or_default();
                let snippet = hit["snippet"].as_str().unwrap_or("");
                Line::styled(
                    crate::markdown::clean(&format!(
                        "{} {session}  {instance}:{idx}  {snippet}",
                        if start + offset == browser.search_selected { ">" } else { " " }
                    )),
                    if start + offset == browser.search_selected {
                        crate::v2ui::selection()
                    } else {
                        Style::default().fg(FG)
                    },
                )
            })
            .collect()
    } else {
        browser.rows[start..start + count]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                Line::styled(
                    format!(
                        "{} {}  {}  {}",
                        if start + offset == browser.selected {
                            ">"
                        } else if row.attached_path() == current {
                            "*"
                        } else {
                            " "
                        },
                        crate::v2ui::fit(&row.name, usize::from(inner.width.saturating_sub(13)).min(32)),
                        if row.archived {
                            "archived"
                        } else if row.live {
                            "running"
                        } else {
                            "idle"
                        },
                        row.id,
                    ),
                    if start + offset == browser.selected { crate::v2ui::selection() } else { Style::default().fg(FG) },
                )
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(rows), inner);
    let title = if browser.search_active() {
        browser
            .search_hits
            .get(browser.search_selected)
            .and_then(|hit| hit["snippet"].as_str())
            .map(|snippet| format!("Search hit: {}", crate::markdown::clean(snippet)))
            .unwrap_or_else(|| "Search hit".into())
    } else {
        browser
            .rows
            .get(browser.selected)
            .map(|row| format!("History: {}", row.name))
            .unwrap_or_else(|| "History".into())
    };
    let history = crate::v2ui::panel(title);
    let inner = history.inner(chunks[2]);
    frame.render_widget(history, chunks[2]);
    let entries =
        if browser.search_active() { Vec::new() } else { crate::transcript::project(&browser.entries, "", false) };
    let root = browser.rows.get(browser.selected).map(|r| r.path.as_str()).unwrap_or("");
    let mut document = browser.transcript.document(&entries, inner.width as usize, None, Some(graphics), root).clone();
    let mut lines = document.lines.clone();
    if browser.search_active() && !browser.search_editing && !browser.search_pending {
        if let Some(hit) = browser.search_hits.get(browser.search_selected) {
            lines = crate::markdown::plain(
                &format!(
                    "Session: {}\nMember: {} · epoch {} · entry {}\n\n{}\n\nSnippet only. Enter opens this session.",
                    hit["session_id"].as_str().unwrap_or("?"),
                    hit["instance_id"].as_str().unwrap_or("?"),
                    hit["epoch"].as_i64().map_or_else(|| "?".into(), |n| n.to_string()),
                    hit["idx"].as_i64().unwrap_or_default(),
                    hit["snippet"].as_str().unwrap_or("")
                ),
                inner.width as usize,
                Style::default().fg(FG),
            );
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(if browser.search_pending {
            "Searching…"
        } else if browser.search_active() {
            "Press Enter on a result to open its session."
        } else if browser.busy {
            "Loading..."
        } else {
            "No stored messages."
        }));
    }
    browser.last_height = (inner.height as usize).max(1);
    browser.last_lines = lines.len();
    browser.scroll = browser.scroll.min(lines.len().saturating_sub(browser.last_height));
    document.resolve(graphics, browser.scroll, inner);
    if !document.lines.is_empty() {
        lines = document.lines.clone();
    }
    let pictures = document.visible(browser.scroll, inner);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(browser.scroll).take(browser.last_height).collect::<Vec<_>>()),
        inner,
    );
    let footer = if browser.search_editing {
        format!(
            "{}\nType a query; Enter: search  Backspace: edit  Esc: cancel",
            browser.error.as_deref().unwrap_or("Search is local to stored messages.")
        )
    } else if browser.search_active() {
        format!(
            "Up/Down: select hit  PgUp/PgDn: jump  Enter: open session  /: edit  Esc: clear search\n{}{}",
            if browser.search_pending { "Search in progress; Esc cancels. ".to_string() } else { String::new() },
            if browser.search_skipped > 0 {
                format!("{} unreadable session(s) skipped. ", browser.search_skipped)
            } else {
                String::new()
            },
        ) + if browser.search_pending {
            ""
        } else {
            browser.error.as_deref().unwrap_or(if browser.search_hits.is_empty() {
                "No matching stored messages."
            } else {
                "Search results are local; opening requires Enter."
            })
        }
    } else if let Some(row) = &browser.confirmation {
        format!(
            "Delete {} ({})? Stop its work and erase stored history.\n{}\ny: delete permanently  n/Esc: cancel",
            row.name.chars().take(40).collect::<String>(),
            row.id,
            if row.attached_path() == current {
                "This is the current session. Success exits the TUI."
            } else {
                "Your current session stays open."
            }
        )
    } else if browser.busy {
        "Working...\nCtrl+Q detaches; session commands already started may continue.".into()
    } else {
        format!("Enter switch · Esc back · / search · Up/Down select · d delete\n[ older · ] newer · PgUp/PgDn scroll · r refresh\n{}",
            browser.error.as_deref().unwrap_or(if browser.next_before.is_some() { "Older records available." } else { "Beginning of stored history." }))
    };
    let footer_lines = footer
        .lines()
        .flat_map(|line| crate::wrap::wrap_line(&crate::v2ui::hint_line(line), chunks[3].width as usize))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(footer_lines).style(if browser.error.is_some() || browser.confirmation.is_some() {
            Style::default().fg(NOTICE)
        } else {
            secondary()
        }),
        chunks[3],
    );
    pictures
}
