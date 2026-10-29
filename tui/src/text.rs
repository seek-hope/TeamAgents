//! Composer buffer.
//! Enter sends, Shift+Enter inserts a newline, ↑↓ recall history at the
//! first/last row. Lines are Vec<char> so multibyte editing stays simple.

use unicode_width::UnicodeWidthChar;

/// Input history depth kept for arrow-key recall.
pub const HISTORY_LIMIT: usize = 500;

pub struct Composer {
    pub lines: Vec<Vec<char>>,
    pub row: usize,
    pub col: usize,
    pub history: Vec<String>,
    pub hist_idx: usize,
    pub draft: String,
    // One local kill buffer shared by line/word deletion, never sent separately.
    killed: String,
}

impl Composer {
    pub fn new(history: Vec<String>) -> Composer {
        let hist_idx = history.len();
        Composer { lines: vec![vec![]], row: 0, col: 0, history, hist_idx, draft: String::new(), killed: String::new() }
    }

    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.lines.is_empty() {
            self.lines.push(vec![]);
        }
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    pub fn clear(&mut self) {
        self.set_text("");
    }

    pub fn insert_char(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        let rest: Vec<char> = self.lines[self.row].split_off(self.col);
        self.row += 1;
        self.lines.insert(self.row, rest);
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
        } else if self.row > 0 {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(cur);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.lines[self.row].len();
    }

    /// action_submit_prompt: strip; empty is a no-op; returns Some(text).
    fn word_boundary_left(line: &[char], col: usize) -> usize {
        let mut i = col;
        while i > 0 && line[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !line[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_boundary_right(line: &[char], col: usize) -> usize {
        let mut i = col;
        while i < line.len() && line[i].is_whitespace() {
            i += 1;
        }
        while i < line.len() && !line[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Ctrl+← / Ctrl+→: jump to the previous/next word (across soft wraps).
    pub fn move_word_left(&mut self) {
        if self.col == 0 && self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
        self.col = Self::word_boundary_left(&self.lines[self.row], self.col);
    }

    pub fn move_word_right(&mut self) {
        if self.col >= self.lines[self.row].len() && self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
        self.col = Self::word_boundary_right(&self.lines[self.row], self.col);
    }

    /// Ctrl+W / Alt+Backspace: delete the word before the cursor.
    pub fn delete_word(&mut self) {
        if self.col == 0 {
            if self.row == 0 {
                return;
            }
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(current);
            self.killed = "\n".into();
            return;
        }
        let start = Self::word_boundary_left(&self.lines[self.row], self.col);
        self.killed = self.lines[self.row].drain(start..self.col).collect();
        self.col = start;
    }

    /// Local editing shared by the message and pending-input composers. Application
    /// shortcuts and submission stay with the owner, which calls this after routing them.
    pub fn edit_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let control = key.modifiers == KeyModifiers::CONTROL;
        let alt = key.modifiers == KeyModifiers::ALT;
        match key.code {
            KeyCode::Left if control || alt => self.move_word_left(),
            KeyCode::Right if control || alt => self.move_word_right(),
            KeyCode::Backspace if alt => self.delete_word(),
            KeyCode::Char('w') if control => self.delete_word(),
            KeyCode::Char('b') if control => self.move_left(),
            KeyCode::Char('f') if control => self.move_right(),
            KeyCode::Char('u') if control => {
                if self.col > 0 {
                    self.killed = self.lines[self.row].drain(..self.col).collect();
                    self.col = 0;
                }
            }
            KeyCode::Char('k') if control => {
                if self.col < self.lines[self.row].len() {
                    self.killed = self.lines[self.row].drain(self.col..).collect();
                } else if self.row + 1 < self.lines.len() {
                    self.delete();
                    self.killed = "\n".into();
                }
            }
            KeyCode::Char('y') if control => {
                for ch in self.killed.clone().chars() {
                    if ch == '\n' {
                        self.insert_newline();
                    } else {
                        self.insert_char(ch);
                    }
                }
            }
            KeyCode::Enter if key.modifiers == KeyModifiers::SHIFT => self.insert_newline(),
            KeyCode::Left if key.modifiers.is_empty() => self.move_left(),
            KeyCode::Right if key.modifiers.is_empty() => self.move_right(),
            KeyCode::Up if key.modifiers.is_empty() => self.move_up(),
            KeyCode::Down if key.modifiers.is_empty() => self.move_down(),
            KeyCode::Home if key.modifiers.is_empty() => self.move_home(),
            KeyCode::End if key.modifiers.is_empty() => self.move_end(),
            KeyCode::Backspace if key.modifiers.is_empty() => self.backspace(),
            KeyCode::Delete if key.modifiers.is_empty() => self.delete(),
            KeyCode::Char(ch)
                if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
                    && !ch.is_control() =>
            {
                self.insert_char(ch)
            }
            _ => return false,
        }
        true
    }

    pub fn submit(&mut self) -> Option<String> {
        let text = self.text().trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.clear();
        Some(text)
    }

    /// record_submission: skip adjacent duplicates, cap, index to end.
    pub fn record_submission(&mut self, text: &str) {
        if !text.is_empty() && self.history.last().map(|s| s.as_str()) != Some(text) {
            self.history.push(text.to_string());
            if self.history.len() > HISTORY_LIMIT {
                let extra = self.history.len() - HISTORY_LIMIT;
                self.history.drain(..extra);
            }
        }
        self.hist_idx = self.history.len();
    }

    /// recall(direction): -1 older, +1 newer; draft restored past the newest.
    pub fn recall(&mut self, direction: i64) {
        if self.history.is_empty() {
            return;
        }
        if self.hist_idx == self.history.len() {
            self.draft = self.text();
        }
        let next = (self.hist_idx as i64 + direction).clamp(0, self.history.len() as i64) as usize;
        self.hist_idx = next;
        let text =
            if self.hist_idx == self.history.len() { self.draft.clone() } else { self.history[self.hist_idx].clone() };
        self.set_text(&text);
    }

    /// Soft-wrap by terminal cells and locate the caret in those same rows.
    /// A full row at the caret gets a continuation row so the cursor never
    /// disappears off the right edge. Combining marks stay with their base.
    pub fn visual_lines(&self, width: usize) -> (Vec<String>, usize, usize) {
        let width = width.max(1);
        let mut rows = vec![String::new()];
        let mut x = 0;
        let mut cursor = (0, 0);
        for (row, line) in self.lines.iter().enumerate() {
            if row > 0 {
                rows.push(String::new());
                x = 0;
            }
            for (col, ch) in line.iter().enumerate() {
                let cells = UnicodeWidthChar::width(*ch).unwrap_or(0);
                if cells > 0 && x + cells.min(width) > width {
                    rows.push(String::new());
                    x = 0;
                }
                if row == self.row && col == self.col {
                    cursor = (rows.len() - 1, x);
                }
                // A double-cell glyph cannot fit a one-cell terminal; its
                // replacement is display-only, never a change to the draft.
                rows.last_mut().unwrap().push(if cells > width { '\u{fffd}' } else { *ch });
                x += cells.min(width);
            }
            if row == self.row && self.col >= line.len() {
                if x >= width {
                    rows.push(String::new());
                    x = 0;
                }
                cursor = (rows.len() - 1, x);
            }
        }
        (rows, cursor.0, cursor.1.min(width - 1))
    }

    /// Wrapped visual height at a text width, including the two border rows.
    pub fn widget_height(&self, width: usize) -> usize {
        (self.visual_lines(width).0.len() + 2).clamp(3, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_multiline() {
        let mut c = Composer::new(vec![]);
        for ch in "hi".chars() {
            c.insert_char(ch);
        }
        c.insert_newline();
        for ch in "yo".chars() {
            c.insert_char(ch);
        }
        assert_eq!(c.text(), "hi\nyo");
        c.backspace();
        assert_eq!(c.text(), "hi\ny");
        c.move_up();
        c.move_end();
        c.delete();
        assert_eq!(c.text(), "hiy");
        assert_eq!(c.submit().as_deref(), Some("hiy"));
        assert_eq!(c.text(), "");
        assert_eq!(Composer::new(vec![]).submit(), None);
    }

    #[test]
    fn history_recall_restores_draft_and_skips_dupes() {
        let mut c = Composer::new(vec!["a".into(), "b".into()]);
        c.record_submission("b"); // adjacent dupe: not appended
        assert_eq!(c.history, ["a", "b"]);
        c.set_text("draft");
        c.recall(-1);
        assert_eq!(c.text(), "b");
        c.recall(-1);
        assert_eq!(c.text(), "a");
        c.recall(1);
        c.recall(1);
        assert_eq!(c.text(), "draft");
    }

    #[test]
    fn shared_editor_deletes_and_restores_unicode_text_at_the_cursor() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut c = Composer::new(vec![]);
        c.set_text("one \u{754c}\u{9762}\nnext");
        c.move_up();
        c.move_end();
        let control = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL);
        c.edit_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(c.text(), "one \nnext");
        c.edit_key(control('y'));
        assert_eq!(c.text(), "one \u{754c}\u{9762}\nnext");
        c.edit_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(c.col, 4);
        c.edit_key(control('k'));
        c.edit_key(control('y'));
        assert_eq!(c.text(), "one \u{754c}\u{9762}\nnext");
        c.edit_key(control('u'));
        assert_eq!(c.text(), "\nnext");
        c.edit_key(control('y'));
        assert_eq!(c.text(), "one \u{754c}\u{9762}\nnext");
        c.edit_key(control('k'));
        assert_eq!(c.text(), "one \u{754c}\u{9762}next");
        c.edit_key(control('y'));
        assert_eq!(c.text(), "one \u{754c}\u{9762}\nnext");
        c.edit_key(control('b'));
        c.edit_key(control('f'));
        assert_eq!((c.row, c.col), (1, 0));
        assert!(!c.edit_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)));
        let before = c.text();
        let mut release = control('u');
        release.kind = crossterm::event::KeyEventKind::Release;
        assert!(!c.edit_key(release));
        assert_eq!(c.text(), before);
    }

    #[test]
    fn widget_height_bounds() {
        let mut c = Composer::new(vec![]);
        assert_eq!(c.widget_height(20), 3);
        c.set_text(&"x".repeat(100));
        assert_eq!(c.widget_height(20), 8, "the caret needs a row after five full rows");
        c.set_text(&"x".repeat(500));
        assert_eq!(c.widget_height(20), 8);
    }
}
