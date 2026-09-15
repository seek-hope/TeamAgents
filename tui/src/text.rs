//! Composer buffer.
//! Enter sends, Shift+Enter/Ctrl+J inserts a newline, ↑↓ recall history at the
//! first/last row. Lines are Vec<char> so multibyte editing stays simple.

use unicode_width::UnicodeWidthStr;

pub struct Composer {
    pub lines: Vec<Vec<char>>,
    pub row: usize,
    pub col: usize,
    pub history: Vec<String>,
    pub hist_idx: usize,
    pub draft: String,
}

impl Composer {
    pub fn new(history: Vec<String>) -> Composer {
        let hist_idx = history.len();
        Composer { lines: vec![vec![]], row: 0, col: 0, history, hist_idx, draft: String::new() }
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
            return;
        }
        let start = Self::word_boundary_left(&self.lines[self.row], self.col);
        self.lines[self.row].drain(start..self.col);
        self.col = start;
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
            if self.history.len() > crate::i18n::HISTORY_LIMIT {
                let extra = self.history.len() - crate::i18n::HISTORY_LIMIT;
                self.history.drain(..extra);
            }
        }
        self.hist_idx = self.history.len();
    }

    pub fn clear_composer(&mut self) {
        self.draft.clear();
        self.clear();
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
        let text = if self.hist_idx == self.history.len() {
            self.draft.clone()
        } else {
            self.history[self.hist_idx].clone()
        };
        self.set_text(&text);
    }

    /// Wrapped visual height at a text width → widget height (3..=8).
    pub fn widget_height(&self, width: usize) -> usize {
        let wrapped = self
            .lines
            .iter()
            .map(|l| {
                let w = UnicodeWidthStr::width(l.iter().collect::<String>().as_str());
                (w.max(1) + width - 1) / width.max(1)
            })
            .sum::<usize>()
            .max(1);
        (wrapped + 2).clamp(3, 8)
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
    fn widget_height_bounds() {
        let mut c = Composer::new(vec![]);
        assert_eq!(c.widget_height(20), 3);
        c.set_text(&"x".repeat(100));
        assert_eq!(c.widget_height(20), 7);
        c.set_text(&"x".repeat(500));
        assert_eq!(c.widget_height(20), 8);
    }
}
