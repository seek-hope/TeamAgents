//! D-451: local attachment draft and path picker. Engine reads file bytes only on submit.
use crate::text::Composer;
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Slash,
    Mention { row: usize, start: usize, end: usize },
}

/// Identify the whitespace-delimited @token containing the caret. The completion prefix ends at
/// the caret, while acceptance replaces the whole token. No filesystem access happens here.
fn request_target(composer: &Composer) -> Option<(Target, String)> {
    if let Some(prefix) = composer.text().strip_prefix("/attach ") {
        return Some((Target::Slash, prefix.to_string()));
    }
    let line = composer.lines.get(composer.row)?;
    let col = composer.col.min(line.len());
    let start = (0..col).rev().find(|index| line[*index].is_whitespace()).map_or(0, |index| index + 1);
    let end = (col..line.len()).find(|index| line[*index].is_whitespace()).unwrap_or(line.len());
    if col <= start || line.get(start) != Some(&'@') || end < start + 1 {
        return None;
    }
    let prefix = line[start + 1..col].iter().collect::<String>();
    Some((Target::Mention { row: composer.row, start, end }, prefix))
}

#[derive(Default)]
pub struct Attachments {
    pub paths: Vec<String>,
    pub candidates: Vec<String>,
    pub selected: usize,
    prefix: String,
    target: Option<Target>,
    notice: Option<String>,
    pub limited: bool,
    retry: Option<(String, Vec<String>, String)>,
}
impl Attachments {
    /// Begin either the existing /attach completion or a Grok-style @file reference.
    /// The effect carries only the prefix; the target remains local and is rechecked on reply.
    pub fn request(&mut self, composer: &Composer) -> Option<String> {
        let (target, prefix) = request_target(composer)?;
        self.target = Some(target);
        Some(prefix)
    }

    pub fn apply(&mut self, reply: &Value, composer: &Composer) {
        let Some(prefix) = reply["prefix"].as_str() else { return };
        let Some((target, current)) = request_target(composer) else { return };
        if current != prefix || self.target.as_ref() != Some(&target) {
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

    fn accept_mention(&mut self, composer: &mut Composer, row: usize, start: usize, end: usize) {
        let path = self.candidates[self.selected].clone();
        self.candidates.clear();
        if self.paths.len() >= 8 && !self.paths.contains(&path) {
            self.notice = Some("At most eight attachments per input; the @reference was not changed.".into());
            return;
        }
        let Some(line) = composer.lines.get_mut(row) else { return };
        if end > line.len() || start > end {
            return;
        }
        let mut replacement = format!("@{path}").chars().collect::<Vec<_>>();
        if !replacement.last().is_some_and(|ch| ch.is_whitespace())
            && !line.get(end).is_some_and(|ch| ch.is_whitespace())
        {
            replacement.push(' ');
        }
        let replacement_len = replacement.len();
        line.splice(start..end, replacement);
        composer.row = row;
        composer.col = start + replacement_len;
        if !self.paths.contains(&path) {
            self.paths.push(path.clone());
        }
        self.notice = Some(format!(
            "Attachment {} staged from @reference. Send to submit; /detach INDEX removes it.",
            self.paths.iter().position(|staged| staged == &path).unwrap_or(0) + 1
        ));
    }

    pub fn key(&mut self, composer: &mut Composer, key: KeyEvent) -> bool {
        if !self.visible(composer) || !key.modifiers.is_empty() {
            return false;
        }
        match key.code {
            KeyCode::Esc => self.candidates.clear(),
            KeyCode::Up => self.selected = (self.selected + self.candidates.len() - 1) % self.candidates.len(),
            KeyCode::Down => self.selected = (self.selected + 1) % self.candidates.len(),
            KeyCode::Tab | KeyCode::Enter => match self.target.clone() {
                Some(Target::Mention { row, start, end }) => self.accept_mention(composer, row, start, end),
                _ => {
                    composer.set_text(&format!("/attach {}", self.candidates[self.selected]));
                    self.candidates.clear();
                }
            },
            _ => return false,
        }
        true
    }

    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
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
        self.target = None;
    }
}
