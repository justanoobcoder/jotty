//! The text of a logical line (soft-wrapped rows joined), with the cell
//! each character came from, for finding links and searching.

use crate::selection::Point;
use crate::terminal::{Flags, Screen};

/// One cell of a [`LineText`].
pub struct TextCell {
    pub point: Point,
    /// The cell's character, without the rest of its cluster.
    pub c: char,
    /// Byte offset of the cell's character in the text.
    pub offset: usize,
    pub link: u16,
}

/// The text of a logical line (soft-wrapped rows joined), with where each
/// character came from.
pub struct LineText {
    pub text: String,
    pub cells: Vec<TextCell>,
    /// The first and last screen line of the rows joined.
    pub first: isize,
    pub last: isize,
}

impl LineText {
    /// The logical line that the row at `at.line` belongs to.
    pub fn new(screen: &Screen, at: Point) -> Option<LineText> {
        Self::around(screen, at, isize::MAX)
    }

    /// Like `new`, with at most `reach` rows of the line on either side of
    /// `at`: a line can wrap over the whole history.
    pub fn around(screen: &Screen, at: Point, reach: isize) -> Option<LineText> {
        screen.row_at(at.line)?;
        let limit = at.line.saturating_add(reach);
        let mut first = at.line;
        while at.line - first < reach && screen.row_at(first - 1).is_some_and(|r| r.wrapped) {
            first -= 1;
        }
        let mut text = String::new();
        let mut cells = Vec::new();
        let mut line = first;
        while let Some(row) = screen.row_at(line) {
            for (col, cell) in row.cells.iter().enumerate() {
                if cell.flags.intersects(Flags::WIDE_SPACER | Flags::WRAP_GAP) {
                    continue;
                }
                cells.push(TextCell {
                    point: Point { line, col },
                    c: cell.c,
                    offset: text.len(),
                    link: screen.extra(cell).link,
                });
                screen.cell_text(cell, &mut text);
            }
            if !row.wrapped || line >= limit {
                break;
            }
            line += 1;
        }
        Some(LineText {
            text,
            cells,
            first,
            last: line,
        })
    }

    /// The cell at `at`; the second half of a wide character counts as its
    /// first half.
    pub fn index_of(&self, at: Point) -> Option<usize> {
        self.cells
            .iter()
            .rposition(|c| c.point.line == at.line && c.point.col <= at.col)
    }

    /// The point of the character starting at byte `offset`.
    pub fn point_at(&self, offset: usize) -> Point {
        let i = self.cells.partition_point(|c| c.offset < offset);
        self.cells[i.min(self.cells.len() - 1)].point
    }

    /// The point of the last character before byte `offset`.
    pub fn point_before(&self, offset: usize) -> Point {
        let i = self.cells.partition_point(|c| c.offset < offset);
        self.cells[i.saturating_sub(1)].point
    }
}
