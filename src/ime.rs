//! Input-method support (text-input-unstable-v3), protocol-independent part.
//!
//! A terminal does not own the text being edited — the shell does — so the
//! "surrounding text" reported to the input method is an approximation: the
//! cursor row, starting after the prompt when the shell marks it with
//! OSC 133;B. Deleting surrounding text is translated into Backspace and
//! Delete key presses sent to the child.

use crate::terminal::{Flags, Row};

/// The protocol limits a request to 4096 bytes; stay well below it.
const MAX_SURROUNDING_BYTES: usize = 4000;

/// The text around the cursor, as the input method is told it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Surrounding {
    pub text: String,
    /// Byte offset of the cursor in `text`.
    pub cursor: usize,
}

impl Surrounding {
    /// Builds the surrounding text from columns `start..` of `row`, with the
    /// cursor at column `cursor_col`.
    #[must_use]
    pub fn from_row(row: &Row, start: usize, cursor_col: usize) -> Surrounding {
        let start = start.min(cursor_col);
        let mut text = String::new();
        let mut cursor = 0;
        for (col, cell) in row.cells().iter().enumerate().skip(start) {
            if col == cursor_col {
                cursor = text.len();
            }
            // One character per cluster: Backspace deletes a cluster at a
            // time, so `delete_keys` must count them, not their parts.
            if !cell
                .flags
                .intersects(Flags::WIDE_SPACER | Flags::WRAP_GAP | Flags::CONTINUATION)
            {
                text.push(cell.c);
            }
        }
        if cursor_col >= row.cells().len() {
            cursor = text.len();
        }
        // Trailing blanks are just empty cells, unless they are before the cursor.
        let trimmed = text.trim_end_matches(' ').len().max(cursor);
        text.truncate(trimmed);
        let mut s = Surrounding { text, cursor };
        s.limit(MAX_SURROUNDING_BYTES);
        s
    }

    /// `text` with the cursor at its end, as for a query being typed.
    #[must_use]
    pub fn at_end(text: &str) -> Surrounding {
        let mut s = Surrounding {
            text: text.to_owned(),
            cursor: text.len(),
        };
        s.limit(MAX_SURROUNDING_BYTES);
        s
    }

    /// Trims text far from the cursor until it fits in `max` bytes.
    fn limit(&mut self, max: usize) {
        if self.text.len() <= max {
            return;
        }
        let mut end = self
            .text
            .len()
            .min(self.cursor + max / 2)
            .max(max.min(self.text.len()));
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.truncate(end);
        let mut start = self.text.len().saturating_sub(max);
        while !self.text.is_char_boundary(start) {
            start += 1;
        }
        self.text.drain(..start);
        self.cursor -= start;
    }

    /// Key presses that delete `before` bytes before and `after` bytes after
    /// the cursor, as requested by the input method.
    #[must_use]
    pub fn delete_keys(&self, before: usize, after: usize) -> Vec<u8> {
        let floor = |mut i: usize| {
            while !self.text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let start = floor(self.cursor.saturating_sub(before));
        let end = floor((self.cursor + after).min(self.text.len()));
        let backspaces = self.text[start..self.cursor].chars().count();
        let deletes = self.text[self.cursor..end].chars().count();
        let mut out = vec![0x7f; backspaces];
        for _ in 0..deletes {
            out.extend_from_slice(b"\x1b[3~");
        }
        out
    }
}

/// Text the input method is composing, shown at the cursor until it is
/// committed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preedit {
    pub text: String,
    /// Byte range of the cursor/selection inside `text`, if the IME sent one.
    pub cursor: Option<(usize, usize)>,
}

/// What the input method is told the focused text is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Content {
    /// A shell or program reading ordinary text.
    #[default]
    Terminal,
    /// A password prompt: keys should pass through as typed.
    Password,
}

impl Content {
    /// `password_prompt` is the pty's state; `own_input` is set while
    /// jotty itself takes the keys (search bar, vi mode), which is never a
    /// password field whatever the program behind it is doing.
    #[must_use]
    pub fn new(password_prompt: bool, own_input: bool) -> Content {
        if password_prompt && !own_input {
            Content::Password
        } else {
            Content::Terminal
        }
    }
}

/// Double-buffered text-input state: events accumulate until `done`.
#[derive(Debug, Default)]
pub struct ImeState {
    pub active: bool,
    /// The preedit currently shown at the cursor.
    pub preedit: Option<Preedit>,
    /// Surrounding text last sent to the input method.
    pub surrounding: Surrounding,
    /// Content type last sent to the input method.
    pub content: Content,
    pending_preedit: Option<Preedit>,
    pending_commit: Option<String>,
    pending_delete: Option<(usize, usize)>,
}

impl ImeState {
    /// Queues the preedit string for the next `done`. `begin` and `end` are
    /// byte offsets of the cursor in `text`; negative hides the cursor.
    pub fn set_preedit(&mut self, text: Option<String>, begin: i32, end: i32) {
        self.pending_preedit = text.filter(|t| !t.is_empty()).map(|text| {
            let cursor = (begin >= 0 && end >= 0).then_some((begin as usize, end as usize));
            Preedit { text, cursor }
        });
    }

    /// Queues text to send to the child on the next `done`.
    pub fn set_commit(&mut self, text: Option<String>) {
        self.pending_commit = text;
    }

    /// Queues the deletion of `before` and `after` bytes around the cursor
    /// for the next `done`.
    pub fn set_delete(&mut self, before: u32, after: u32) {
        self.pending_delete = Some((before as usize, after as usize));
    }

    /// Applies pending events (the `done` event). Returns the bytes to send
    /// to the child. Per the protocol: delete, then commit, then preedit.
    pub fn done(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some((before, after)) = self.pending_delete.take() {
            out.extend(self.surrounding.delete_keys(before, after));
        }
        if let Some(text) = self.pending_commit.take() {
            out.extend_from_slice(text.as_bytes());
        }
        self.preedit = self.pending_preedit.take();
        out
    }

    /// Applies pending events to text jotty edits itself, the search query,
    /// whose end the input method was told is the cursor (`at_end`): the
    /// deletion shortens it and the committed text is appended.
    pub fn done_into(&mut self, text: &mut String) {
        if let Some((before, _)) = self.pending_delete.take() {
            let mut start = text.len().saturating_sub(before);
            while !text.is_char_boundary(start) {
                start -= 1;
            }
            text.truncate(start);
        }
        if let Some(commit) = self.pending_commit.take() {
            text.extend(commit.chars().filter(|c| !c.is_control()));
        }
        self.preedit = self.pending_preedit.take();
    }

    /// Forgets the preedit and every pending event, when the input method
    /// leaves the window.
    pub fn reset(&mut self) {
        self.active = false;
        self.preedit = None;
        self.pending_preedit = None;
        self.pending_commit = None;
        self.pending_delete = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn row_of(s: &str, cols: usize) -> Row {
        let mut t = Terminal::new(cols, 1, 0);
        t.feed(s.as_bytes());
        t.screen().grid().rows[0].clone()
    }

    #[test]
    fn surrounding_from_prompt() {
        let row = row_of("$ echo hi", 20);
        let s = Surrounding::from_row(&row, 2, 9);
        assert_eq!(s.text, "echo hi");
        assert_eq!(s.cursor, 7);

        let s = Surrounding::from_row(&row, 2, 4);
        assert_eq!(s.text, "echo hi");
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn surrounding_keeps_spaces_before_cursor() {
        let row = row_of("ab  ", 10);
        let s = Surrounding::from_row(&row, 0, 4);
        assert_eq!(s.text, "ab  ");
        assert_eq!(s.cursor, 4);
    }

    #[test]
    fn surrounding_with_wide_and_multibyte_chars() {
        let row = row_of("日本é", 10);
        // Columns: 日=0,1 本=2,3 é=4.
        let s = Surrounding::from_row(&row, 0, 4);
        assert_eq!(s.text, "日本é");
        assert_eq!(s.cursor, "日本".len());
    }

    #[test]
    fn surrounding_start_after_cursor_is_clamped() {
        let row = row_of("abc", 10);
        let s = Surrounding::from_row(&row, 8, 1);
        assert_eq!(s.text, "bc");
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn limit_keeps_cursor_region() {
        let mut s = Surrounding {
            text: format!("{}é{}", "a".repeat(5000), "b".repeat(5000)),
            cursor: 5000,
        };
        s.limit(100);
        assert!(s.text.len() <= 100);
        assert_eq!(&s.text[s.cursor..s.cursor + 2], "é");
    }

    #[test]
    fn delete_keys_count_characters() {
        let s = Surrounding {
            text: "naïve x".into(),
            cursor: "naïve".len(),
        };
        // "ïve" is 4 bytes but 3 characters.
        assert_eq!(s.delete_keys(4, 0), vec![0x7f; 3]);
        assert_eq!(s.delete_keys(0, 2), b"\x1b[3~\x1b[3~");
        // Out of range requests are clamped.
        assert_eq!(s.delete_keys(100, 0).len(), 5);
    }

    #[test]
    fn done_applies_in_order() {
        let mut ime = ImeState {
            surrounding: Surrounding {
                text: "nihao".into(),
                cursor: 5,
            },
            ..ImeState::default()
        };
        ime.set_delete(5, 0);
        ime.set_commit(Some("你好".into()));
        ime.set_preedit(Some("x".into()), 1, 1);
        let out = ime.done();
        let mut expected = vec![0x7f; 5];
        expected.extend_from_slice("你好".as_bytes());
        assert_eq!(out, expected);
        assert_eq!(
            ime.preedit,
            Some(Preedit {
                text: "x".into(),
                cursor: Some((1, 1))
            })
        );

        // A done without new events clears the preedit.
        assert!(ime.done().is_empty());
        assert_eq!(ime.preedit, None);
    }

    #[test]
    fn done_into_edits_the_query_instead_of_sending_keys() {
        let mut ime = ImeState {
            surrounding: Surrounding::at_end("vi\u{ea}"),
            ..ImeState::default()
        };
        let mut query = String::from("vi\u{ea}");
        // Recomposing the syllable: drop "i\u{ea}" (3 bytes), commit "i\u{1ec7}t".
        ime.set_delete(3, 0);
        ime.set_commit(Some("i\u{1ec7}t\x07".into()));
        ime.done_into(&mut query);
        assert_eq!(query, "vi\u{1ec7}t");
        // A deletion that cuts into a character takes the whole character.
        ime.set_delete(1, 0);
        ime.done_into(&mut query);
        assert_eq!(query, "vi\u{1ec7}");
    }

    #[test]
    fn password_prompt_is_password_content() {
        assert_eq!(Content::new(true, false), Content::Password);
        assert_eq!(Content::new(false, false), Content::Terminal);
    }

    #[test]
    fn search_bar_and_vi_mode_are_never_password_content() {
        assert_eq!(Content::new(true, true), Content::Terminal);
        assert_eq!(Content::new(false, true), Content::Terminal);
    }

    #[test]
    fn hidden_preedit_cursor() {
        let mut ime = ImeState::default();
        ime.set_preedit(Some("ka".into()), -1, -1);
        ime.done();
        assert_eq!(ime.preedit.unwrap().cursor, None);
    }
}
