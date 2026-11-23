//! D-450: local completion edits a draft; only the app's ordinary submit path executes a command.
use crate::text::Composer;
use crossterm::event::{KeyCode, KeyEvent};

macro_rules! commands {
    ($($variant:ident => ($name:literal, $help:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Command { $($variant),+ }
        pub const COMMANDS: [(&str, &str); 21] = [$(($name, $help)),+];
        impl Command {
            pub fn parse(text: &str) -> Option<(Self, Option<&str>)> {
                let end = text.find(char::is_whitespace).unwrap_or(text.len());
                let argument = text.get(end..).and_then(|rest| rest.chars().next().map(|c| &rest[c.len_utf8()..]));
                let command = match &text[..end] { $($name => Self::$variant,)+ _ => return None };
                Some((command, argument))
            }
        }
    };
}

commands! {
    Help => ("/help", "Show keyboard shortcuts, commands, and session info"),
    Model => ("/model", "Pick the AI model and thinking level for chat or workers"),
    Edit => ("/edit", "Open your draft (or TEXT) in your external text editor (Ctrl+E)"),
    Sessions => ("/sessions", "Browse, search, or switch between saved chats"),
    Rewind => ("/rewind", "Undo or go back to an earlier message or file state"),
    CodeGraph => ("/codegraph", "Check, create, or update the project's code map index"),
    Mcp => ("/mcp", "Manage connected tool plugins (start, stop, or reconnect)"),
    Shell => ("/shell", "Run a local terminal command (/shell view|draft COMMAND)"),
    Interrupt => ("/interrupt", "Stop the current AI reply right now"),
    Quit => ("/quit", "Close this window (your background session keeps running)"),
    Compact => ("/compact", "Summarize older messages to free up memory (optional focus TEXT)"),
    Later => ("/later", "Send TEXT automatically after the current task finishes"),
    Queue => ("/queue", "View, edit, or cancel messages waiting to be sent"),
    Tree => ("/tree", "View the session's branching topic map"),
    Attach => ("/attach", "Attach a file to your message (or type @path + Tab)"),
    Detach => ("/detach", "Remove an attached file by its number"),
    Team => ("/team", "Switch focus to the AI team member list"),
    Chat => ("/chat", "Return to the main conversation view"),
    Tasks => ("/tasks", "See the team's task list and results"),
    Activity => ("/activity", "See recent team actions and events"),
    Inspect => ("/inspect", "View details for the selected team member (h for history)"),
}

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

    /// Select and fill one completion candidate into the draft without executing it.
    pub fn apply_at(&mut self, composer: &mut Composer, index: usize) -> bool {
        let candidates = self.candidates(composer);
        let Some((name, _)) = candidates.get(index) else {
            return false;
        };
        self.selected = index;
        composer.set_text(name);
        true
    }
}
