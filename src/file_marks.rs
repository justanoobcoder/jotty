//! Files and directories marked in the text (Ctrl+right click or drag),
//! to drag them out together. Points are screen lines, as for the
//! selection: marks follow their text as output scrolls it.

use std::path::{Path, PathBuf};

use crate::selection::{self, Point};
use crate::terminal::Screen;

/// A marked name: the cells it covers and the file it was resolved to
/// when it was marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMark {
    pub start: Point,
    pub end: Point,
    pub path: PathBuf,
}

/// The marks of a window, in order and without overlaps.
#[derive(Debug, Default)]
pub struct FileMarks {
    marks: Vec<FileMark>,
}

impl FileMarks {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn clear(&mut self) {
        self.marks.clear();
    }

    /// The mark covering `point`.
    #[must_use]
    pub fn at(&self, point: Point) -> Option<&FileMark> {
        let after = self.marks.partition_point(|m| m.start <= point);
        let mark = self.marks.get(after.checked_sub(1)?)?;
        (mark.end >= point).then_some(mark)
    }

    /// Marks `mark`, or unmarks it if it is marked.
    pub fn toggle(&mut self, mark: FileMark) {
        match self
            .marks
            .iter()
            .position(|m| m.start == mark.start && m.end == mark.end)
        {
            Some(i) => {
                self.marks.remove(i);
            }
            None => self.add(mark),
        }
    }

    /// Marks `mark`, unless it overlaps a mark already there.
    pub fn add(&mut self, mark: FileMark) {
        let i = self.marks.partition_point(|m| m.start < mark.start);
        let overlaps_before = i > 0 && self.marks[i - 1].end >= mark.start;
        let overlaps_after = self.marks.get(i).is_some_and(|m| m.start <= mark.end);
        if !overlaps_before && !overlaps_after {
            self.marks.insert(i, mark);
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.marks.iter().map(|m| m.path.as_path())
    }

    /// Fingerprints of the rows of the live screen that marks cover, to
    /// tell after output which marks' text changed.
    #[must_use]
    pub fn live_rows(&self, screen: &Screen) -> Vec<(isize, u64)> {
        let last = screen.rows() as isize - 1;
        let mut rows: Vec<(isize, u64)> = Vec::new();
        for mark in &self.marks {
            for line in mark.start.line.max(0)..=mark.end.line.min(last) {
                // Marks are in order: a row shared by two is the last one.
                if rows.last().is_some_and(|&(l, _)| l >= line) {
                    continue;
                }
                if let Some(hash) = selection::row_hash(screen, line) {
                    rows.push((line, hash));
                }
            }
        }
        rows
    }

    /// Updates the marks after output, given the `live_rows` from before it
    /// and how many rows scrolled into the history meanwhile. Marks follow
    /// their rows; a mark whose text changed or scrolled out of the history
    /// is dropped.
    pub fn follow_output(&mut self, screen: &Screen, before: &[(isize, u64)], scrolled: isize) {
        let changed: Vec<isize> = before
            .iter()
            .filter(|&&(line, hash)| selection::row_hash(screen, line - scrolled) != Some(hash))
            .map(|&(line, _)| line - scrolled)
            .collect();
        self.marks.retain_mut(|mark| {
            mark.start.line -= scrolled;
            mark.end.line -= scrolled;
            screen.row_at(mark.start.line).is_some()
                && !changed
                    .iter()
                    .any(|&line| (mark.start.line..=mark.end.line).contains(&line))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn p(line: isize, col: usize) -> Point {
        Point { line, col }
    }

    fn mark(start: Point, end: Point, path: &str) -> FileMark {
        FileMark {
            start,
            end,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn toggling_marks_and_unmarks() {
        let mut marks = FileMarks::default();
        marks.toggle(mark(p(0, 4), p(0, 6), "b"));
        marks.toggle(mark(p(0, 0), p(0, 2), "a"));
        assert_eq!(
            marks.paths().collect::<Vec<_>>(),
            [Path::new("a"), Path::new("b")]
        );
        marks.toggle(mark(p(0, 0), p(0, 2), "a"));
        assert_eq!(marks.paths().collect::<Vec<_>>(), [Path::new("b")]);
    }

    #[test]
    fn overlapping_marks_are_not_added() {
        let mut marks = FileMarks::default();
        marks.add(mark(p(0, 4), p(0, 8), "b"));
        marks.add(mark(p(0, 2), p(0, 4), "x"));
        marks.add(mark(p(0, 8), p(1, 0), "y"));
        marks.add(mark(p(0, 0), p(0, 3), "a"));
        assert_eq!(
            marks.paths().collect::<Vec<_>>(),
            [Path::new("a"), Path::new("b")]
        );
    }

    #[test]
    fn finds_the_mark_at_a_point() {
        let mut marks = FileMarks::default();
        marks.add(mark(p(0, 2), p(0, 4), "a"));
        marks.add(mark(p(0, 8), p(1, 1), "b"));
        let at = |point| marks.at(point).map(|m| m.path.clone());
        assert_eq!(at(p(0, 1)), None);
        assert_eq!(at(p(0, 2)), Some(PathBuf::from("a")));
        assert_eq!(at(p(0, 4)), Some(PathBuf::from("a")));
        assert_eq!(at(p(0, 5)), None);
        assert_eq!(at(p(1, 0)), Some(PathBuf::from("b")));
        assert_eq!(at(p(1, 2)), None);
    }

    #[test]
    fn marks_follow_scrolled_output_and_drop_changed_text() {
        let mut t = Terminal::new(10, 3, 10);
        t.feed(b"a.rs b.rs\r\nc.rs\r\n");
        let mut marks = FileMarks::default();
        marks.add(mark(p(0, 0), p(0, 3), "a"));
        marks.add(mark(p(0, 5), p(0, 8), "b"));
        marks.add(mark(p(1, 0), p(1, 3), "c"));
        let scroll = |t: &mut Terminal, marks: &mut FileMarks, bytes: &[u8]| {
            let before = marks.live_rows(t.screen());
            let scrolled_before = t.screen().grid().scrolled;
            t.feed(bytes);
            let scrolled = t.screen().grid().scrolled.wrapping_sub(scrolled_before) as isize;
            marks.follow_output(t.screen(), &before, scrolled);
        };
        // Two new lines scroll the first row into the history.
        scroll(&mut t, &mut marks, b"x\r\ny");
        assert_eq!(marks.len(), 3);
        assert_eq!(
            marks.at(p(-1, 0)).map(|m| m.path.clone()),
            Some(PathBuf::from("a"))
        );
        assert_eq!(
            marks.at(p(0, 0)).map(|m| m.path.clone()),
            Some(PathBuf::from("c"))
        );
        // Rewriting the row of `c.rs` drops its mark only.
        scroll(&mut t, &mut marks, b"\x1b[1;1Hzzzz");
        assert_eq!(
            marks.paths().collect::<Vec<_>>(),
            [Path::new("a"), Path::new("b")]
        );
    }
}
