//! Mouse text selection.
//!
//! Points use "screen lines": 0 is the top row of the live screen, negative
//! lines are in the scrollback history. Scrolling the view therefore does
//! not move a selection; new output clears it.

use crate::terminal::{Flags, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub line: isize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    anchor: Point,
    head: Point,
}

impl Selection {
    pub fn new(at: Point) -> Self {
        Selection {
            anchor: at,
            head: at,
        }
    }

    pub fn update(&mut self, to: Point) {
        self.head = to;
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
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
            let Some(row) = screen.row_at(line) else { continue };
            let first = if line == start.line { start.col } else { 0 };
            let last = if line == end.line { end.col + 1 } else { row.cells.len() };
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

    #[test]
    fn wide_characters() {
        let mut t = Terminal::new(10, 1, 10);
        t.feed("a中b".as_bytes());
        let mut s = Selection::new(p(0, 0));
        s.update(p(0, 3));
        assert_eq!(s.text(t.screen()), "a中b");
    }
}
