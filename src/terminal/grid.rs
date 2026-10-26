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
    /// Hyperlink (OSC 8) id, see `Screen::link`; 0 for none.
    pub link: u16,
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
            link: 0,
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

/// A shell-integration mark (OSC 133) on a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The prompt starts here (`A`).
    Prompt,
    /// The prompt ends and the command line starts here (`B`).
    Input,
    /// The command's output starts here (`C`), at column `col`.
    Output { col: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// The line continues on the next row (it was soft-wrapped).
    pub wrapped: bool,
    /// The last shell-integration mark set on this row.
    pub mark: Option<Mark>,
}

impl Row {
    pub fn new(cols: usize, template: &Cell) -> Self {
        Row {
            cells: vec![Cell::blank(template); cols],
            wrapped: false,
            mark: None,
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
    /// Rows scrolled off the top so far (kept in history or not), so that
    /// positions in the history can follow new output.
    pub scrolled: u64,
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
            scrolled: 0,
            cols,
        }
    }

    #[cfg(test)]
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn push_history(&mut self, row: Row) {
        self.scrolled += 1;
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

    /// Re-wraps the history and screen to `cols` columns and resizes the
    /// screen to `rows`. Soft-wrapped rows are joined into lines first, so
    /// text flows to the new width. `points` are (screen row, column)
    /// positions, such as the cursor, moved along with the text; the first
    /// one stays on screen.
    pub fn reflow(&mut self, cols: usize, rows: usize, points: &mut [(usize, usize)]) {
        let history_len = self.history.len();
        // Blank rows below the content and the points carry nothing.
        let last_content = self
            .rows
            .iter()
            .rposition(|r| r.mark.is_some() || r.cells.iter().any(|c| *c != Cell::default()));
        let keep = points
            .iter()
            .map(|p| p.0)
            .chain(last_content)
            .max()
            .map_or(0, |r| r + 1)
            .min(self.rows.len());
        let old: Vec<Row> = self
            .history
            .drain(..)
            .chain(self.rows.drain(..keep))
            .collect();

        // Join soft-wrapped rows into lines, remembering where each old
        // row starts: (line, offset).
        let mut lines: Vec<Line> = Vec::new();
        let mut starts = Vec::with_capacity(old.len());
        let mut open = false;
        for row in old {
            if !open {
                lines.push(Line::default());
            }
            let index = lines.len() - 1;
            let line = &mut lines[index];
            // A wide character that didn't fit at the end of the previous
            // row left its last column blank.
            if open
                && row
                    .cells
                    .first()
                    .is_some_and(|c| c.flags.contains(Flags::WIDE))
                && line.cells.last() == Some(&Cell::default())
            {
                line.cells.pop();
            }
            starts.push((index, line.cells.len()));
            if let Some(mark) = row.mark {
                line.marks.push((line.cells.len(), mark));
            }
            let mut cells = row.cells;
            if !row.wrapped {
                let len = cells
                    .iter()
                    .rposition(|c| *c != Cell::default())
                    .map_or(0, |i| i + 1);
                cells.truncate(len);
            }
            line.cells.extend(cells);
            open = row.wrapped;
        }

        // Where each point is: (line, offset).
        let targets: Vec<Option<(usize, usize)>> = points
            .iter()
            .map(|&(row, col)| {
                let (line, start) = *starts.get(history_len + row)?;
                Some((line, start + col))
            })
            .collect();

        let mut out: Vec<Row> = Vec::new();
        let mut moved: Vec<Option<usize>> = vec![None; points.len()];
        for (index, line) in lines.iter().enumerate() {
            let first = out.len();
            let layout = line.layout(cols);
            let offsets = targets
                .iter()
                .flatten()
                .filter(|t| t.0 == index)
                .map(|t| t.1)
                .chain(line.marks.iter().map(|m| m.0));
            let needed = offsets
                .map(|o| layout.position(o) + 1)
                .max()
                .unwrap_or(0)
                .max(layout.end);
            let count = needed.div_ceil(cols).max(1);
            for i in 0..count {
                let mut row = Row::new(cols, &Cell::default());
                row.wrapped = i + 1 < count;
                out.push(row);
            }
            for (cell, &pos) in line.cells.iter().zip(&layout.positions) {
                out[first + pos / cols].cells[pos % cols] = *cell;
            }
            for &(offset, mark) in &line.marks {
                let pos = layout.position(offset);
                out[first + pos / cols].mark = Some(mark);
            }
            for (target, slot) in targets.iter().zip(&mut moved) {
                if let Some((_, offset)) = target.filter(|t| t.0 == index) {
                    let pos = layout.position(offset);
                    *slot = Some((first + pos / cols) * cols + pos % cols);
                }
            }
        }

        // The screen shows the last rows, unless that would hide the first
        // point (the cursor).
        let total = out.len();
        let mut start = total.saturating_sub(rows);
        if let Some(Some(pos)) = moved.first() {
            start = start.min(pos / cols);
        }
        let mut screen: Vec<Row> = out.drain(start..).take(rows).collect();
        while screen.len() < rows {
            screen.push(Row::new(cols, &Cell::default()));
        }
        for (point, pos) in points.iter_mut().zip(moved) {
            if let Some(pos) = pos {
                let row = (pos / cols).saturating_sub(start).min(rows - 1);
                *point = (row, pos % cols);
            } else {
                *point = (point.0.min(rows - 1), point.1.min(cols - 1));
            }
        }
        let drop = out.len().saturating_sub(self.max_history);
        self.history = out.into_iter().skip(drop).collect();
        self.rows = screen;
        self.cols = cols;
    }
}

/// A logical line: soft-wrapped rows joined together.
#[derive(Default)]
struct Line {
    cells: Vec<Cell>,
    /// Shell-integration marks by cell offset.
    marks: Vec<(usize, Mark)>,
}

/// Where a line's cells go when wrapped to some width.
struct Layout {
    /// Linear position (row × columns + column) of each cell.
    positions: Vec<usize>,
    /// Position just past the last cell.
    end: usize,
    len: usize,
}

impl Line {
    fn layout(&self, cols: usize) -> Layout {
        let mut positions = Vec::with_capacity(self.cells.len());
        let mut pos = 0;
        for cell in &self.cells {
            // A wide character never straddles two rows.
            if cell.flags.contains(Flags::WIDE) && cols > 1 && pos % cols == cols - 1 {
                pos += 1;
            }
            positions.push(pos);
            pos += 1;
        }
        Layout {
            positions,
            end: pos,
            len: self.cells.len(),
        }
    }
}

impl Layout {
    /// The new position of cell `offset`; past the end, positions continue
    /// linearly (the cursor can be beyond the text).
    fn position(&self, offset: usize) -> usize {
        match self.positions.get(offset) {
            Some(&pos) => pos,
            None => self.end + (offset - self.len),
        }
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
            link: 1,
        };
        let blank = Cell::blank(&template);
        assert_eq!(blank.link, 0);
        assert_eq!(blank.c, ' ');
        assert_eq!(blank.fg, Color::Default);
        assert_eq!(blank.bg, Color::Indexed(2));
        assert_eq!(blank.flags, Flags::empty());
        assert_eq!(blank.underline, Underline::None);
    }
}
