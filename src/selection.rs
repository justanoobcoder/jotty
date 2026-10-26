//! Mouse text selection.
//!
//! Points use "screen lines": 0 is the top row of the live screen, negative
//! lines are in the scrollback history. Scrolling the view therefore does
//! not move a selection; new output clears it.

use crate::terminal::{Cell, Flags, Screen};

/// Characters that end a word for double-click selection, besides
/// whitespace. Paths, URLs and options stay in one piece.
const WORD_DELIMITERS: &str = "\"'`()[]{}<>|,;";

fn is_word(cell: &Cell) -> bool {
    // The spacer of a wide character belongs to that character.
    cell.flags.contains(Flags::WIDE_SPACER)
        || !(cell.c.is_whitespace() || WORD_DELIMITERS.contains(cell.c))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub line: isize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    anchor: Point,
    head: Point,
    /// Made from a double click rather than a drag, so it covers at least
    /// one cell even when `anchor == head`.
    expanded: bool,
}

impl Selection {
    pub fn new(at: Point) -> Self {
        Selection {
            anchor: at,
            head: at,
            expanded: false,
        }
    }

    /// A selection of `start..=end` that is never empty.
    pub fn span(start: Point, end: Point) -> Self {
        Selection {
            anchor: start,
            head: end,
            expanded: true,
        }
    }

    /// The word under `at`, following soft-wrapped lines, or `None` on
    /// whitespace and delimiters.
    pub fn word(screen: &Screen, at: Point) -> Option<Self> {
        let word_at = |p: Point| {
            screen
                .row_at(p.line)
                .and_then(|r| r.cells.get(p.col))
                .is_some_and(is_word)
        };
        if !word_at(at) {
            return None;
        }
        let mut start = at;
        loop {
            let prev = if start.col > 0 {
                Point {
                    line: start.line,
                    col: start.col - 1,
                }
            } else {
                match screen.row_at(start.line - 1) {
                    Some(row) if row.wrapped => Point {
                        line: start.line - 1,
                        col: row.cells.len().saturating_sub(1),
                    },
                    _ => break,
                }
            };
            if !word_at(prev) {
                break;
            }
            start = prev;
        }
        let mut end = at;
        loop {
            let row = screen.row_at(end.line)?;
            let next = if end.col + 1 < row.cells.len() {
                Point {
                    line: end.line,
                    col: end.col + 1,
                }
            } else if row.wrapped {
                Point {
                    line: end.line + 1,
                    col: 0,
                }
            } else {
                break;
            };
            if !word_at(next) {
                break;
            }
            end = next;
        }
        Some(Selection::span(start, end))
    }

    pub fn update(&mut self, to: Point) {
        self.head = to;
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head && !self.expanded
    }

    /// Start and end, inclusive, in reading order.
    pub fn range(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        let (start, end) = self.range();
        !self.is_empty() && start <= p && p <= end
    }

    pub fn text(&self, screen: &Screen) -> String {
        let (start, end) = self.range();
        let mut out = String::new();
        for line in start.line..=end.line {
            let Some(row) = screen.row_at(line) else {
                continue;
            };
            let first = if line == start.line { start.col } else { 0 };
            let last = if line == end.line {
                end.col + 1
            } else {
                row.cells.len()
            };
            let mut text: String = row
                .cells
                .iter()
                .take(last)
                .skip(first)
                .filter(|c| !c.flags.contains(Flags::WIDE_SPACER))
                .map(|c| c.c)
                .collect();
            let joined = row.wrapped && line != end.line;
            if !joined {
                text.truncate(text.trim_end_matches(' ').len());
            }
            out.push_str(&text);
            if !joined && line != end.line {
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn p(line: isize, col: usize) -> Point {
        Point { line, col }
    }

    #[test]
    fn range_is_ordered() {
        let mut s = Selection::new(p(2, 5));
        s.update(p(1, 3));
        assert_eq!(s.range(), (p(1, 3), p(2, 5)));
        assert!(s.contains(p(1, 80)));
        assert!(s.contains(p(2, 0)));
        assert!(!s.contains(p(2, 6)));
        assert!(!s.contains(p(1, 2)));
    }

    #[test]
    fn empty_selection_contains_nothing() {
        let s = Selection::new(p(0, 0));
        assert!(s.is_empty());
        assert!(!s.contains(p(0, 0)));
    }

    #[test]
    fn text_across_lines() {
        let mut t = Terminal::new(10, 3, 10);
        t.feed(b"hello   \r\nworld");
        let mut s = Selection::new(p(0, 2));
        s.update(p(1, 2));
        assert_eq!(s.text(t.screen()), "llo\nwor");
    }

    #[test]
    fn wrapped_lines_are_joined() {
        let mut t = Terminal::new(4, 3, 10);
        t.feed(b"abcdefg");
        let mut s = Selection::new(p(0, 0));
        s.update(p(1, 3));
        assert_eq!(s.text(t.screen()), "abcdefg");
    }

    #[test]
    fn text_from_history() {
        let mut t = Terminal::new(4, 1, 10);
        t.feed(b"one\r\ntwo");
        let mut s = Selection::new(p(-1, 0));
        s.update(p(0, 3));
        assert_eq!(s.text(t.screen()), "one\ntwo");
    }

    fn word_text(t: &Terminal, at: Point) -> Option<String> {
        Selection::word(t.screen(), at).map(|s| s.text(t.screen()))
    }

    #[test]
    fn word_under_point() {
        let mut t = Terminal::new(40, 1, 10);
        t.feed(b"ls -la ~/src/foo.rs (x)");
        assert_eq!(word_text(&t, p(0, 1)).as_deref(), Some("ls"));
        assert_eq!(word_text(&t, p(0, 4)).as_deref(), Some("-la"));
        assert_eq!(word_text(&t, p(0, 10)).as_deref(), Some("~/src/foo.rs"));
        assert_eq!(word_text(&t, p(0, 21)).as_deref(), Some("x"));
        // Whitespace, delimiters and cells past the text.
        assert_eq!(word_text(&t, p(0, 2)), None);
        assert_eq!(word_text(&t, p(0, 20)), None);
        assert_eq!(word_text(&t, p(0, 30)), None);
    }

    #[test]
    fn single_character_word_is_not_empty() {
        let mut t = Terminal::new(10, 1, 10);
        t.feed(b"a b");
        let s = Selection::word(t.screen(), p(0, 0)).unwrap();
        assert!(!s.is_empty());
        assert!(s.contains(p(0, 0)));
        assert_eq!(s.text(t.screen()), "a");
    }

    #[test]
    fn word_across_wrapped_lines() {
        let mut t = Terminal::new(4, 3, 10);
        t.feed(b"x abcdef y");
        assert_eq!(word_text(&t, p(0, 3)).as_deref(), Some("abcdef"));
        assert_eq!(word_text(&t, p(1, 2)).as_deref(), Some("abcdef"));
    }

    #[test]
    fn word_with_wide_characters() {
        let mut t = Terminal::new(10, 1, 10);
        t.feed("a中文b c".as_bytes());
        assert_eq!(word_text(&t, p(0, 2)).as_deref(), Some("a中文b"));
    }

    #[test]
    fn wide_characters() {
        let mut t = Terminal::new(10, 1, 10);
        t.feed("a中b".as_bytes());
        let mut s = Selection::new(p(0, 0));
        s.update(p(0, 3));
        assert_eq!(s.text(t.screen()), "a中b");
    }
}
