//! Vi mode: a cursor that moves over the screen and the scrollback with
//! vim keys, for selecting (by character, line or block), yanking and
//! searching without the mouse.

use crate::search::Direction;
use crate::selection::{self, Point, Selection, Unit};
use crate::terminal::{Flags, Screen};

/// A key, as vi mode sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// Ctrl with a letter.
    Ctrl(char),
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visual {
    Char,
    Line,
    Block,
}

/// What a key asks the application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Handled: the cursor or selection may have changed.
    Done,
    /// Not a vi key; it may still be a key binding.
    Ignored,
    /// Leave vi mode.
    Exit,
    /// Copy this text to the clipboard.
    Yank(String),
    /// Open the search bar, searching in this direction.
    Search(Direction),
    /// Repeat the last search, in its direction or reversed.
    Next { reverse: bool },
}

pub struct ViMode {
    pub cursor: Point,
    /// The visual mode and where it started.
    visual: Option<(Visual, Point)>,
    /// A count typed before a command (`5j`).
    count: Option<usize>,
    /// The first key of a two-key command (`gg`, `yy`), with its count.
    pending: Option<(char, usize)>,
}

impl ViMode {
    pub fn new(cursor: Point) -> ViMode {
        ViMode {
            cursor,
            visual: None,
            count: None,
            pending: None,
        }
    }

    pub fn visual(&self) -> Option<Visual> {
        self.visual.map(|(mode, _)| mode)
    }

    /// What visual mode covers, from where it started to the cursor.
    pub fn selection(&self, screen: &Screen) -> Option<Selection> {
        let (mode, anchor) = self.visual?;
        let (start, end) = (anchor.min(self.cursor), anchor.max(self.cursor));
        Some(match mode {
            Visual::Char => Selection::span(start, end, Unit::Fixed),
            Visual::Line => Selection::span(
                selection::line_bounds(screen, start).0,
                selection::line_bounds(screen, end).1,
                Unit::Fixed,
            ),
            Visual::Block => Selection::block(anchor, self.cursor),
        })
    }

    /// Follows the text after `lines` rows scrolled into the history.
    pub fn scroll(&mut self, lines: isize, screen: &Screen) {
        let top = top(screen);
        self.cursor.line = (self.cursor.line - lines).max(top);
        if let Some((_, anchor)) = &mut self.visual {
            anchor.line = (anchor.line - lines).max(top);
        }
        // The line it is on now may be shorter.
        self.clamp(screen);
    }

    /// Moves the cursor to `p` (a search match), within bounds.
    pub fn move_to(&mut self, p: Point, screen: &Screen) {
        self.cursor = p;
        self.clamp(screen);
    }

    pub fn key(&mut self, screen: &Screen, key: Key) -> Outcome {
        if let Key::Char(c @ '0'..='9') = key
            && (c != '0' || self.count.is_some())
            && self.pending.is_none()
        {
            let digit = c as usize - '0' as usize;
            self.count = Some((self.count.unwrap_or(0) * 10 + digit).min(100_000));
            return Outcome::Done;
        }
        let count = self.count.take().unwrap_or(1);
        if let Some((first, count)) = self.pending.take() {
            return self.second_key(screen, first, key, count);
        }

        let (top, bottom) = (top(screen), screen.rows() as isize - 1);
        let rows = screen.rows() as isize;
        let lines = count as isize;
        let p = &mut self.cursor;
        match key {
            Key::Char('h') | Key::Left | Key::Ctrl('h') => p.col = p.col.saturating_sub(count),
            Key::Char('l' | ' ') | Key::Right => p.col = p.col.saturating_add(count),
            Key::Char('j') | Key::Down | Key::Ctrl('n' | 'j') => p.line += lines,
            Key::Char('k') | Key::Up | Key::Ctrl('p') => p.line -= lines,
            Key::Char('0') | Key::Home => p.col = 0,
            Key::Char('^') => p.col = first_non_blank(screen, p.line),
            Key::Char('$') | Key::End => p.col = last_non_blank(screen, p.line),
            Key::Char(c @ ('w' | 'W' | 'b' | 'B' | 'e' | 'E')) => {
                for _ in 0..count {
                    *p = word_motion(screen, *p, c);
                }
            }
            Key::Char('g') => self.pending = Some(('g', count)),
            Key::Char('G') => {
                *p = Point {
                    line: bottom,
                    col: 0,
                }
            }
            Key::Char(c @ ('H' | 'M' | 'L')) => {
                let visible_top = -(screen.display_offset() as isize);
                p.line = visible_top
                    + match c {
                        'H' => 0,
                        'M' => (rows - 1) / 2,
                        _ => rows - 1,
                    };
                p.col = first_non_blank(screen, p.line);
            }
            Key::Ctrl('u') => p.line -= rows / 2 * lines,
            Key::Ctrl('d') => p.line += rows / 2 * lines,
            Key::Ctrl('b') | Key::PageUp => p.line -= rows * lines,
            Key::Ctrl('f') | Key::PageDown => p.line += rows * lines,
            Key::Char('v') => self.toggle_visual(Visual::Char),
            Key::Char('V') => self.toggle_visual(Visual::Line),
            Key::Ctrl('v') => self.toggle_visual(Visual::Block),
            Key::Char('o') => {
                if let Some((_, anchor)) = &mut self.visual {
                    std::mem::swap(anchor, &mut self.cursor);
                }
            }
            Key::Char('y') => match self.selection(screen) {
                Some(selection) => {
                    // As in vim, the cursor goes to the start of what was
                    // yanked and visual mode ends.
                    self.cursor = selection.range().0;
                    self.visual = None;
                    return Outcome::Yank(selection.text(screen));
                }
                None => self.pending = Some(('y', count)),
            },
            Key::Char('/') => return Outcome::Search(Direction::Newer),
            Key::Char('?') => return Outcome::Search(Direction::Older),
            Key::Char('n') => return Outcome::Next { reverse: false },
            Key::Char('N') => return Outcome::Next { reverse: true },
            Key::Escape | Key::Ctrl('c' | '[') => {
                if self.visual.take().is_none() {
                    return Outcome::Exit;
                }
            }
            Key::Char('i' | 'I' | 'a' | 'A' | 'q') => return Outcome::Exit,
            _ => return Outcome::Ignored,
        }
        self.cursor.line = self.cursor.line.clamp(top, bottom);
        self.clamp(screen);
        Outcome::Done
    }

    fn second_key(&mut self, screen: &Screen, first: char, key: Key, count: usize) -> Outcome {
        match (first, key) {
            // `gg` goes to the first line, `5gg` to the fifth.
            ('g', Key::Char('g')) => {
                let line = top(screen).saturating_add_unsigned(count - 1);
                self.cursor = Point {
                    line: line.min(screen.rows() as isize - 1),
                    col: 0,
                };
                Outcome::Done
            }
            // `yy`: whole lines, without the final newline (which would
            // run a pasted command).
            ('y', Key::Char('y')) => {
                let line = self.cursor.line;
                let last = (line + count as isize - 1).min(screen.rows() as isize - 1);
                let start = selection::line_bounds(screen, self.cursor).0;
                let end = selection::line_bounds(screen, Point { line: last, col: 0 }).1;
                Outcome::Yank(Selection::span(start, end, Unit::Fixed).text(screen))
            }
            // Anything else cancels the command.
            _ => Outcome::Done,
        }
    }

    fn toggle_visual(&mut self, mode: Visual) {
        self.visual = match self.visual {
            Some((current, _)) if current == mode => None,
            Some((_, anchor)) => Some((mode, anchor)),
            None => Some((mode, self.cursor)),
        };
    }

    /// Keeps the cursor on the screen's columns and off the second half of
    /// a wide character.
    fn clamp(&mut self, screen: &Screen) {
        let p = &mut self.cursor;
        p.line = p.line.clamp(top(screen), screen.rows() as isize - 1);
        p.col = p.col.min(screen.cols() - 1);
        if p.col > 0 && cell_flags(screen, *p).contains(Flags::WIDE_SPACER) {
            p.col -= 1;
        }
    }
}

/// The first line of the history.
fn top(screen: &Screen) -> isize {
    -(screen.grid().history.len() as isize)
}

fn cell_flags(screen: &Screen, p: Point) -> Flags {
    screen
        .row_at(p.line)
        .and_then(|r| r.cells().get(p.col))
        .map_or(Flags::empty(), |c| c.flags)
}

/// The character at `p`; the second half of a wide character counts as
/// the character.
fn char_at(screen: &Screen, p: Point) -> char {
    let Some(row) = screen.row_at(p.line) else {
        return ' ';
    };
    match row.cells().get(p.col) {
        Some(c) if c.flags.contains(Flags::WIDE_SPACER) && p.col > 0 => row.cells()[p.col - 1].c,
        Some(c) => c.c,
        None => ' ',
    }
}

fn first_non_blank(screen: &Screen, line: isize) -> usize {
    (0..screen.cols())
        .find(|&col| !char_at(screen, Point { line, col }).is_whitespace())
        .unwrap_or(0)
}

fn last_non_blank(screen: &Screen, line: isize) -> usize {
    (0..screen.cols())
        .rev()
        .find(|&col| !char_at(screen, Point { line, col }).is_whitespace())
        .unwrap_or(0)
}

/// Character classes for word motions: blanks, word characters, and other
/// punctuation. With `big` (W, B, E), everything but blanks is one class.
fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// The next cell (skipping the second half of wide characters), and
/// whether a line ended in between.
fn next(screen: &Screen, p: Point) -> Option<(Point, bool)> {
    let mut q = p;
    loop {
        if q.col + 1 < screen.cols() {
            q.col += 1;
        } else if q.line + 1 < screen.rows() as isize {
            q = Point {
                line: q.line + 1,
                col: 0,
            };
        } else {
            return None;
        }
        if !cell_flags(screen, q).contains(Flags::WIDE_SPACER) {
            let broke = q.line != p.line && !screen.row_at(p.line).is_some_and(|r| r.wrapped);
            return Some((q, broke));
        }
    }
}

/// The previous cell, and whether a line ended in between.
fn prev(screen: &Screen, p: Point) -> Option<(Point, bool)> {
    let mut q = p;
    loop {
        if q.col > 0 {
            q.col -= 1;
        } else if q.line > top(screen) {
            q = Point {
                line: q.line - 1,
                col: screen.cols() - 1,
            };
        } else {
            return None;
        }
        if !cell_flags(screen, q).contains(Flags::WIDE_SPACER) {
            let broke = q.line != p.line && !screen.row_at(q.line).is_some_and(|r| r.wrapped);
            return Some((q, broke));
        }
    }
}

/// Whether `p` is on a line with nothing on it.
fn is_empty_line(screen: &Screen, p: Point) -> bool {
    screen
        .row_at(p.line)
        .is_some_and(|row| !row.wrapped && row.cells().iter().all(|c| c.c == ' '))
}

/// Where `w`, `b`, `e` (or `W`, `B`, `E`) move from `p`. The end of a line
/// separates words like a blank.
fn word_motion(screen: &Screen, p: Point, motion: char) -> Point {
    let big = motion.is_uppercase();
    let class_at = |q: Point| class(char_at(screen, q), big);
    match motion.to_ascii_lowercase() {
        'w' => {
            let mut q = p;
            let start = class_at(p);
            // The rest of this word, then blanks.
            let mut in_word = start != 0;
            loop {
                let Some((n, broke)) = next(screen, q) else {
                    return q;
                };
                q = n;
                // An empty line counts as a word, as in vim.
                if broke && is_empty_line(screen, q) {
                    return q;
                }
                if in_word && (broke || class_at(q) != start) {
                    in_word = false;
                }
                if !in_word && class_at(q) != 0 {
                    return q;
                }
            }
        }
        'e' => {
            // At least one step, past blanks, then to the word's end.
            let Some((mut q, _)) = next(screen, p) else {
                return p;
            };
            while class_at(q) == 0 {
                let Some((n, _)) = next(screen, q) else {
                    return q;
                };
                q = n;
            }
            let class = class_at(q);
            while let Some((n, broke)) = next(screen, q) {
                if broke || class_at(n) != class {
                    break;
                }
                q = n;
            }
            q
        }
        _ => {
            // 'b': at least one step back, past blanks, then to the start.
            let Some((mut q, _)) = prev(screen, p) else {
                return p;
            };
            while class_at(q) == 0 {
                if is_empty_line(screen, q) {
                    return Point { col: 0, ..q };
                }
                let Some((n, _)) = prev(screen, q) else {
                    return q;
                };
                q = n;
            }
            let class = class_at(q);
            while let Some((n, broke)) = prev(screen, q) {
                if broke || class_at(n) != class {
                    break;
                }
                q = n;
            }
            q
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn p(line: isize, col: usize) -> Point {
        Point { line, col }
    }

    /// Presses each key of `keys` (plain characters).
    fn press(vi: &mut ViMode, t: &Terminal, keys: &str) -> Outcome {
        let mut outcome = Outcome::Done;
        for c in keys.chars() {
            outcome = vi.key(t.screen(), Key::Char(c));
        }
        outcome
    }

    fn terminal() -> Terminal {
        let mut t = Terminal::new(20, 3, 10);
        // History: -2 "one two", -1 "  foo.bar(baz)"; screen 0-2.
        t.feed(b"one two\r\n  foo.bar(baz)\r\nthree\r\nfour five\r\nsix");
        t
    }

    #[test]
    fn hjkl_with_counts_and_bounds() {
        let t = terminal();
        let mut vi = ViMode::new(p(2, 0));
        press(&mut vi, &t, "3l");
        assert_eq!(vi.cursor, p(2, 3));
        press(&mut vi, &t, "k");
        assert_eq!(vi.cursor, p(1, 3));
        press(&mut vi, &t, "100k");
        assert_eq!(vi.cursor, p(-2, 3));
        press(&mut vi, &t, "100l");
        assert_eq!(vi.cursor, p(-2, 19));
        press(&mut vi, &t, "G");
        assert_eq!(vi.cursor, p(2, 0));
        press(&mut vi, &t, "gg");
        assert_eq!(vi.cursor, p(-2, 0));
    }

    #[test]
    fn line_motions() {
        let t = terminal();
        let mut vi = ViMode::new(p(-1, 8));
        press(&mut vi, &t, "^");
        assert_eq!(vi.cursor, p(-1, 2));
        press(&mut vi, &t, "$");
        assert_eq!(vi.cursor, p(-1, 13));
        press(&mut vi, &t, "0");
        assert_eq!(vi.cursor, p(-1, 0));
    }

    #[test]
    fn gg_takes_a_count() {
        let mut t = Terminal::new(10, 5, 0);
        t.feed(b"a\r\nb\r\nc");
        let mut vi = ViMode::new(p(0, 0));
        press(&mut vi, &t, "3gg");
        assert_eq!(vi.cursor, p(2, 0));
        press(&mut vi, &t, "gg");
        assert_eq!(vi.cursor, p(0, 0));
        press(&mut vi, &t, "99gg");
        assert_eq!(vi.cursor, p(4, 0));
    }

    #[test]
    fn word_motions_stop_on_empty_lines() {
        let mut t = Terminal::new(10, 5, 0);
        t.feed(b"one\r\n\r\ntwo");
        let mut vi = ViMode::new(p(0, 0));
        press(&mut vi, &t, "gg");
        press(&mut vi, &t, "w");
        assert_eq!(vi.cursor, p(1, 0));
        press(&mut vi, &t, "w");
        assert_eq!(vi.cursor, p(2, 0));
        press(&mut vi, &t, "b");
        assert_eq!(vi.cursor, p(1, 0));
        press(&mut vi, &t, "b");
        assert_eq!(vi.cursor, p(0, 0));
    }

    #[test]
    fn word_motions() {
        let t = terminal();
        let mut vi = ViMode::new(p(-2, 0));
        press(&mut vi, &t, "w");
        assert_eq!(vi.cursor, p(-2, 4));
        // The line end separates words.
        press(&mut vi, &t, "w");
        assert_eq!(vi.cursor, p(-1, 2));
        press(&mut vi, &t, "w");
        assert_eq!(vi.cursor, p(-1, 5), "punctuation is its own word");
        press(&mut vi, &t, "e");
        assert_eq!(vi.cursor, p(-1, 8));
        press(&mut vi, &t, "e");
        assert_eq!(vi.cursor, p(-1, 9));
        press(&mut vi, &t, "b");
        assert_eq!(vi.cursor, p(-1, 6));
        press(&mut vi, &t, "B");
        assert_eq!(vi.cursor, p(-1, 2));
        press(&mut vi, &t, "W");
        assert_eq!(vi.cursor, p(0, 0));
        press(&mut vi, &t, "2b");
        assert_eq!(vi.cursor, p(-1, 10));
    }

    #[test]
    fn visual_modes_and_yank() {
        let t = terminal();
        let mut vi = ViMode::new(p(-2, 4));
        press(&mut vi, &t, "vjll");
        assert_eq!(vi.visual(), Some(Visual::Char));
        assert_eq!(
            vi.selection(t.screen()).unwrap().text(t.screen()),
            "two\n  foo.b"
        );
        // Switch to line mode, then yank.
        press(&mut vi, &t, "V");
        assert_eq!(
            press(&mut vi, &t, "y"),
            Outcome::Yank("one two\n  foo.bar(baz)".into())
        );
        assert_eq!(vi.visual(), None);
        assert_eq!(vi.cursor, p(-2, 0));
    }

    #[test]
    fn block_visual() {
        let t = terminal();
        let mut vi = ViMode::new(p(0, 1));
        vi.key(t.screen(), Key::Ctrl('v'));
        press(&mut vi, &t, "jjll");
        assert_eq!(vi.visual(), Some(Visual::Block));
        assert_eq!(
            press(&mut vi, &t, "y"),
            Outcome::Yank("hre\nour\nix".into())
        );
    }

    #[test]
    fn yy_yanks_lines() {
        let t = terminal();
        let mut vi = ViMode::new(p(0, 2));
        assert_eq!(
            press(&mut vi, &t, "2yy"),
            Outcome::Yank("three\nfour five".into())
        );
    }

    #[test]
    fn search_and_exit_keys() {
        let t = terminal();
        let mut vi = ViMode::new(p(0, 0));
        assert_eq!(press(&mut vi, &t, "?"), Outcome::Search(Direction::Older));
        assert_eq!(press(&mut vi, &t, "N"), Outcome::Next { reverse: true });
        press(&mut vi, &t, "v");
        assert_eq!(vi.key(t.screen(), Key::Escape), Outcome::Done);
        assert_eq!(vi.visual(), None);
        assert_eq!(vi.key(t.screen(), Key::Escape), Outcome::Exit);
        assert_eq!(press(&mut vi, &t, "I"), Outcome::Exit);
        assert_eq!(press(&mut vi, &t, "x"), Outcome::Ignored);
    }

    #[test]
    fn follows_scrolling() {
        let t = terminal();
        let mut vi = ViMode::new(p(1, 0));
        vi.scroll(2, t.screen());
        assert_eq!(vi.cursor, p(-1, 0));
        vi.scroll(10, t.screen());
        assert_eq!(vi.cursor, p(-2, 0));
    }
}
