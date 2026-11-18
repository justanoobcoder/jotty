//! Cells, rows and the screen grid with its scrollback history.

use std::collections::VecDeque;
use std::ops::{BitOr, Range};

use crate::color::Rgb;

/// A cell's foreground, background or underline color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// The configured default foreground or background.
    #[default]
    Default,
    Indexed(u8),
    Rgb(Rgb),
}

/// A cell's attributes (SGR), layout flags and underline style, in one
/// `u16`.
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

    #[must_use]
    pub const fn empty() -> Self {
        Flags(0)
    }

    #[must_use]
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any of the flags in `other` is set.
    #[must_use]
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
    #[must_use]
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

    /// Sets the underline style, replacing the one before.
    pub fn set_underline(&mut self, underline: Underline) {
        let bits = (underline as u16) << Self::UNDERLINE_SHIFT;
        self.0 = (self.0 & !Self::UNDERLINE_MASK) | bits;
    }

    const UNDERLINE_SHIFT: u16 = 12;
    const UNDERLINE_MASK: u16 = 0b111 << Self::UNDERLINE_SHIFT;
}

impl BitOr for Flags {
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

const _: () = assert!(
    std::mem::size_of::<Cell>() == 16,
    "a Cell must stay 16 bytes"
);

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
    #[must_use]
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

/// One row of cells, with what the row knows about the line it is part of.
#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    /// The cells from here on are all alike: nothing wrote them since the
    /// row was made blank. A blank row is reset, and copied into the
    /// history, by touching only the cells before it, which matters because
    /// scrolling does both once per line of output.
    occupied: usize,
    /// The row's width in columns. Cells past the stored ones are
    /// `Cell::default()`.
    width: usize,
    /// The line continues on the next row (it was soft-wrapped).
    pub wrapped: bool,
    /// The last shell-integration mark set on this row.
    pub mark: Option<Mark>,
}

impl PartialEq for Row {
    fn eq(&self, other: &Row) -> bool {
        // `occupied` only says how much a reset has to write.
        self.cells == other.cells
            && self.width == other.width
            && self.wrapped == other.wrapped
            && self.mark == other.mark
    }
}

impl Eq for Row {}

impl Row {
    /// A blank row: `cols` cells with the colors of `template`.
    #[must_use]
    pub fn new(cols: usize, template: &Cell) -> Self {
        Row {
            cells: vec![Cell::blank(template); cols],
            occupied: 0,
            width: cols,
            wrapped: false,
            mark: None,
        }
    }

    /// The stored cells: the first ones of the row, up to its `width`.
    #[must_use]
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
        self.width = cols;
        self.wrapped = false;
        self.mark = None;
    }

    /// The row, to read it like a history row.
    #[must_use]
    pub fn view(&self) -> RowRef<'_> {
        RowRef {
            cells: &self.cells,
            width: self.width,
            wrapped: self.wrapped,
            mark: self.mark,
        }
    }

    /// How many cells the history keeps of the row: all but the
    /// `Cell::default()` cells at its end, which most rows mostly are. A
    /// soft-wrapped row is kept whole: its trailing blanks are text that
    /// reflow joins.
    #[must_use]
    fn trimmed_len(&self) -> usize {
        let blank = Cell::default();
        // The cells from `occupied` on are alike, so the last one tells
        // whether any of them are kept.
        if self.wrapped || self.cells.last().is_some_and(|c| *c != blank) {
            self.cells.len()
        } else {
            self.cells[..self.occupied.min(self.cells.len())]
                .iter()
                .rposition(|c| *c != blank)
                .map_or(0, |i| i + 1)
        }
    }

    /// Truncates or pads the row to `cols` cells, without rewrapping. A wide
    /// character cut in half is cleared.
    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
        self.occupied = cols;
        self.width = cols;
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
        self.view().text()
    }
}

/// A row to read, on the screen or in the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowRef<'a> {
    cells: &'a [Cell],
    width: usize,
    /// The line continues on the next row (it was soft-wrapped).
    pub wrapped: bool,
    /// The last shell-integration mark set on this row.
    pub mark: Option<Mark>,
}

impl<'a> RowRef<'a> {
    /// The stored cells: the first ones of the row, up to its `width`.
    /// Cells past them are `Cell::default()`.
    #[must_use]
    pub fn cells(&self) -> &'a [Cell] {
        self.cells
    }

    /// How many columns the row spans, which may be more than it stores
    /// cells for.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// A copy to put on the screen or reflow, with as many cells as stored.
    fn to_row(self) -> Row {
        Row {
            cells: self.cells.to_vec(),
            occupied: self.cells.len(),
            width: self.width,
            wrapped: self.wrapped,
            mark: self.mark,
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

/// Cells in a history page. A row never straddles two pages, so a page
/// wastes less than a row's cells at its end.
const PAGE_CELLS: usize = 8192;

/// The rows scrolled off the top of the screen, oldest first.
///
/// Rows are stored trimmed (see `Row::trimmed_len`), so their cells may end
/// before their width, and their cells are packed into pages shared by many
/// rows. Every line of output that scrolls the screen pushes a row here and,
/// once the history is full, drops the oldest one: neither allocates or
/// frees, but for a page filling up or emptying.
#[derive(Debug, Clone, Default)]
pub struct History {
    rows: VecDeque<Stored>,
    pages: VecDeque<Vec<Cell>>,
    /// Pages dropped from the front so far. `Stored::page` counts pages
    /// from the first one, so that it holds as pages are dropped.
    dropped_pages: usize,
    /// The last page emptied, kept to be filled again.
    spare: Option<Vec<Cell>>,
}

/// Where a history row's cells are, and what the row knows about its line.
#[derive(Debug, Clone, Copy)]
struct Stored {
    /// The page, counted from the first one ever (see
    /// `History::dropped_pages`). Rows without cells name the last page
    /// there was when they were pushed, so that pages never go up and
    /// down from one row to the next.
    page: usize,
    start: u32,
    len: u32,
    width: u32,
    wrapped: bool,
    mark: Option<Mark>,
}

impl PartialEq for History {
    fn eq(&self, other: &History) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for History {}

impl History {
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row `index` rows from the oldest one.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<RowRef<'_>> {
        self.rows.get(index).map(|stored| self.view(stored))
    }

    /// The rows, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = RowRef<'_>> + ExactSizeIterator {
        self.rows.iter().map(|stored| self.view(stored))
    }

    /// Every cell of every row, to change them in place.
    pub fn cells_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        // Only the first page holds cells of rows dropped already, before
        // those of the oldest row.
        let skip = self.rows.front().map_or(0, |stored| {
            if stored.page == self.dropped_pages {
                stored.start as usize
            } else {
                0
            }
        });
        self.pages
            .iter_mut()
            .enumerate()
            .flat_map(move |(i, page)| page[if i == 0 { skip } else { 0 }..].iter_mut())
    }

    fn view(&self, stored: &Stored) -> RowRef<'_> {
        let cells = if stored.len == 0 {
            // Its page may be gone.
            &[]
        } else {
            let start = stored.start as usize;
            &self.pages[stored.page - self.dropped_pages][start..start + stored.len as usize]
        };
        RowRef {
            cells,
            width: stored.width as usize,
            wrapped: stored.wrapped,
            mark: stored.mark,
        }
    }

    /// Adds a copy of `row` without the cells `Row::trimmed_len` leaves out.
    fn push_trimmed(&mut self, row: &Row) {
        let cells = &row.cells[..row.trimmed_len()];
        let fits = self
            .pages
            .back()
            .is_some_and(|page| page.capacity() - page.len() >= cells.len());
        if !fits && !cells.is_empty() {
            let page = match self.spare.take() {
                Some(page) if page.capacity() >= cells.len() => page,
                _ => Vec::with_capacity(PAGE_CELLS.max(cells.len())),
            };
            self.pages.push_back(page);
        }
        let page = self.dropped_pages + self.pages.len().saturating_sub(1);
        let start = self.pages.back().map_or(0, Vec::len);
        if let Some(last) = self.pages.back_mut()
            && !cells.is_empty()
        {
            // Within its capacity: the page never moves.
            last.extend_from_slice(cells);
        }
        self.rows.push_back(Stored {
            page,
            start: start as u32,
            len: cells.len() as u32,
            width: row.width as u32,
            wrapped: row.wrapped,
            mark: row.mark,
        });
    }

    /// Drops the oldest row, and the pages no row is on any more.
    fn pop_front(&mut self) {
        self.rows.pop_front();
        let first = self.rows.front().map(|stored| stored.page);
        while !self.pages.is_empty() && first.is_none_or(|page| self.dropped_pages < page) {
            if let Some(page) = self.pages.pop_front() {
                self.keep_spare(page);
            }
            self.dropped_pages += 1;
        }
    }

    /// Takes the newest row out.
    fn pop_back(&mut self) -> Option<Row> {
        let stored = self.rows.pop_back()?;
        let row = self.view(&stored).to_row();
        if stored.len > 0 {
            // The newest row's cells are the last ones on its page.
            self.pages[stored.page - self.dropped_pages].truncate(stored.start as usize);
            while self.pages.back().is_some_and(Vec::is_empty) {
                if let Some(page) = self.pages.pop_back() {
                    self.keep_spare(page);
                }
            }
        }
        Some(row)
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        while let Some(page) = self.pages.pop_front() {
            self.keep_spare(page);
        }
        self.dropped_pages = 0;
    }

    fn keep_spare(&mut self, mut page: Vec<Cell>) {
        page.clear();
        self.spare = Some(page);
    }
}

/// One screen buffer: the visible rows plus scrollback history.
#[derive(Debug, Clone)]
pub struct Grid {
    pub rows: VecDeque<Row>,
    pub history: History,
    pub max_history: usize,
    /// Rows scrolled off the top so far (kept in history or not), so that
    /// positions in the history can follow new output.
    pub scrolled: u64,
    cols: usize,
}

impl Grid {
    /// A blank grid with room for `max_history` rows of history.
    #[must_use]
    pub fn new(cols: usize, rows: usize, max_history: usize) -> Self {
        Grid {
            rows: (0..rows)
                .map(|_| Row::new(cols, &Cell::default()))
                .collect(),
            history: History::default(),
            max_history,
            scrolled: 0,
            cols,
        }
    }

    #[cfg(test)]
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    /// Adds a trimmed copy of `row` to the history, dropping the oldest row
    /// when it is full.
    pub fn push_history(&mut self, row: &Row) {
        self.scrolled += 1;
        if self.max_history == 0 {
            return;
        }
        if self.history.len() == self.max_history {
            self.history.pop_front();
        }
        self.history.push_trimmed(row);
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
            // the row that leaves is reused, still in the cache, while the
            // history gets a trimmed copy. At the top of the screen, as
            // nearly always, this takes no shifting: the rows are a ring.
            let mut row = self.rows.remove(top).expect("the region is on screen");
            if keep {
                self.push_history(&row);
            }
            row.reset(self.cols, template);
            self.rows.insert(bottom, row);
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
                self.push_history(&row);
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
        let mut old: Vec<Row> = self.history.iter().map(RowRef::to_row).collect();
        self.history.clear();
        old.extend(self.rows.drain(..keep));

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
        // point (the cursor). Rows below the screen then no longer fit
        // anywhere (the history is above it) and are dropped: the cursor
        // staying visible matters more, and content below it is rare.
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
        for row in out.iter().skip(drop) {
            self.history.push_trimmed(row);
        }
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
        grid.scroll_up(0, 2, 1, &Cell::default(), /* keep= */ true);
        assert_eq!(texts(&grid), ["b", "c", ""]);
        assert_eq!(grid.history.len(), 1);
        assert_eq!(grid.history.get(0).unwrap().text(), "a");
    }

    #[test]
    fn scroll_region_does_not_touch_history() {
        let mut grid = grid_with_lines(&["a", "b", "c", "d"], 4);
        grid.scroll_up(1, 2, 1, &Cell::default(), /* keep= */ false);
        assert_eq!(texts(&grid), ["a", "c", "", "d"]);
        assert!(grid.history.is_empty());
        grid.scroll_down(0, 3, 2, &Cell::default());
        assert_eq!(texts(&grid), ["", "", "a", "c"]);
    }

    #[test]
    fn history_rows_leave_out_trailing_blanks() {
        let mut grid = grid_with_lines(&["ab", ""], 6);
        grid.scroll_up(0, 1, 2, &Cell::default(), /* keep= */ true);
        assert_eq!(grid.history.get(0).unwrap().cells().len(), 2);
        assert_eq!(grid.history.get(0).unwrap().width(), 6);
        assert_eq!(grid.history.get(0).unwrap().text(), "ab");
        assert!(grid.history.get(1).unwrap().cells().is_empty());
        assert!(grid.rows.iter().all(|r| r.cells().len() == 6));
    }

    #[test]
    fn history_rows_keep_colored_and_wrapped_blanks() {
        let red = Cell {
            bg: Color::Indexed(1),
            ..Cell::default()
        };
        let mut grid = Grid::new(4, 2, 10);
        grid.rows[0] = Row::new(4, &red);
        grid.rows[0].cell_mut(0).c = 'a';
        grid.rows[1].cell_mut(0).c = 'b';
        grid.rows[1].wrapped = true;
        grid.scroll_up(0, 1, 2, &Cell::default(), /* keep= */ true);
        assert_eq!(
            grid.history.get(0).unwrap().cells(),
            [Cell { c: 'a', ..red }, red, red, red]
        );
        assert_eq!(grid.history.get(1).unwrap().cells().len(), 4);
    }

    #[test]
    fn rows_back_from_the_history_span_the_screen() {
        let mut grid = grid_with_lines(&["ab", "c"], 4);
        grid.scroll_up(0, 1, 1, &Cell::default(), /* keep= */ true);
        grid.resize(4, 3, 0);
        assert_eq!(texts(&grid), ["ab", "c", ""]);
        assert!(
            grid.rows
                .iter()
                .all(|r| r.cells().len() == 4 && r.width == 4)
        );
    }

    #[test]
    fn reflow_keeps_the_blanks_of_wrapped_history_rows() {
        let mut grid = grid_with_lines(&["ab  ", "cd"], 4);
        grid.rows[0].wrapped = true;
        grid.scroll_up(0, 1, 1, &Cell::default(), /* keep= */ true);
        grid.reflow(8, 2, &mut [(0, 0)]);
        assert_eq!(texts(&grid), ["ab  cd", ""]);
    }

    #[test]
    fn reflowed_history_is_trimmed() {
        let mut grid = grid_with_lines(&["ab", "cd", ""], 4);
        grid.scroll_up(0, 2, 2, &Cell::default(), /* keep= */ true);
        grid.reflow(8, 1, &mut [(0, 0)]);
        assert_eq!(grid.history.len(), 2);
        assert!(
            grid.history
                .iter()
                .all(|r| r.cells().len() == 2 && r.width() == 8)
        );
    }

    #[test]
    fn history_is_bounded() {
        let mut grid = Grid::new(2, 1, 2);
        for c in ['a', 'b', 'c'] {
            grid.rows[0].cell_mut(0).c = c;
            grid.scroll_up(0, 0, 1, &Cell::default(), /* keep= */ true);
        }
        let history: Vec<_> = grid.history.iter().map(|r| r.text()).collect();
        assert_eq!(history, ["b", "c"]);
    }

    /// A grid one row tall whose rows fill half a history page each.
    fn wide_grid(max_history: usize) -> Grid {
        Grid::new(PAGE_CELLS / 2, 1, max_history)
    }

    /// Scrolls a row starting with `c` into the history, or a blank one.
    fn scroll_out(grid: &mut Grid, c: Option<char>) {
        if let Some(c) = c {
            let cols = grid.cols;
            grid.rows[0].cell_mut(0).c = c;
            grid.rows[0].cell_mut(cols - 1).c = c;
        }
        grid.scroll_up(0, 0, 1, &Cell::default(), /* keep= */ true);
    }

    fn first_chars(grid: &Grid) -> String {
        grid.history
            .iter()
            .map(|r| r.cells().first().map_or('.', |c| c.c))
            .collect()
    }

    #[test]
    fn history_drops_pages_its_rows_left() {
        let mut grid = wide_grid(3);
        for c in "abcdefgh".chars() {
            scroll_out(&mut grid, Some(c));
        }
        assert_eq!(first_chars(&grid), "fgh");
        assert!(
            grid.history
                .iter()
                .all(|r| r.cells().len() == PAGE_CELLS / 2)
        );
        // Two rows a page: "f" shares its page with the dropped "e".
        assert_eq!(grid.history.pages.len(), 2);
        assert_eq!(grid.history.dropped_pages, 2);
    }

    #[test]
    fn history_keeps_rows_without_cells() {
        let mut grid = wide_grid(4);
        for c in [Some('a'), None, Some('b'), Some('c'), None, Some('d'), None] {
            scroll_out(&mut grid, c);
        }
        assert_eq!(first_chars(&grid), "c.d.");
        for _ in 0..4 {
            scroll_out(&mut grid, None);
        }
        assert_eq!(first_chars(&grid), "....");
        // The rows left name the page "d" was on, the only one kept.
        assert_eq!(grid.history.pages.len(), 1);
        scroll_out(&mut grid, Some('e'));
        assert_eq!(first_chars(&grid), "...e");
    }

    #[test]
    fn rows_come_back_from_history_pages_in_order() {
        let mut grid = wide_grid(10);
        for c in [Some('a'), Some('b'), None, Some('c'), Some('d')] {
            scroll_out(&mut grid, c);
        }
        grid.resize(PAGE_CELLS / 2, 4, 0);
        let firsts: String = grid.rows.iter().map(|r| r.cells[0].c).collect();
        assert_eq!(firsts, " cd ");
        assert_eq!(first_chars(&grid), "ab");
        // Pages emptied by rows coming back are not kept.
        assert_eq!(grid.history.pages.len(), 1);
        scroll_out(&mut grid, Some('e'));
        scroll_out(&mut grid, Some('f'));
        assert_eq!(first_chars(&grid), "abef");
    }

    #[test]
    fn history_cells_change_in_place() {
        let mut grid = wide_grid(2);
        for c in "abc".chars() {
            scroll_out(&mut grid, Some(c));
        }
        for cell in grid.history.cells_mut() {
            cell.c = cell.c.to_ascii_uppercase();
        }
        assert_eq!(first_chars(&grid), "BC");
        // Only the cells of rows still there are handed out.
        assert_eq!(grid.history.cells_mut().count(), PAGE_CELLS);
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
                grid.scroll_up(0, 0, 1, &template, /* keep= */ true);
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
