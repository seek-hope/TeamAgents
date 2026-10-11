//! D-450: local completion edits a draft; only the app's ordinary submit path executes a command.
use crate::text::Composer;
use crossterm::event::{KeyCode, KeyEvent};

pub const COMMANDS: [(&str, &str); 7] = [
    ("/help", "Show controls"),
    ("/sessions", "Browse conversations"),
    ("/interrupt", "Interrupt the Leader model request"),
    ("/quit", "Detach from this session"),
    ("/queue", "Edit or cancel pending input"),
    ("/attach", "Stage a file; Tab completes paths"),
    ("/detach", "Remove an attachment by index"),
];

#[derive(Default)]
pub struct Completion {
    prefix: String,
    selected: usize,
    hidden: bool,
}
impl Completion {
    pub fn candidates(&self, composer: &Composer) -> Vec<(&'static str, &'static str)> {
        if composer.lines.len() != 1 || composer.col != composer.lines[0].len() {
            return vec![];
        }
        let text = composer.text();
        if !text.starts_with('/') || (self.hidden && text == self.prefix) || COMMANDS.iter().any(|c| c.0 == text) {
            return vec![];
        }
        COMMANDS.iter().copied().filter(|c| c.0.starts_with(&text)).collect()
    }

    pub fn selected(&self, composer: &Composer) -> usize {
        if self.prefix == composer.text() {
            self.selected
        } else {
            0
        }
    }

    /// Return true only when the key belongs to the completion menu. No business effect is possible here.
    pub fn key(&mut self, composer: &mut Composer, key: KeyEvent) -> bool {
        let text = composer.text();
        if self.prefix != text {
            self.prefix = text;
            self.selected = 0;
            self.hidden = false;
        }
        let candidates = self.candidates(composer);
        if candidates.is_empty() || !key.modifiers.is_empty() {
            return false;
        }
        self.selected = self.selected.min(candidates.len() - 1);
        match key.code {
            KeyCode::Up => self.selected = (self.selected + candidates.len() - 1) % candidates.len(),
            KeyCode::Down => self.selected = (self.selected + 1) % candidates.len(),
            KeyCode::Tab | KeyCode::Enter => composer.set_text(candidates[self.selected].0),
            KeyCode::Esc => self.hidden = true,
            _ => return false,
        }
        true
    }
}
