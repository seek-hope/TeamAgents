//! D-451: optional pending-input view. Its editor never owns the conversation draft.
use crate::text::Composer;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
    Frame,
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    List { after: i64 },
    Revise { envelope: String, revision: i64, text: Option<String>, command_id: String },
}

#[derive(Default)]
pub struct Queue {
    pub rows: Vec<Value>,
    pub selected: usize,
    pub next_after: Option<i64>,
    pub editor: Option<Composer>,
    pub confirming: bool,
    pub error: Option<String>,
    // Captures identity/version before editing or confirmation, independent of refreshed rows.
    target: Option<(String, i64)>,
    pending: Option<Action>,
}

impl Queue {
    pub fn apply(&mut self, reply: &Value) {
        self.rows = reply["inputs"].as_array().cloned().unwrap_or_default();
        self.selected = 0;
        self.next_after = reply["next_after"].as_i64();
        self.error = None;
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn saved(&mut self) {
        self.editor = None;
        self.confirming = false;
        self.target = None;
        self.pending = None;
    }

    pub fn escape(&mut self) -> bool {
        if self.editor.is_some() || self.confirming {
            self.saved();
            self.error = None;
            false
        } else {
            true
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(editor) = &mut self.editor {
            for c in text.chars() {
                if c == '\n' {
                    editor.insert_newline();
                } else if !c.is_control() {
                    editor.insert_char(c);
                }
            }
        }
    }

    fn revise(&mut self, text: Option<String>) -> Option<Action> {
        let (envelope, revision) = self.target.clone()?;
        // Reuse the command identity after a lost reply, until the intended payload changes.
        if let Some(action @ Action::Revise { text: previous, .. }) = &self.pending {
            if previous == &text {
                return Some(action.clone());
            }
        }
        let action =
            Action::Revise { envelope, revision, text, command_id: format!("input-change-{}", uuid::Uuid::new_v4()) };
        self.pending = Some(action.clone());
        Some(action)
    }

    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind == KeyEventKind::Release
            || (key.kind == KeyEventKind::Repeat && (self.editor.is_none() || key.code == KeyCode::Enter))
        {
            return None;
        }
        if self.confirming {
            return if key.code == KeyCode::Char('y') && key.modifiers.is_empty() { self.revise(None) } else { None };
        }
        if let Some(editor) = &mut self.editor {
            if key.code == KeyCode::Enter && key.modifiers.is_empty() {
                let text = editor.text();
                if text.trim().is_empty() {
                    self.error = Some("Edited input needs text.".into());
                    return None;
                }
                return self.revise(Some(text));
            }
            editor.edit_key(key);
            return None;
        }
        if !key.modifiers.is_empty() {
            return None;
        }
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1)),
            KeyCode::Char('r') => return Some(Action::List { after: 0 }),
            KeyCode::Char('n') => return self.next_after.map(|after| Action::List { after }),
            KeyCode::Enter | KeyCode::Delete => {
                let row = self.rows.get(self.selected)?;
                self.target = Some((row["envelope_id"].as_str()?.into(), row["revision"].as_i64()?));
                self.error = None;
                if key.code == KeyCode::Enter {
                    let mut editor = Composer::new(vec![]);
                    editor.set_text(row["payload"]["text"].as_str().unwrap_or(""));
                    self.editor = Some(editor);
                } else {
                    self.confirming = true;
                }
            }
            _ => {}
        }
        None
    }
}

pub fn render(frame: &mut Frame, queue: &Queue) {
    use crate::theme::{secondary, BG, CODEX, ERROR, FG};
    use crate::v2ui::{hint_line, panel, selection};
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(3)]).split(area);
    frame.render_widget(
        Paragraph::new("Pending Leader inputs").style(Style::default().fg(CODEX).add_modifier(Modifier::BOLD)),
        rows[0],
    );
    let lines: Vec<Line> = if queue.editor.is_some() {
        vec![]
    } else {
        let height = rows[1].height.saturating_sub(2) as usize;
        let start = queue.selected.saturating_sub(height.saturating_sub(1));
        let mut lines: Vec<Line> = queue
            .rows
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(i, row)| {
                let text = row["payload"]["text"].as_str().unwrap_or("");
                let preview: String = text.chars().take(120).map(|c| if c.is_control() { ' ' } else { c }).collect();
                let count = row["payload"]["attachments"].as_array().map_or(0, Vec::len);
                Line::styled(
                    crate::v2ui::fit(
                        &format!("{} {preview} · {count} files", if i == queue.selected { ">" } else { " " },),
                        rows[1].width.saturating_sub(2) as usize,
                    ),
                    if i == queue.selected { selection() } else { secondary() },
                )
            })
            .collect();
        if lines.is_empty() {
            lines.push(Line::styled("No pending inputs. r refreshes.", secondary()));
        }
        lines
    };
    let title =
        if queue.editor.is_some() { "Edit pending text (attachments retained)" } else { "Accepted, not yet consumed" };
    if let Some(editor) = &queue.editor {
        crate::v2ui::render_editor(frame, editor, rows[1], title, true, "Edit queued message");
    } else {
        frame.render_widget(Paragraph::new(lines).block(panel(title)), rows[1]);
    }
    let footer = if queue.confirming {
        format!("Cancel {}? y confirms; Esc keeps it.", queue.target.as_ref().map(|t| t.0.as_str()).unwrap_or("input"))
    } else if queue.editor.is_some() {
        "Enter save · Esc discard · Ctrl+U/K delete · Ctrl+Y restore".into()
    } else {
        "Enter edit · Esc close · Up/Down select · Delete cancel · n next page · r refresh".into()
    };
    if let Some(error) = queue.error.as_deref() {
        frame
            .render_widget(Paragraph::new(error).style(Style::default().fg(ERROR)).wrap(Wrap { trim: false }), rows[2]);
    } else {
        frame.render_widget(Paragraph::new(hint_line(&footer)).wrap(Wrap { trim: false }), rows[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(id: &str, revision: i64, text: &str) -> Value {
        json!({"envelope_id": id, "revision": revision, "payload": {"text": text, "attachments": [{"digest":"sha256:x"}]}})
    }

    #[test]
    fn edit_keeps_original_identity_and_attachment_reference() {
        let mut queue = Queue::default();
        queue.apply(&json!({"inputs": [row("env-a", 3, "old")]}));
        assert!(queue.key(key(KeyCode::Enter)).is_none());
        if let Some(editor) = &mut queue.editor {
            editor.set_text("new text");
        }
        let action = queue.key(key(KeyCode::Enter)).expect("edit action");
        match action {
            Action::Revise { envelope, revision, text, .. } => {
                assert_eq!(envelope, "env-a");
                assert_eq!(revision, 3);
                assert_eq!(text.as_deref(), Some("new text"));
            }
            Action::List { .. } => panic!("edit must be a revision"),
        }
        assert!(queue.editor.is_some(), "the editor remains until the daemon receipt arrives");
        queue.saved();
        assert!(queue.editor.is_none());
    }

    #[test]
    fn cancel_requires_confirmation_and_escape_does_not_dispatch() {
        let mut queue = Queue::default();
        queue.apply(&json!({"inputs": [row("env-b", 0, "cancel me")]}));
        assert!(queue.key(key(KeyCode::Delete)).is_none());
        assert!(queue.confirming);
        assert!(queue.key(key(KeyCode::Char('n'))).is_none());
        assert!(queue.confirming);
        assert!(!queue.escape());
        assert!(!queue.confirming);
        assert!(queue.key(key(KeyCode::Delete)).is_none());
        let action = queue.key(key(KeyCode::Char('y'))).expect("confirmed cancellation");
        assert!(
            matches!(action, Action::Revise { text: None, envelope, revision, .. } if envelope == "env-b" && revision == 0)
        );
    }

    #[test]
    fn queue_page_uses_shared_rounded_chrome_and_selection() {
        let mut queue = Queue::default();
        queue.apply(&json!({"inputs": [row("env-b", 0, "cancel me")]}));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(72, 12)).unwrap();
        terminal.draw(|frame| render(frame, &queue)).unwrap();
        let buffer = terminal.backend().buffer();
        let corner = buffer[(0, 1)].clone();
        assert_eq!(corner.symbol(), "╭");
        assert_eq!(corner.fg, crate::theme::BORDER);
        let selected = buffer[(1, 2)].clone();
        assert_eq!(selected.symbol(), ">");
        assert_eq!(selected.fg, ratatui::style::Color::Reset);
        assert!(selected.modifier.contains(ratatui::style::Modifier::BOLD | ratatui::style::Modifier::REVERSED));
        assert!(!selected.modifier.contains(ratatui::style::Modifier::DIM));
        assert!(!selected.modifier.contains(ratatui::style::Modifier::UNDERLINED));
    }

    #[test]
    fn paged_refresh_and_stale_revision_error_are_visible() {
        let mut queue = Queue::default();
        queue.apply(&json!({"inputs": [row("env-a", 1, "first")], "next_after": 1}));
        assert_eq!(queue.next_after, Some(1));
        assert_eq!(queue.key(key(KeyCode::Char('n'))), Some(Action::List { after: 1 }));
        queue.fail("revision 2 != expected 1: refresh before editing".into());
        assert_eq!(queue.error.as_deref(), Some("revision 2 != expected 1: refresh before editing"));
    }
}
