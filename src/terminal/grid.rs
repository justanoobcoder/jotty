//! Cells, rows and the screen grid with its scrollback history.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// The configured default foreground or background.
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags(u16);

impl Flags {
    pub const BOLD: Flags = Flags(1 << 0);
    pub const DIM: Flags = Flags(1 << 1);
    pub const ITALIC: Flags = Flags(1 << 2);
    pub const BLINK: Flags = Flags(1 << 4);
    pub const INVERSE: Flags = Flags(1 << 5);
    pub const HIDDEN: Flags = Flags(1 << 6);
    pub const STRIKE: Flags = Flags(1 << 7);
    /// First half of a double-width character.
    pub const WIDE: Flags = Flags(1 << 8);
    /// Placeholder occupying the second half of a double-width character.
    pub const WIDE_SPACER: Flags = Flags(1 << 9);

    pub const fn empty() -> Self {
        Flags(0)
    }

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Flags) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Flags) {
        self.0 &= !other.0;
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;

    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

/// Underline style, as set by SGR 4 and its `4:n` sub-parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
    pub underline: Underline,
    /// Underline color (SGR 58); `Default` means the text color.
    pub underline_color: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: Flags::empty(),
            underline: Underline::None,
            underline_color: Color::Default,
        }
    }
}

impl Cell {
    /// A blank cell that keeps only the background of `template`
    /// (background color erase).
    pub fn blank(template: &Cell) -> Cell {
        Cell {
            bg: template.bg,
            ..Cell::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// The line continues on the next row (it was soft-wrapped).
    pub wrapped: bool,
}

impl Row {
    pub fn new(cols: usize, template: &Cell) -> Self {
        Row {
            cells: vec![Cell::blank(template); cols],
            wrapped: false,
        }
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
        if let Some(last) = self.cells.last_mut()
            && last.flags.contains(Flags::WIDE)
        {
            // The spacer half was cut off.
            *last = Cell::default();
        }
    }

    /// The row's text without wide-character spacers and trailing blanks.
    #[cfg(test)]
    pub fn text(&self) -> String {
        let mut s: String = self
            .cells
            .iter()
            .filter(|c| !c.flags.contains(Flags::WIDE_SPACER))
            .map(|c| c.c)
            .collect();
        s.truncate(s.trim_end_matches(' ').len());
        s
    }
}

/// One screen buffer: the visible rows plus scrollback history.
#[derive(Debug, Clone)]
pub struct Grid {
    pub rows: Vec<Row>,
    pub history: VecDeque<Row>,
    pub max_history: usize,
    cols: usize,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, max_history: usize) -> Self {
        Grid {
            rows: (0..rows)
                .map(|_| Row::new(cols, &Cell::default()))
                .collect(),
            history: VecDeque::new(),
            max_history,
            cols,
        }
    }

    #[cfg(test)]
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn push_history(&mut self, row: Row) {
        if self.max_history == 0 {
            return;
        }
        if self.history.len() == self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(row);
    }

    /// Scrolls rows `top..=bottom` up by `n`, filling the bottom with blank
    /// rows. With `keep`, rows scrolled off go into the history.
    /// Returns how many rows were scrolled.
    pub fn scroll_up(
        &mut self,
        top: usize,
        bottom: usize,
        n: usize,
        template: &Cell,
        keep: bool,
    ) -> usize {
        let n = n.min(bottom + 1 - top);
        for _ in 0..n {
            let row = self.rows.remove(top);
            if keep {
                self.push_history(row);
            }
            self.rows.insert(bottom, Row::new(self.cols, template));
        }
        n
    }

    /// Scrolls rows `top..=bottom` down by `n`, filling the top with blank rows.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, n: usize, template: &Cell) {
        let n = n.min(bottom + 1 - top);
        for _ in 0..n {
            self.rows.remove(bottom);
            self.rows.insert(top, Row::new(self.cols, template));
        }
    }

    /// Resizes without reflowing. When shrinking vertically, `keep_row` (the
    /// cursor row) stays on screen by moving rows above it into history.
    /// Returns how many rows were moved into history.
    pub fn resize(&mut self, cols: usize, rows: usize, keep_row: usize) -> usize {
        self.cols = cols;
        for row in &mut self.rows {
            row.resize(cols);
        }
        let mut pushed = 0;
        if rows < self.rows.len() {
            let excess = (keep_row + 1).saturating_sub(rows);
            for row in self.rows.drain(..excess).collect::<Vec<_>>() {
                self.push_history(row);
                pushed += 1;
            }
            self.rows.truncate(rows);
        }
        while self.rows.len() < rows {
            self.rows.push(Row::new(cols, &Cell::default()));
        }
        pushed
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_with_lines(lines: &[&str], cols: usize) -> Grid {
        let mut grid = Grid::new(cols, lines.len(), 100);
        for (row, line) in grid.rows.iter_mut().zip(lines) {
            for (cell, c) in row.cells.iter_mut().zip(line.chars()) {
                cell.c = c;
            }
        }
        grid
    }

    fn texts(grid: &Grid) -> Vec<String> {
        grid.rows.iter().map(Row::text).collect()
    }

    #[test]
    fn scroll_up_pushes_history() {
        let mut grid = grid_with_lines(&["a", "b", "c"], 4);
        grid.scroll_up(0, 2, 1, &Cell::default(), true);
        assert_eq!(texts(&grid), ["b", "c", ""]);
        assert_eq!(grid.history.len(), 1);
        assert_eq!(grid.history[0].text(), "a");
    }

    #[test]
    fn scroll_region_does_not_touch_history() {
        let mut grid = grid_with_lines(&["a", "b", "c", "d"], 4);
        grid.scroll_up(1, 2, 1, &Cell::default(), false);
        assert_eq!(texts(&grid), ["a", "c", "", "d"]);
        assert!(grid.history.is_empty());
        grid.scroll_down(0, 3, 2, &Cell::default());
        assert_eq!(texts(&grid), ["", "", "a", "c"]);
    }

    #[test]
    fn history_is_bounded() {
        let mut grid = Grid::new(2, 1, 2);
        for c in ['a', 'b', 'c'] {
            grid.rows[0].cells[0].c = c;
            grid.scroll_up(0, 0, 1, &Cell::default(), true);
        }
        let history: Vec<_> = grid.history.iter().map(Row::text).collect();
        assert_eq!(history, ["b", "c"]);
    }

    #[test]
    fn resize_keeps_cursor_row() {
        let mut grid = grid_with_lines(&["a", "b", "c", "d"], 4);
        let pushed = grid.resize(4, 2, 3);
        assert_eq!(pushed, 2);
        assert_eq!(texts(&grid), ["c", "d"]);
        assert_eq!(grid.history.len(), 2);

        grid.resize(2, 3, 0);
        assert_eq!(grid.num_rows(), 3);
        assert!(grid.rows.iter().all(|r| r.cells.len() == 2));
    }

    #[test]
    fn row_text_skips_spacers() {
        let mut row = Row::new(4, &Cell::default());
        row.cells[0].c = '中';
        row.cells[0].flags.insert(Flags::WIDE);
        row.cells[1].flags.insert(Flags::WIDE_SPACER);
        row.cells[2].c = 'x';
        assert_eq!(row.text(), "中x");
    }

    #[test]
    fn blank_keeps_background_only() {
        let template = Cell {
            c: 'x',
            fg: Color::Indexed(1),
            bg: Color::Indexed(2),
            flags: Flags::BOLD,
            underline: Underline::Curly,
            underline_color: Color::Indexed(3),
        };
        let blank = Cell::blank(&template);
        assert_eq!(blank.c, ' ');
        assert_eq!(blank.fg, Color::Default);
        assert_eq!(blank.bg, Color::Indexed(2));
        assert_eq!(blank.flags, Flags::empty());
        assert_eq!(blank.underline, Underline::None);
    }
}
