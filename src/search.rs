//! Searching the scrollback and the screen.

use crate::selection::Point;
use crate::terminal::Screen;
use crate::text::LineText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Toward the top of the history.
    Older,
    /// Toward the bottom of the screen.
    Newer,
}

/// Finds `query` from `from` in `direction`: the nearest match that starts
/// before `from` (Older) or after it (Newer), or at it with `inclusive`.
/// Matches may span soft-wrapped rows. The search ignores case unless the
/// query has an uppercase letter. Returns the first and last cell.
pub fn find(
    screen: &Screen,
    query: &str,
    from: Point,
    direction: Direction,
    inclusive: bool,
) -> Option<(Point, Point)> {
    if query.is_empty() {
        return None;
    }
    let case_sensitive = query.chars().any(char::is_uppercase);
    let fold = |c: char| {
        if case_sensitive {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let before = |p: Point| p < from || (inclusive && p == from);
    let after = |p: Point| p > from || (inclusive && p == from);

    let top = -(screen.grid().history.len() as isize);
    let bottom = screen.rows() as isize - 1;
    let mut line = from.line.clamp(top, bottom);
    loop {
        let text = LineText::new(screen, Point { line, col: 0 })?;
        let hay: Vec<char> = text.text.chars().map(fold).collect();
        let mut matches = (0..=hay.len().saturating_sub(needle.len()))
            .filter(|&i| hay[i..].starts_with(&needle))
            .map(|i| (text.cells[i].point, text.cells[i + needle.len() - 1].point));
        let found = match direction {
            Direction::Older => matches.rfind(|m| before(m.0)),
            Direction::Newer => matches.find(|m| after(m.0)),
        };
        if found.is_some() {
            return found;
        }
        line = match direction {
            Direction::Older if text.first > top => text.first - 1,
            Direction::Newer if text.last < bottom => text.last + 1,
            _ => return None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn p(line: isize, col: usize) -> Point {
        Point { line, col }
    }

    /// "foo" twice in history, once wrapped across rows on screen.
    fn terminal() -> Terminal {
        let mut t = Terminal::new(8, 3, 10);
        t.feed(b"a foo\r\nFoo b\r\nxx\r\nyyyyyyfoo\r\nzz");
        t
    }

    #[test]
    fn finds_older_matches_in_order() {
        let t = terminal();
        let s = t.screen();
        // History: -3 "a foo", -2 "Foo b", -1 "xx"; screen: 0-1
        // "yyyyyyfo|o", 2 "zz".
        let end = p(2, 7);
        let m = find(s, "foo", end, Direction::Older, true).unwrap();
        assert_eq!(m, (p(0, 6), p(1, 0)));
        let m = find(s, "foo", m.0, Direction::Older, false).unwrap();
        assert_eq!(m, (p(-2, 0), p(-2, 2)));
        let m = find(s, "foo", m.0, Direction::Older, false).unwrap();
        assert_eq!(m, (p(-3, 2), p(-3, 4)));
        assert_eq!(find(s, "foo", m.0, Direction::Older, false), None);
        // And back.
        let m = find(s, "foo", m.0, Direction::Newer, false).unwrap();
        assert_eq!(m, (p(-2, 0), p(-2, 2)));
    }

    #[test]
    fn uppercase_makes_it_case_sensitive() {
        let t = terminal();
        let m = find(t.screen(), "Foo", p(2, 7), Direction::Older, true).unwrap();
        assert_eq!(m, (p(-2, 0), p(-2, 2)));
    }

    #[test]
    fn nothing_found() {
        let t = terminal();
        assert_eq!(
            find(t.screen(), "nope", p(2, 7), Direction::Older, true),
            None
        );
        assert_eq!(find(t.screen(), "", p(2, 7), Direction::Older, true), None);
    }
}
