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

/// First and last cell of the word under `at`, following soft-wrapped
/// lines, or `None` on whitespace and delimiters.
fn word_bounds(screen: &Screen, at: Point) -> Option<(Point, Point)> {
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
    Some((start, end))
}

/// First and last cell of the line under `at`, including the rows it
/// soft-wraps onto.
fn line_bounds(screen: &Screen, at: Point) -> (Point, Point) {
    let mut first = at.line;
    while screen.row_at(first - 1).is_some_and(|r| r.wrapped) {
        first -= 1;
    }
    let mut last = at.line;
    while screen.row_at(last).is_some_and(|r| r.wrapped) {
        last += 1;
    }
    let end = screen
        .row_at(last)
        .map_or(at.col, |r| r.cells.len().saturating_sub(1));
    (
        Point {
            line: first,
            col: 0,
        },
        Point {
            line: last,
            col: end,
        },
    )
}

/// What a selection grows by while dragging: cells after a single click,
/// words after a double click, lines after a triple click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Cell,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    anchor: Point,
    head: Point,
    unit: Unit,
    /// What the click itself selected (a word, a line, or command output);
    /// dragging extends from it and never shrinks below it.
    origin: (Point, Point),
}

impl Selection {
    pub fn new(at: Point) -> Self {
        Selection {
            anchor: at,
            head: at,
            unit: Unit::Cell,
            origin: (at, at),
        }
    }

    /// A selection of `start..=end` that is never empty and, when dragged,
    /// grows by `unit`.
    pub fn span(start: Point, end: Point, unit: Unit) -> Self {
        Selection {
            anchor: start,
            head: end,
            unit,
            origin: (start, end),
        }
    }

    /// The word under `at`, following soft-wrapped lines, or `None` on
    /// whitespace and delimiters.
    pub fn word(screen: &Screen, at: Point) -> Option<Self> {
        let (start, end) = word_bounds(screen, at)?;
        Some(Selection::span(start, end, Unit::Word))
    }

    /// The whole line under `at`, including the rows it soft-wraps onto.
    pub fn line(screen: &Screen, at: Point) -> Self {
        let (start, end) = line_bounds(screen, at);
        Selection::span(start, end, Unit::Line)
    }

    pub fn update(&mut self, to: Point) {
        self.head = to;
    }

    /// Extends the selection to `to` while dragging, by whole words or
    /// lines if it was started by a double or triple click.
    pub fn extend(&mut self, screen: &Screen, to: Point) {
        let (start, end) = match self.unit {
            Unit::Cell => return self.update(to),
            Unit::Word => word_bounds(screen, to).unwrap_or((to, to)),
            Unit::Line => line_bounds(screen, to),
        };
        let (origin_start, origin_end) = self.origin;
        if start < origin_start {
            self.anchor = origin_end;
            self.head = start;
        } else {
            self.anchor = origin_start;
            self.head = end.max(origin_end);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head && self.unit == Unit::Cell
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
    fn line_includes_wrapped_rows() {
        let mut t = Terminal::new(4, 4, 10);
        t.feed(b"one\r\nabcdefg\r\nz");
        let s = Selection::line(t.screen(), p(2, 1));
        assert_eq!(s.range(), (p(1, 0), p(2, 3)));
        assert_eq!(s.text(t.screen()), "abcdefg");
        assert_eq!(Selection::line(t.screen(), p(0, 3)).text(t.screen()), "one");
    }

    #[test]
    fn dragging_extends_by_words() {
        let mut t = Terminal::new(40, 2, 10);
        t.feed(b"alpha beta gamma\r\ndelta");
        let screen = t.screen();
        let mut s = Selection::word(screen, p(0, 7)).unwrap();
        s.extend(screen, p(0, 12));
        assert_eq!(s.text(screen), "beta gamma");
        s.extend(screen, p(0, 1));
        assert_eq!(s.text(screen), "alpha beta");
        // Dragging back into the clicked word keeps just that word.
        s.extend(screen, p(0, 8));
        assert_eq!(s.text(screen), "beta");
        s.extend(screen, p(1, 2));
        assert_eq!(s.text(screen), "beta gamma\ndelta");
    }

    #[test]
    fn dragging_extends_by_lines() {
        let mut t = Terminal::new(10, 3, 10);
        t.feed(b"one\r\ntwo\r\nthree");
        let screen = t.screen();
        let mut s = Selection::line(screen, p(1, 1));
        s.extend(screen, p(2, 0));
        assert_eq!(s.text(screen), "two\nthree");
        s.extend(screen, p(0, 5));
        assert_eq!(s.text(screen), "one\ntwo");
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
