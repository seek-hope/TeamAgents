//! D-451: local attachment draft and path picker. Engine reads file bytes only on submit.
use crate::text::Composer;
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

#[derive(Default)]
pub struct Attachments {
    pub paths: Vec<String>,
    pub candidates: Vec<String>,
    pub selected: usize,
    prefix: String,
    pub limited: bool,
    retry: Option<(String, Vec<String>, String)>,
}
impl Attachments {
    pub fn apply(&mut self, reply: &Value, composer: &Composer) {
        let Some(prefix) = reply["prefix"].as_str() else { return };
        if composer.text().strip_prefix("/attach ") != Some(prefix) {
            return;
        }
        self.prefix = composer.text();
        self.candidates =
            reply["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)).collect();
        self.selected = 0;
        self.limited = reply["limited"].as_bool().unwrap_or(false);
    }
    pub fn visible(&self, composer: &Composer) -> bool {
        !self.candidates.is_empty() && self.prefix == composer.text()
    }
    pub fn key(&mut self, composer: &mut Composer, key: KeyEvent) -> bool {
        if !self.visible(composer) || !key.modifiers.is_empty() {
            return false;
        }
        match key.code {
            KeyCode::Esc => self.candidates.clear(),
            KeyCode::Up => self.selected = (self.selected + self.candidates.len() - 1) % self.candidates.len(),
            KeyCode::Down => self.selected = (self.selected + 1) % self.candidates.len(),
            KeyCode::Tab | KeyCode::Enter => {
                composer.set_text(&format!("/attach {}", self.candidates[self.selected]));
                self.candidates.clear();
            }
            _ => return false,
        }
        true
    }
    pub fn envelope(&mut self, text: &str) -> String {
        if let Some((previous, paths, envelope)) = &self.retry {
            if text == previous && &self.paths == paths {
                return envelope.clone();
            }
        }
        let envelope = format!("env-{}", uuid::Uuid::new_v4());
        self.retry = Some((text.into(), self.paths.clone(), envelope.clone()));
        envelope
    }
    pub fn accepted(&mut self) {
        self.paths.clear();
        self.retry = None;
        self.candidates.clear();
    }
}
