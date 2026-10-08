//! D-446: the optional session page. Selection is never a delete confirmation.
use crate::{
    theme::*,
    viewport::{item_at, window},
    wrap::wrap_lines,
};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Borders, Paragraph},
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
    History { id: String, before: Option<i64> },
    Open(SessionRow),
    Delete(SessionRow),
}

pub struct SessionBrowser {
    pub rows: Vec<SessionRow>,
    pub selected: usize,
    pub entries: Vec<Json>,
    pub next_before: Option<i64>,
    pub before: Option<i64>,
    pub newer: Vec<Option<i64>>,
    pub scroll: usize,
    pub last_height: usize,
    pub last_lines: usize,
    pub confirmation: Option<SessionRow>,
    pub busy: bool,
    pub error: Option<String>,
}

impl Default for SessionBrowser {
    fn default() -> Self {
        Self {
            rows: vec![],
            selected: 0,
            entries: vec![],
            next_before: None,
            before: None,
            newer: vec![],
            scroll: 0,
            last_height: 1,
            last_lines: 0,
            confirmation: None,
            busy: true,
            error: None,
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

    pub fn key(&mut self, key: KeyEvent) -> Option<SessionAction> {
        if self.busy {
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
        let previous = self.selected;
        match key.code {
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
                if let Some(index) =
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
        Constraint::Length(3),
    ])
    .split(area)
}

pub fn render(frame: &mut Frame, browser: &mut SessionBrowser, current: &str) {
    let chunks = geometry(frame.area());
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), frame.area());
    frame.render_widget(Paragraph::new(" Sessions").style(Style::default().fg(ACCENT)), chunks[0]);
    let list = Block::default().borders(Borders::ALL).title("All sessions").border_style(Style::default().fg(GREY));
    let inner = list.inner(chunks[1]);
    frame.render_widget(list, chunks[1]);
    let (start, count) = window(browser.rows.len(), browser.selected, inner.height as usize);
    let rows: Vec<Line> = browser.rows[start..start + count]
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            Line::styled(
                format!(
                    "{} {}  {}  {}",
                    if row.attached_path() == current { "*" } else { " " },
                    row.name,
                    row.id,
                    if row.archived {
                        "archived"
                    } else if row.live {
                        "running"
                    } else {
                        "idle"
                    }
                ),
                if start + offset == browser.selected {
                    Style::default().fg(PANEL_BG).bg(ACCENT)
                } else {
                    Style::default().fg(FG)
                },
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), inner);
    let title = browser
        .rows
        .get(browser.selected)
        .map(|row| format!("History: {}", row.name))
        .unwrap_or_else(|| "History".into());
    let history = Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(GREY));
    let inner = history.inner(chunks[2]);
    frame.render_widget(history, chunks[2]);
    let mut lines = Vec::new();
    for entry in &browser.entries {
        let member = entry["instance_id"].as_str().unwrap_or("");
        let who = match entry["kind"].as_str().unwrap_or("") {
            "user" => "You".to_string(),
            "assistant" if member == "i-leader" => "Leader".to_string(),
            "tool_result" => format!("Tool result · {member}"),
            "summary" => "Conversation summary".to_string(),
            kind => format!("{member} · {kind}"),
        };
        lines.push(Line::styled(who, Style::default().fg(ACCENT)));
        let message = &entry["message"];
        let mut content = match &message["content"] {
            Json::String(text) => text.clone(),
            Json::Null => String::new(),
            value => serde_json::to_string_pretty(value).unwrap_or_default(),
        };
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            content.push_str(&format!(
                "\nTool: {}\n{}",
                call["function"]["name"].as_str().unwrap_or("unknown"),
                call["function"]["arguments"].as_str().unwrap_or("")
            ));
        }
        let content: String = content.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
        lines.extend(content.lines().map(|line| Line::from(line.to_string())));
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(Line::from(if browser.busy { "Loading..." } else { "No stored messages." }));
    }
    let lines = wrap_lines(lines, inner.width as usize);
    browser.last_height = (inner.height as usize).max(1);
    browser.last_lines = lines.len();
    browser.scroll = browser.scroll.min(lines.len().saturating_sub(browser.last_height));
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(browser.scroll).take(browser.last_height).collect::<Vec<_>>()),
        inner,
    );
    let footer = if let Some(row) = &browser.confirmation {
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
        "Working...\nCtrl+D detaches; session commands already started may continue.".into()
    } else {
        format!("Up/Down: select  Enter: switch  d: delete  Esc: back\n[: older  ]: newer  PgUp/PgDn: scroll  r: refresh\n{}",
            browser.error.as_deref().unwrap_or(if browser.next_before.is_some() { "Older records available." } else { "Beginning of stored history." }))
    };
    frame.render_widget(
        Paragraph::new(footer).style(
            Style::default().fg(if browser.error.is_some() || browser.confirmation.is_some() { NOTICE } else { GREY }),
        ),
        chunks[3],
    );
}
