//! Cells, rows and the screen grid with its scrollback history.

use std::collections::VecDeque;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    /// Continues the grapheme cluster of an earlier cell (an emoji after a
    /// ZWJ, a skin tone, the second letter of a flag). That cell holds the
    /// whole cluster and draws it; this one only takes up room.
    pub const CONTINUATION: Flags = Flags(1 << 10);
    /// The blank left at the end of a row by a wide character that didn't
    /// fit there. It isn't text: the line goes on at the next row.
    pub const WRAP_GAP: Flags = Flags(1 << 11);

    pub const fn empty() -> Self {
        Flags(0)
    }

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any of the flags in `other` is set.
    pub const fn intersects(self, other: Flags) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Flags) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Flags) {
        self.0 &= !other.0;
    }

    /// The underline style, kept in bits 12–14 so that `Cell` stays small.
    pub const fn underline(self) -> Underline {
        match (self.0 & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT {
            0 => Underline::None,
            1 => Underline::Single,
            2 => Underline::Double,
            3 => Underline::Curly,
            4 => Underline::Dotted,
            _ => Underline::Dashed,
        }
    }

    pub fn set_underline(&mut self, underline: Underline) {
        let bits = (underline as u16) << Self::UNDERLINE_SHIFT;
        self.0 = (self.0 & !Self::UNDERLINE_MASK) | bits;
    }

    const UNDERLINE_SHIFT: u16 = 12;
    const UNDERLINE_MASK: u16 = 0b111 << Self::UNDERLINE_SHIFT;
}

impl std::ops::BitOr for Flags {
    type Output = Flags;

    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

/// Underline style, as set by SGR 4 and its `4:n` sub-parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// One cell of the grid. It is 16 bytes: the grid and its history hold
/// millions, and printing and scrolling are bound by writing them. What
/// few cells carry beyond colors and flags is in `Extra`, by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    /// Attributes, including the underline style (`Flags::underline`).
    pub flags: Flags,
    /// The cell's `Extra`, see `Screen::extra`; 0 for none.
    pub extra: u16,
}

const _: () = assert!(std::mem::size_of::<Cell>() == 16, "a Cell must stay 16 bytes");

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: Flags::empty(),
            extra: 0,
        }
    }
}

/// What a cell carries besides its character, colors and flags. Few cells
/// have any of it, so cells refer to it by id (`Cell::extra`), and cells
/// with the same share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Extra {
    /// Hyperlink (OSC 8) id, see `Screen::link`; 0 for none.
    pub link: u16,
    /// Characters after `c` in its grapheme cluster (combining marks,
    /// emoji sequences), by id; 0 for none. See `Screen::cluster`.
    pub cluster: u16,
    /// Underline color (SGR 58); `Default` means the text color.
    pub underline_color: Color,
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

#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    /// The cells from here on are all alike: nothing wrote them since the
    /// row was made blank. A blank row is reset by writing only the cells
    /// before it, which matters because scrolling resets a row per line
    /// of output, and the row it reuses, from the far end of the history,
    /// is no longer in the cache.
    occupied: usize,
    /// The line continues on the next row (it was soft-wrapped).
    pub wrapped: bool,
    /// The last shell-integration mark set on this row.
    pub mark: Option<Mark>,
}

impl PartialEq for Row {
    fn eq(&self, other: &Row) -> bool {
        // `occupied` only says how much a reset has to write.
        self.cells == other.cells && self.wrapped == other.wrapped && self.mark == other.mark
    }
}

impl Eq for Row {}

impl Row {
    pub fn new(cols: usize, template: &Cell) -> Self {
        Row {
            cells: vec![Cell::blank(template); cols],
            occupied: 0,
            wrapped: false,
            mark: None,
        }
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// All the cells, to change any of them.
    pub fn cells_mut(&mut self) -> &mut [Cell] {
        self.occupied = self.cells.len();
        &mut self.cells
    }

    /// The cells in `range`, to write them. Cheaper than `cells_mut` for
    /// the next reset when the range ends before the last column.
    pub fn cells_in(&mut self, range: Range<usize>) -> &mut [Cell] {
        self.occupied = self.occupied.max(range.end);
        &mut self.cells[range]
    }

    /// The cell at `col`, to write it.
    pub fn cell_mut(&mut self, col: usize) -> &mut Cell {
        self.occupied = self.occupied.max(col + 1);
        &mut self.cells[col]
    }

    /// Makes this a blank row of `cols` cells, keeping its allocation.
    fn reset(&mut self, cols: usize, template: &Cell) {
        let blank = Cell::blank(template);
        if self.cells.len() == cols {
            // The cells after `occupied` are alike, so the last one tells
            // whether they are blank already.
            let end = if self.cells.last() == Some(&blank) {
                self.occupied
            } else {
                cols
            };
            self.cells[..end].fill(blank);
        } else {
            self.cells.clear();
            self.cells.resize(cols, blank);
        }
        self.occupied = 0;
        self.wrapped = false;
        self.mark = None;
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
        self.occupied = cols;
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
            .filter(|c| !c.flags.intersects(Flags::WIDE_SPACER | Flags::WRAP_GAP))
            .map(|c| c.c)
            .collect();
        s.truncate(s.trim_end_matches(' ').len());
        s
    }
}

/// One screen buffer: the visible rows plus scrollback history.
#[derive(Debug, Clone)]
pub struct Grid {
    pub rows: VecDeque<Row>,
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

    /// Adds `row` to the history. Returns the row that no longer fits (the
    /// oldest, or `row` itself without history), for reuse.
    pub fn push_history(&mut self, row: Row) -> Option<Row> {
        self.scrolled += 1;
        if self.max_history == 0 {
            return Some(row);
        }
        let dropped = if self.history.len() == self.max_history {
            self.history.pop_front()
        } else {
            None
        };
        self.history.push_back(row);
        dropped
    }

    /// A blank row, made from `spare` when there is one.
    fn blank_row(&self, spare: Option<Row>, template: &Cell) -> Row {
        match spare {
            Some(mut row) => {
                row.reset(self.cols, template);
                row
            }
            None => Row::new(self.cols, template),
        }
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
            // Every line of output scrolls the screen once it is full, so
            // the row that leaves is reused instead of freed and allocated.
            // At the top of the screen, as nearly always, this takes no
            // shifting: the rows are a ring.
            let row = self.rows.remove(top).expect("the region is on screen");
            let spare = if keep {
                self.push_history(row)
            } else {
                Some(row)
            };
            let blank = self.blank_row(spare, template);
            self.rows.insert(bottom, blank);
        }
        n
    }

    /// Scrolls rows `top..=bottom` down by `n`, filling the top with blank rows.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, n: usize, template: &Cell) {
        let n = n.min(bottom + 1 - top);
        for _ in 0..n {
            let row = self.rows.remove(bottom).expect("the region is on screen");
            let blank = self.blank_row(Some(row), template);
            self.rows.insert(top, blank);
        }
    }

    /// Resizes without reflowing. When shrinking vertically, `keep_row` (the
    /// cursor row) stays on screen by moving rows above it into history;
    /// when growing, rows come back from the history.
    /// Returns how many rows the old screen moved down (up when negative).
    pub fn resize(&mut self, cols: usize, rows: usize, keep_row: usize) -> isize {
        self.cols = cols;
        for row in &mut self.rows {
            row.resize(cols);
        }
        let mut moved = 0;
        if rows < self.rows.len() {
            let excess = (keep_row + 1).saturating_sub(rows);
            for row in self.rows.drain(..excess).collect::<Vec<_>>() {
                // Resizing is rare; the dropped row isn't worth keeping.
                let _ = self.push_history(row);
                moved -= 1;
            }
            self.rows.truncate(rows);
        }
        while self.rows.len() < rows {
            match self.history.pop_back() {
                Some(mut row) => {
                    row.resize(cols);
                    self.rows.insert(0, row);
                    self.scrolled = self.scrolled.saturating_sub(1);
                    moved += 1;
                }
                None => self.rows.push_back(Row::new(cols, &Cell::default())),
            }
        }
        moved
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
                && line
                    .cells
                    .last()
                    .is_some_and(|c| c.flags.contains(Flags::WRAP_GAP))
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
            let mut next = 0;
            for (cell, &pos) in line.cells.iter().zip(&layout.positions) {
                if pos > next {
                    let gap = pos - 1;
                    out[first + gap / cols]
                        .cell_mut(gap % cols)
                        .flags
                        .insert(Flags::WRAP_GAP);
                }
                *out[first + pos / cols].cell_mut(pos % cols) = *cell;
                next = pos + 1;
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
        self.rows = screen.into();
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
            for (cell, c) in row.cells_mut().iter_mut().zip(line.chars()) {
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
            grid.rows[0].cell_mut(0).c = c;
            grid.scroll_up(0, 0, 1, &Cell::default(), true);
        }
        let history: Vec<_> = grid.history.iter().map(Row::text).collect();
        assert_eq!(history, ["b", "c"]);
    }

    #[test]
    fn reused_rows_come_back_blank() {
        let template = Cell {
            bg: Color::Indexed(4),
            ..Cell::default()
        };
        // A full history and no history both hand back a used row.
        for max_history in [1, 0] {
            let mut grid = Grid::new(3, 1, max_history);
            for c in ['a', 'b', 'c'] {
                let row = &mut grid.rows[0];
                row.cell_mut(0).c = c;
                row.cell_mut(1).extra = 1;
                row.wrapped = true;
                row.mark = Some(Mark::Prompt);
                grid.scroll_up(0, 0, 1, &template, true);
                let row = &grid.rows[0];
                assert_eq!(row.cells, [Cell::blank(&template); 3]);
                assert!(!row.wrapped);
                assert_eq!(row.mark, None);
            }
        }
        let mut grid = grid_with_lines(&["a", "b"], 3);
        grid.rows[1].wrapped = true;
        grid.scroll_down(0, 1, 1, &template);
        assert_eq!(grid.rows[0].cells, [Cell::blank(&template); 3]);
        assert!(!grid.rows[0].wrapped);
    }

    #[test]
    fn reset_blanks_whatever_was_written() {
        let red = Cell {
            bg: Color::Indexed(1),
            ..Cell::default()
        };
        let blank = Cell::default();
        let mut row = Row::new(6, &blank);
        // Text at the start, then the rest erased in another color.
        row.cells_in(0..2).fill(Cell { c: 'x', ..blank });
        row.cells_in(2..6).fill(Cell::blank(&red));
        row.reset(6, &blank);
        assert_eq!(row.cells(), [blank; 6]);
        // A cell changed through `cells_mut`, past what was written.
        row.cells_in(0..1)[0].c = 'a';
        row.cells_mut()[4].c = 'b';
        row.reset(6, &blank);
        assert_eq!(row.cells(), [blank; 6]);
        // Blank in another color: every cell changes.
        row.cell_mut(0).c = 'c';
        row.reset(6, &red);
        assert_eq!(row.cells(), [Cell::blank(&red); 6]);
        row.reset(6, &blank);
        assert_eq!(row.cells(), [blank; 6]);
        // Resizing adds default cells after whatever the row held.
        let mut row = Row::new(2, &red);
        row.resize(4);
        row.reset(4, &red);
        assert_eq!(row.cells(), [Cell::blank(&red); 4]);
    }

    #[test]
    fn resize_keeps_cursor_row() {
        let mut grid = grid_with_lines(&["a", "b", "c", "d"], 4);
        let pushed = grid.resize(4, 2, 3);
        assert_eq!(pushed, -2);
        assert_eq!(texts(&grid), ["c", "d"]);
        assert_eq!(grid.history.len(), 2);

        grid.resize(2, 3, 0);
        assert_eq!(grid.num_rows(), 3);
        assert!(grid.rows.iter().all(|r| r.cells.len() == 2));
    }

    #[test]
    fn row_text_skips_spacers() {
        let mut row = Row::new(4, &Cell::default());
        row.cell_mut(0).c = '中';
        row.cell_mut(0).flags.insert(Flags::WIDE);
        row.cell_mut(1).flags.insert(Flags::WIDE_SPACER);
        row.cell_mut(2).c = 'x';
        assert_eq!(row.text(), "中x");
    }

    #[test]
    fn blank_keeps_background_only() {
        let mut flags = Flags::BOLD;
        flags.set_underline(Underline::Curly);
        let template = Cell {
            c: 'x',
            fg: Color::Indexed(1),
            bg: Color::Indexed(2),
            flags,
            extra: 1,
        };
        let blank = Cell::blank(&template);
        assert_eq!(blank.extra, 0);
        assert_eq!(blank.c, ' ');
        assert_eq!(blank.fg, Color::Default);
        assert_eq!(blank.bg, Color::Indexed(2));
        assert_eq!(blank.flags, Flags::empty());
        assert_eq!(blank.flags.underline(), Underline::None);
    }

    #[test]
    fn underline_style_shares_flags_without_touching_them() {
        let mut flags = Flags::BOLD | Flags::WRAP_GAP;
        for style in [
            Underline::Single,
            Underline::Double,
            Underline::Curly,
            Underline::Dotted,
            Underline::Dashed,
            Underline::None,
        ] {
            flags.set_underline(style);
            assert_eq!(flags.underline(), style);
            assert!(flags.contains(Flags::BOLD | Flags::WRAP_GAP));
            assert!(!flags.intersects(Flags::WIDE | Flags::CONTINUATION));
        }
        assert_eq!(flags, Flags::BOLD | Flags::WRAP_GAP);
    }
}
