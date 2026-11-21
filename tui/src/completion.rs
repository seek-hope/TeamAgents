//! D-450: local completion edits a draft; only the app's ordinary submit path executes a command.
use crate::text::Composer;
use crossterm::event::{KeyCode, KeyEvent};

macro_rules! commands {
    ($($variant:ident => ($name:literal, $help:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Command { $($variant),+ }
        pub const COMMANDS: [(&str, &str); 20] = [$(($name, $help)),+];
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
    Help => ("/help", "Show controls"),
    Model => ("/model", "Choose the Leader model and effort or the shared Worker model"),
    Edit => ("/edit", "Edit optional TEXT in the external editor; Ctrl+E edits the current draft"),
    Sessions => ("/sessions", "Browse conversations"),
    Rewind => ("/rewind", "List Leader prompts; preview KEY, apply or reopen the last draft"),
    Mcp => ("/mcp", "Inspect session MCP services, or start|stop|reconnect|hide|expose KEY"),
    Shell => ("/shell", "Open a local shell, or use /shell view|draft COMMAND for bounded stdout"),
    Interrupt => ("/interrupt", "Interrupt the Leader model request"),
    Quit => ("/quit", "Detach from this session"),
    Compact => ("/compact", "Summarize older Leader context, optionally emphasizing TEXT"),
    Later => ("/later", "Send TEXT after the Leader's current work settles"),
    Queue => ("/queue", "Edit or cancel pending input"),
    Tree => ("/tree", "Show the session exploration tree"),
    Attach => ("/attach", "Stage a file; @path + Tab also completes"),
    Detach => ("/detach", "Remove an attachment by index"),
    Team => ("/team", "Focus the team member sidebar"),
    Chat => ("/chat", "Return to the Leader conversation"),
    Tasks => ("/tasks", "Inspect tasks and recorded outcomes"),
    Activity => ("/activity", "Show recent observed team events"),
    Inspect => ("/inspect", "Inspect the selected member; h opens history"),
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
}
