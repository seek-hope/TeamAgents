//! UTF-8 and CJK-aware text composer input plus complementary slash-command palette.
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
    pub add_space: bool,
}

impl SlashSpec {
    pub fn completion(self) -> String {
        if self.add_space {
            format!("{} ", self.name)
        } else {
            self.name.to_owned()
        }
    }
}

/// Configuration, parameterized, and workflow commands only (non-overlapping with keybindings).
pub(crate) const SLASH_COMMANDS: &[SlashSpec] = &[
    SlashSpec {
        name: "/model",
        usage: "/model [MODEL [EFFORT]|effort E|default]",
        summary: "Pick or set model and reasoning effort",
        add_space: false,
    },
    SlashSpec { name: "/to", usage: "/to [NAME]", summary: "Pick or set target recipient agent", add_space: false },
    SlashSpec {
        name: "/check",
        usage: "/check [COMMAND|off]",
        summary: "Edit, arm, or clear authorized --check command",
        add_space: false,
    },
    SlashSpec {
        name: "/review",
        usage: "/review [on|off]",
        summary: "Toggle or set per-operation --review policy",
        add_space: false,
    },
    SlashSpec {
        name: "/new",
        usage: "/new TEXT",
        summary: "Submit a new root task while clarification waits",
        add_space: true,
    },
    SlashSpec {
        name: "/answer",
        usage: "/answer ROOT_ID TEXT",
        summary: "Reply to a waiting clarification root",
        add_space: true,
    },
    SlashSpec { name: "/clear", usage: "/clear", summary: "Clear session event feed and composer", add_space: false },
    SlashSpec { name: "/stop", usage: "/stop", summary: "Stop an idle daemon (session stays saved)", add_space: false },
];

pub(crate) fn matching_commands(input: &str) -> Vec<&'static SlashSpec> {
    if !input.starts_with('/') || input.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    SLASH_COMMANDS.iter().filter(|spec| spec.name.starts_with(input)).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextInput {
    buf: String,
    cursor: usize,
    history: Vec<String>,
    history_idx: Option<usize>,
    draft: String,
}

impl TextInput {
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn set_text(&mut self, text: &str) {
        self.buf = normalize_paste(text);
        self.cursor = self.buf.len();
        self.history_idx = None;
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.history_idx = None;
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch == '\t' {
            self.insert_str("    ");
            return;
        }
        let ch = if ch == '\r' { '\n' } else { ch };
        if ch.is_control() && ch != '\n' {
            return;
        }
        self.buf.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.history_idx = None;
    }

    pub fn insert_str(&mut self, text: &str) {
        let clean = normalize_paste(text);
        if clean.is_empty() {
            return;
        }
        self.buf.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
        self.history_idx = None;
    }

    pub fn backspace(&mut self) {
        if let Some((idx, _)) = self.buf[..self.cursor].char_indices().next_back() {
            self.buf.remove(idx);
            self.cursor = idx;
            self.history_idx = None;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
            self.history_idx = None;
        }
    }

    pub fn move_left(&mut self) {
        if let Some((idx, _)) = self.buf[..self.cursor].char_indices().next_back() {
            self.cursor = idx;
        }
    }

    pub fn move_right(&mut self) {
        if let Some(ch) = self.buf[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = self.buf[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }

    pub fn move_end(&mut self) {
        self.cursor = self.buf[self.cursor..].find('\n').map_or(self.buf.len(), |i| self.cursor + i);
    }

    pub fn move_word_left(&mut self) {
        let mut idx = self.cursor;
        let before = &self.buf[..idx];
        let mut chars = before.char_indices().rev().peekable();
        while let Some(&(_, c)) = chars.peek() {
            if c.is_whitespace() {
                idx = chars.next().unwrap().0;
            } else {
                break;
            }
        }
        for (pos, c) in chars {
            if c.is_whitespace() {
                break;
            }
            idx = pos;
        }
        self.cursor = idx;
    }

    pub fn move_word_right(&mut self) {
        let mut idx = self.cursor;
        let after = &self.buf[idx..];
        let mut chars = after.char_indices().peekable();
        while let Some(&(_, c)) = chars.peek() {
            if !c.is_whitespace() {
                idx = self.cursor + chars.next().unwrap().0 + c.len_utf8();
            } else {
                break;
            }
        }
        for (pos, c) in chars {
            if !c.is_whitespace() {
                break;
            }
            idx = self.cursor + pos + c.len_utf8();
        }
        self.cursor = idx;
    }

    pub fn delete_word_back(&mut self) {
        let end = self.cursor;
        self.move_word_left();
        if self.cursor < end {
            self.buf.drain(self.cursor..end);
            self.history_idx = None;
        }
    }

    pub fn clear_to_start(&mut self) {
        let line_start = self.buf[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        if line_start < self.cursor {
            self.buf.drain(line_start..self.cursor);
            self.cursor = line_start;
            self.history_idx = None;
        }
    }

    pub fn take_and_push_history(&mut self) -> String {
        let text = std::mem::take(&mut self.buf);
        self.cursor = 0;
        self.history_idx = None;
        self.draft.clear();
        if !text.trim().is_empty() && self.history.last().is_none_or(|last| last != &text) {
            self.history.push(text.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        text
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_idx {
            None => {
                self.draft = self.buf.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(next);
        self.buf = self.history[next].clone();
        self.cursor = self.buf.len();
    }

    pub fn history_next(&mut self) {
        let Some(idx) = self.history_idx else { return };
        if idx + 1 < self.history.len() {
            let next = idx + 1;
            self.history_idx = Some(next);
            self.buf = self.history[next].clone();
        } else {
            self.history_idx = None;
            self.buf = std::mem::take(&mut self.draft);
        }
        self.cursor = self.buf.len();
    }

    /// Wrap lines by exact display width so cursor coordinates match rendered cells even for CJK.
    pub fn visual_layout(&self, inner_width: usize) -> (Vec<String>, u16, u16) {
        let max_w = inner_width.max(2);
        let mut lines = Vec::new();
        let mut current = String::new();
        let mut col = 0usize;
        let mut cursor_pos = None;

        for (byte_idx, ch) in self.buf.char_indices() {
            if byte_idx == self.cursor && cursor_pos.is_none() {
                if col >= max_w {
                    lines.push(std::mem::take(&mut current));
                    col = 0;
                }
                cursor_pos = Some((col as u16, lines.len() as u16));
            }
            if ch == '\n' {
                lines.push(std::mem::take(&mut current));
                col = 0;
                continue;
            }
            let w = ch.width().unwrap_or(0);
            if col > 0 && col + w > max_w {
                lines.push(std::mem::take(&mut current));
                col = 0;
            }
            current.push(ch);
            col += w;
        }

        if cursor_pos.is_none() {
            if col >= max_w {
                lines.push(std::mem::take(&mut current));
                col = 0;
            }
            cursor_pos = Some((col as u16, lines.len() as u16));
        }
        lines.push(current);

        let (cx, cy) = cursor_pos.unwrap_or((0, 0));
        (lines, cx, cy)
    }

    /// Place cursor at the character boundary closest to visual `(target_col, target_row)`.
    pub fn set_cursor_visual(&mut self, inner_width: usize, target_col: u16, target_row: u16) {
        let max_w = inner_width.max(2);
        let t_row = target_row as usize;
        let t_col = target_col as usize;
        let mut row = 0usize;
        let mut col = 0usize;
        let mut row_end_byte: Option<usize> = None;

        for (byte_idx, ch) in self.buf.char_indices() {
            if ch == '\n' {
                if row == t_row {
                    self.cursor = byte_idx;
                    return;
                }
                row += 1;
                col = 0;
                row_end_byte = None;
                continue;
            }
            let w = ch.width().unwrap_or(0);
            if col > 0 && col + w > max_w {
                if row == t_row {
                    self.cursor = row_end_byte.unwrap_or(byte_idx);
                    return;
                }
                row += 1;
                col = 0;
                row_end_byte = None;
            }
            if row == t_row {
                if t_col <= col || (w > 0 && t_col < col + w.div_ceil(2)) {
                    self.cursor = byte_idx;
                    return;
                }
                row_end_byte = Some(byte_idx + ch.len_utf8());
            }
            col += w;
        }

        self.cursor = if row == t_row { row_end_byte.unwrap_or(self.buf.len()) } else { self.buf.len() };
    }
}

fn normalize_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_cjk_editing_word_motion_and_exact_cursor_wrapping() {
        let mut input = TextInput::default();
        input.insert_str("修复 bug\r\n测试\x1b[2J");
        assert_eq!(input.as_str(), "修复 bug\n测试[2J");

        let (lines, cx, cy) = input.visual_layout(6);
        assert_eq!(lines[0], "修复 b");
        assert_eq!(lines[1], "ug");
        assert_eq!(lines[2], "测试[2");
        assert_eq!(lines[3], "J");
        assert_eq!((cx, cy), (1, 3));

        // Click visual positions and verify round-trip back to character boundaries.
        input.set_cursor_visual(6, 0, 0);
        assert_eq!(input.visual_layout(6).1..=input.visual_layout(6).1, 0..=0);
        assert_eq!(input.visual_layout(6).2, 0);
        input.set_cursor_visual(6, 2, 0); // start of "复"
        assert_eq!((input.visual_layout(6).1, input.visual_layout(6).2), (2, 0));
        input.set_cursor_visual(6, 20, 1); // past end of wrapped row 1 ("ug") -> before row 2
        assert_eq!((input.visual_layout(6).1, input.visual_layout(6).2), (2, 1));
        input.set_cursor_visual(6, 0, 2); // start of row 2 ("测试[2")
        assert_eq!((input.visual_layout(6).1, input.visual_layout(6).2), (0, 2));

        input.clear();
        input.insert_str("你好abc world");
        input.move_word_left();
        input.delete_word_back();
        assert_eq!(input.as_str(), "world");
    }

    #[test]
    fn tracks_history_and_matches_complementary_slash_commands() {
        let mut input = TextInput::default();
        input.insert_str("first task");
        assert_eq!(input.take_and_push_history(), "first task");
        input.insert_str("draft");
        input.history_prev();
        assert_eq!(input.as_str(), "first task");
        input.history_next();
        assert_eq!(input.as_str(), "draft");

        let matches: Vec<_> = matching_commands("/mo").into_iter().map(|s| s.name).collect();
        assert_eq!(matches, vec!["/model"]);
        assert!(matching_commands("/model gpt-5").is_empty());
        assert_eq!(matching_commands("/").len(), SLASH_COMMANDS.len());
        // Removed overlapping commands and /theme are not in the slash palette.
        assert!(!SLASH_COMMANDS.iter().any(|s| matches!(s.name, "/quit" | "/help" | "/tasks" | "/members" | "/theme")));
    }
}
