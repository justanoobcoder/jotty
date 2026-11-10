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

impl Direction {
    pub fn reversed(self) -> Direction {
        match self {
            Direction::Older => Direction::Newer,
            Direction::Newer => Direction::Older,
        }
    }
}

/// Like [`find`], but past the last match in `direction` it continues from
/// the other end, as vim does.
pub fn find_wrapping(
    screen: &Screen,
    query: &str,
    from: Point,
    direction: Direction,
    inclusive: bool,
) -> Option<(Point, Point)> {
    find(screen, query, from, direction, inclusive).or_else(|| {
        let edge = match direction {
            Direction::Older => Point {
                line: screen.rows() as isize - 1,
                col: usize::MAX,
            },
            Direction::Newer => Point {
                line: -(screen.grid().history.len() as isize),
                col: 0,
            },
        };
        find(screen, query, edge, direction, /* inclusive= */ true)
    })
}

/// Finds `query` from `from` in `direction`: the nearest match that starts
/// before `from` (Older) or after it (Newer), or at it with `inclusive`.
/// Matches may span soft-wrapped rows. The search ignores case unless the
/// query has an uppercase letter, and compares the first character of
/// each cell (combining marks are ignored). Returns the first and last
/// cell.
pub fn find(
    screen: &Screen,
    query: &str,
    from: Point,
    direction: Direction,
    inclusive: bool,
) -> Option<(Point, Point)> {
    use unicode_normalization::UnicodeNormalization;
    use unicode_width::UnicodeWidthChar;
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
    // Composed, so that both forms of a letter with a mark match; marks that
    // don't compose are ignored, as they are on screen.
    let needle: Vec<char> = query
        .nfc()
        .filter(|c| c.width() != Some(0))
        .map(fold)
        .collect();
    if needle.is_empty() {
        return None;
    }
    let before = |p: Point| p < from || (inclusive && p == from);
    let after = |p: Point| p > from || (inclusive && p == from);

    let top = -(screen.grid().history.len() as isize);
    let bottom = screen.rows() as isize - 1;
    let mut line = from.line.clamp(top, bottom);
    loop {
        let text = LineText::new(screen, Point { line, col: 0 })?;
        // One character per cell: the cluster's first one.
        let hay: Vec<char> = (0..text.cells.len())
            .map(|i| fold(cell_char(&text, i)))
            .collect();
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

/// The character cell `i` stands for in a search: the one its cluster
/// composes into, or else its first.
fn cell_char(text: &LineText, i: usize) -> char {
    use unicode_normalization::UnicodeNormalization;
    let cell = &text.cells[i];
    let end = text.cells.get(i + 1).map_or(text.text.len(), |c| c.offset);
    let cluster = &text.text[cell.offset..end];
    if cluster.len() == cell.c.len_utf8() {
        return cell.c;
    }
    let mut composed = cluster.nfc();
    match (composed.next(), composed.next()) {
        (Some(c), None) => c,
        _ => cell.c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_of_a_marked_letter_match() {
        let mut t = Terminal::new(20, 1, 0);
        t.feed("cafe\u{301} ha\u{300} ha".as_bytes());
        let from = Point { line: 0, col: 19 };
        for query in ["caf\u{e9}", "cafe\u{301}", "h\u{e0}"] {
            assert!(
                find(
                    t.screen(),
                    query,
                    from,
                    Direction::Older,
                    /* inclusive= */ true
                )
                .is_some(),
                "{query}"
            );
        }
        // The plain letter is another one.
        let found = find(
            t.screen(),
            "ha",
            from,
            Direction::Older,
            /* inclusive= */ true,
        )
        .unwrap();
        assert_eq!(found.0.col, 8);
        let first = Point { line: 0, col: 7 };
        assert_eq!(
            find(
                t.screen(),
                "ha",
                first,
                Direction::Older,
                /* inclusive= */ true
            ),
            None
        );
    }
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
        let m = find(s, "foo", end, Direction::Older, /* inclusive= */ true).unwrap();
        assert_eq!(m, (p(0, 6), p(1, 0)));
        let m = find(s, "foo", m.0, Direction::Older, /* inclusive= */ false).unwrap();
        assert_eq!(m, (p(-2, 0), p(-2, 2)));
        let m = find(s, "foo", m.0, Direction::Older, /* inclusive= */ false).unwrap();
        assert_eq!(m, (p(-3, 2), p(-3, 4)));
        assert_eq!(
            find(s, "foo", m.0, Direction::Older, /* inclusive= */ false),
            None
        );
        // And back.
        let m = find(s, "foo", m.0, Direction::Newer, /* inclusive= */ false).unwrap();
        assert_eq!(m, (p(-2, 0), p(-2, 2)));
    }

    #[test]
    fn uppercase_makes_it_case_sensitive() {
        let t = terminal();
        let m = find(
            t.screen(),
            "Foo",
            p(2, 7),
            Direction::Older,
            /* inclusive= */ true,
        )
        .unwrap();
        assert_eq!(m, (p(-2, 0), p(-2, 2)));
    }

    #[test]
    fn wraps_around() {
        let t = terminal();
        let s = t.screen();
        // Past the oldest match, the newest one.
        let m = find_wrapping(
            s,
            "foo",
            p(-3, 2),
            Direction::Older,
            /* inclusive= */ false,
        )
        .unwrap();
        assert_eq!(m, (p(0, 6), p(1, 0)));
        let m = find_wrapping(s, "foo", m.0, Direction::Newer, /* inclusive= */ false).unwrap();
        assert_eq!(m, (p(-3, 2), p(-3, 4)));
    }

    #[test]
    fn nothing_found() {
        let t = terminal();
        assert_eq!(
            find(
                t.screen(),
                "nope",
                p(2, 7),
                Direction::Older,
                /* inclusive= */ true
            ),
            None
        );
        assert_eq!(
            find(
                t.screen(),
                "",
                p(2, 7),
                Direction::Older,
                /* inclusive= */ true
            ),
            None
        );
    }
}
