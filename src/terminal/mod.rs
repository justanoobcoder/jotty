//! Terminal emulation: turns the child's output into a grid of cells.
//!
//! [`Terminal`] owns the escape-sequence parser and a [`Screen`], which holds
//! all state and implements the control functions (a practical subset of
//! xterm's, enough for shells, editors, pagers and tmux).

pub mod grid;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use vte::{Params, Perform};

pub use grid::{Cell, Color, Flags, Grid, Mark, Row, Underline};

pub struct Terminal {
    parser: vte::Parser,
    screen: Screen,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Terminal {
            parser: vte::Parser::new(),
            screen: Screen::new(cols, rows, scrollback),
        }
    }

    /// Processes output from the child process.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn screen_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Charset {
    #[default]
    Ascii,
    /// DEC special graphics, used for line drawing.
    LineDrawing,
}

impl Charset {
    fn map(self, c: char) -> char {
        match self {
            Charset::Ascii => c,
            Charset::LineDrawing => match c {
                '`' => '◆',
                'a' => '▒',
                'b' => '␉',
                'c' => '␌',
                'd' => '␍',
                'e' => '␊',
                'f' => '°',
                'g' => '±',
                'h' => '␤',
                'i' => '␋',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                _ => c,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Cursor {
    row: usize,
    col: usize,
    /// Attributes applied to newly printed characters.
    template: Cell,
    /// The last column was written; the next printable character wraps first.
    pending_wrap: bool,
    charsets: [Charset; 2],
    active_charset: usize,
}

#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    cursor: Cursor,
    origin: bool,
}

/// Which pointer events the program asked to receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    /// `?9`: button presses only.
    X10,
    /// `?1000`: presses and releases, including the wheel.
    Normal,
    /// `?1002`: also motion while a button is held.
    ButtonMotion,
    /// `?1003`: also motion without a button.
    AnyMotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub show_cursor: bool,
    pub mouse: MouseMode,
    /// `?1006`: report pointer events in the SGR encoding.
    pub mouse_sgr: bool,
    /// `?2026`: the program is in the middle of an update; hold drawing
    /// until it ends.
    pub synchronized: bool,
    autowrap: bool,
    origin: bool,
    insert: bool,
    linefeed_newline: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            app_keypad: false,
            bracketed_paste: false,
            focus_events: false,
            mouse: MouseMode::Off,
            mouse_sgr: false,
            synchronized: false,
            show_cursor: true,
            autowrap: true,
            origin: false,
            insert: false,
            linefeed_newline: false,
        }
    }
}

pub struct Screen {
    cols: usize,
    rows: usize,
    primary: Grid,
    alternate: Grid,
    alt_active: bool,
    cursor: Cursor,
    /// Saved cursors for the primary and alternate screen.
    saved: [Option<SavedCursor>; 2],
    scroll_top: usize,
    scroll_bottom: usize,
    tabs: Vec<bool>,
    pub modes: Modes,
    pub cursor_shape: CursorShape,
    last_char: Option<char>,
    /// Cursor color set by the program (OSC 12), overriding the configured one.
    cursor_color: Option<(u8, u8, u8)>,
    /// The configured cursor color, reported by OSC 12 queries.
    default_cursor_color: (u8, u8, u8),
    /// Colors set by the program with OSC 4, 10 and 11.
    colors: ColorOverrides,
    /// Hyperlink targets (OSC 8); a cell's `link` is an index + 1.
    links: Vec<String>,
    /// The configured colors, reported by OSC 4, 10 and 11 queries.
    default_colors: DefaultColors,
    /// Where the last character was printed, if nothing else happened
    /// since; a following variation selector applies to it.
    last_printed: Option<(usize, usize)>,
    title: Option<String>,
    title_changed: bool,
    responses: Vec<u8>,
    /// Data of a DECRQSS request (`DCS $ q … ST`) being received.
    decrqss: Option<Vec<u8>>,
    bell: bool,
    dirty: bool,
    /// How many history rows the view is scrolled back.
    display_offset: usize,
    /// Where the shell's prompt ended (OSC 133;B), as (row, column).
    prompt_end: Option<(usize, usize)>,
}

pub type Rgb = (u8, u8, u8);

/// Colors a program changed with OSC 4 (palette), 10 (foreground) and 11
/// (background), until it resets them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorOverrides {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub palette: [Option<Rgb>; 256],
}

impl Default for ColorOverrides {
    fn default() -> Self {
        ColorOverrides {
            foreground: None,
            background: None,
            palette: [None; 256],
        }
    }
}

impl ColorOverrides {
    pub fn is_empty(&self) -> bool {
        *self == ColorOverrides::default()
    }
}

/// The configured colors, which color queries report while the program
/// hasn't changed them.
#[derive(Debug, Clone)]
pub struct DefaultColors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub palette: [Rgb; 256],
}

impl Default for DefaultColors {
    fn default() -> Self {
        DefaultColors {
            foreground: (255, 255, 255),
            background: (0, 0, 0),
            cursor: (255, 255, 255),
            palette: [(0, 0, 0); 256],
        }
    }
}

/// VARIATION SELECTOR-16: display the preceding character as an emoji.
const EMOJI_PRESENTATION: char = '\u{FE0F}';

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0 && c != 0).collect()
}

impl Screen {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Screen {
            cols,
            rows,
            primary: Grid::new(cols, rows, scrollback),
            alternate: Grid::new(cols, rows, 0),
            alt_active: false,
            cursor: Cursor::default(),
            saved: [None, None],
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tabs: default_tabs(cols),
            modes: Modes::default(),
            cursor_shape: CursorShape::default(),
            last_char: None,
            cursor_color: None,
            default_cursor_color: (255, 255, 255),
            colors: ColorOverrides::default(),
            links: Vec::new(),
            default_colors: DefaultColors::default(),
            last_printed: None,
            title: None,
            title_changed: false,
            responses: Vec::new(),
            decrqss: None,
            bell: false,
            dirty: true,
            display_offset: 0,
            prompt_end: None,
        }
    }

    // ---- Public queries -------------------------------------------------

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn grid(&self) -> &Grid {
        if self.alt_active {
            &self.alternate
        } else {
            &self.primary
        }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_active {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    pub fn is_alt_screen(&self) -> bool {
        self.alt_active
    }

    /// The cursor position as (row, column), always within the screen.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col.min(self.cols - 1))
    }

    /// Whether the cursor should be drawn: shown and not scrolled out of view.
    pub fn cursor_visible(&self) -> bool {
        self.modes.show_cursor && self.display_offset == 0
    }

    /// The row displayed at screen line `line`, taking the scrollback view
    /// into account. Rows from history may be narrower or wider than the
    /// screen.
    pub fn visible_row(&self, line: usize) -> &Row {
        let grid = self.grid();
        let history = grid.history.len();
        let offset = self.display_offset.min(history);
        if line < offset {
            &grid.history[history - offset + line]
        } else {
            &grid.rows[line - offset]
        }
    }

    /// The row at screen line `line`: 0 is the top of the live screen,
    /// negative lines are in the history.
    pub fn row_at(&self, line: isize) -> Option<&Row> {
        let grid = self.grid();
        if line < 0 {
            let back = line.unsigned_abs();
            grid.history
                .len()
                .checked_sub(back)
                .map(|i| &grid.history[i])
        } else {
            grid.rows.get(line as usize)
        }
    }

    /// The lines of the command output that `line` belongs to, inclusive,
    /// found through the shell's OSC 133 marks. Trailing blank lines are
    /// left out. Without a `C` mark, the output is taken to start after the
    /// command line (`B`).
    pub fn command_output(&self, line: isize) -> Option<(isize, isize)> {
        let top = -(self.grid().history.len() as isize);
        let mark_at = |l: isize| self.row_at(l).and_then(|r| r.mark);
        let (marked, mark) = (top..=line)
            .rev()
            .find_map(|l| mark_at(l).map(|m| (l, m)))?;
        let first = match mark {
            Mark::Prompt => return None,
            Mark::Output { col: 0 } => marked,
            Mark::Output { .. } => marked + 1,
            Mark::Input => {
                let mut l = marked;
                while self.row_at(l).is_some_and(|r| r.wrapped) {
                    l += 1;
                }
                l + 1
            }
        };
        // The output ends before the next mark, or at the cursor while the
        // command is still running.
        let mut last = (line + 1..self.rows as isize)
            .find(|&l| mark_at(l).is_some())
            .map_or(self.cursor.row as isize, |l| l - 1);
        let blank = |l: isize| {
            self.row_at(l)
                .is_none_or(|r| r.cells.iter().all(|c| c.c == ' '))
        };
        while last >= first && blank(last) {
            last -= 1;
        }
        (first <= line && line <= last).then_some((first, last))
    }

    pub fn display_offset(&self) -> usize {
        self.display_offset
    }

    /// Scrolls the view into history by `delta` rows (positive is up).
    pub fn scroll_display(&mut self, delta: isize) {
        let max = self.grid().history.len();
        let new = self.display_offset.saturating_add_signed(delta).min(max);
        if new != self.display_offset {
            self.display_offset = new;
            self.dirty = true;
        }
    }

    pub fn reset_display(&mut self) {
        self.scroll_display(-(self.display_offset as isize));
    }

    /// Bytes the terminal must send back to the child (e.g. cursor reports).
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }

    /// The new window title, if it changed since the last call.
    pub fn take_title(&mut self) -> Option<String> {
        if std::mem::take(&mut self.title_changed) {
            self.title.clone()
        } else {
            None
        }
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    /// Whether anything visible changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The cursor color requested by the program with OSC 12, if any.
    pub fn cursor_color(&self) -> Option<(u8, u8, u8)> {
        self.cursor_color
    }

    /// The target of hyperlink `id` (a cell's `link`).
    pub fn link(&self, id: u16) -> Option<&str> {
        let index = usize::from(id).checked_sub(1)?;
        self.links.get(index).map(String::as_str)
    }

    /// Starts (`uri` non-empty) or ends a hyperlink (OSC 8). Identical
    /// targets share an id; when all ids are used, new links are ignored.
    fn set_link(&mut self, uri: &str) {
        self.cursor.template.link = if uri.is_empty() {
            0
        } else if let Some(i) = self.links.iter().rposition(|l| l == uri) {
            (i + 1) as u16
        } else if self.links.len() < usize::from(u16::MAX) {
            self.links.push(uri.to_string());
            self.links.len() as u16
        } else {
            log::debug!("too many hyperlinks; ignoring {uri}");
            0
        };
    }

    /// Colors the program set with OSC 4, 10 and 11.
    pub fn color_overrides(&self) -> &ColorOverrides {
        &self.colors
    }

    /// The configured colors, which color queries report while the program
    /// hasn't changed them.
    pub fn set_default_colors(&mut self, colors: DefaultColors) {
        self.default_cursor_color = colors.cursor;
        self.default_colors = colors;
    }

    /// The column where user input starts on the cursor row: after the
    /// prompt if the shell marked it with OSC 133, otherwise 0.
    pub fn input_start(&self) -> usize {
        match self.prompt_end {
            Some((row, col)) if row == self.cursor.row && !self.alt_active => col,
            _ => 0,
        }
    }

    // ---- Resizing -------------------------------------------------------

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }

        if cols != self.cols {
            self.reflow_primary(cols, rows);
        } else {
            let primary_row = if self.alt_active {
                self.saved[0].map_or(0, |s| s.cursor.row)
            } else {
                self.cursor.row
            };
            let pushed = self.primary.resize(cols, rows, primary_row);
            if !self.alt_active {
                self.cursor.row = self.cursor.row.saturating_sub(pushed);
            }
            self.prompt_end = self
                .prompt_end
                .and_then(|(r, c)| Some((r.checked_sub(pushed)?, c)));
        }
        // Full-screen programs redraw on resize; their screen is cut off
        // or padded, not reflowed.
        let alt_row = if self.alt_active { self.cursor.row } else { 0 };
        self.alternate.resize(cols, rows, alt_row);
        // The alternate screen has no history; rows pushed there are dropped.
        self.alternate.clear_history();
        self.prompt_end = self.prompt_end.filter(|&(r, c)| r < rows && c < cols);

        self.cols = cols;
        self.rows = rows;
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.pending_wrap = false;
        for saved in self.saved.iter_mut().flatten() {
            saved.cursor.row = saved.cursor.row.min(rows - 1);
            saved.cursor.col = saved.cursor.col.min(cols - 1);
        }
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.display_offset = 0;
        self.dirty = true;
    }

    /// Re-wraps the primary screen and its history to `cols` columns,
    /// moving the cursor, the saved cursor and the prompt marker with the
    /// text.
    fn reflow_primary(&mut self, cols: usize, rows: usize) {
        let mut points = Vec::new();
        if !self.alt_active {
            points.push((self.cursor.row, self.cursor.col));
        }
        let saved = self.saved[0].map(|s| s.cursor);
        points.extend(saved.map(|c| (c.row, c.col)));
        points.extend(self.prompt_end);
        self.primary.reflow(cols, rows, &mut points);

        let mut points = points.into_iter();
        if !self.alt_active {
            let (row, col) = points.next().unwrap();
            self.cursor.row = row;
            self.cursor.col = col;
        }
        if let Some(saved) = &mut self.saved[0] {
            let (row, col) = points.next().unwrap();
            saved.cursor.row = row;
            saved.cursor.col = col;
        }
        if self.prompt_end.is_some() {
            self.prompt_end = points.next();
        }
    }

    // ---- Cursor movement --------------------------------------------------

    fn goto(&mut self, row: usize, col: usize) {
        let (top, bottom) = if self.modes.origin {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows - 1)
        };
        self.cursor.row = (top + row).min(bottom);
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn move_up(&mut self, n: usize) {
        let top = if self.cursor.row >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        self.cursor.row = self.cursor.row.saturating_sub(n).max(top);
        self.cursor.pending_wrap = false;
    }

    fn move_down(&mut self, n: usize) {
        let bottom = if self.cursor.row <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.rows - 1
        };
        self.cursor.row = (self.cursor.row + n).min(bottom);
        self.cursor.pending_wrap = false;
    }

    fn move_forward(&mut self, n: usize) {
        self.cursor.col = (self.cursor.col + n).min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn move_backward(&mut self, n: usize) {
        self.cursor.col = self.cursor.col.min(self.cols - 1).saturating_sub(n);
        self.cursor.pending_wrap = false;
    }

    fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    fn linefeed(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row < self.rows - 1 {
            self.cursor.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    fn tab(&mut self, n: usize) {
        for _ in 0..n {
            let next = (self.cursor.col + 1..self.cols).find(|&c| self.tabs[c]);
            self.cursor.col = next.unwrap_or(self.cols - 1);
        }
        self.cursor.pending_wrap = false;
    }

    fn back_tab(&mut self, n: usize) {
        for _ in 0..n {
            let prev = (0..self.cursor.col).rev().find(|&c| self.tabs[c]);
            self.cursor.col = prev.unwrap_or(0);
        }
        self.cursor.pending_wrap = false;
    }

    // ---- Scrolling ----------------------------------------------------------

    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let template = self.cursor.template;
        let keep = top == 0 && !self.alt_active;
        let scrolled = self.grid_mut().scroll_up(top, bottom, n, &template, keep);
        if keep {
            self.prompt_end = self
                .prompt_end
                .and_then(|(r, c)| Some((r.checked_sub(scrolled)?, c)));
            if self.display_offset > 0 {
                // Keep the scrolled-back view still while output arrives.
                let max = self.primary.history.len();
                self.display_offset = (self.display_offset + scrolled).min(max);
            }
        }
        self.dirty = true;
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let template = self.cursor.template;
        self.grid_mut().scroll_down(top, bottom, n, &template);
        self.dirty = true;
    }

    // ---- Editing --------------------------------------------------------------

    /// Ensures column `col` of `row` is not half of a wide character, blanking
    /// the other half if it is.
    fn split_wide(&mut self, row: usize, col: usize) {
        let template = self.cursor.template;
        let cols = self.cols;
        let cells = &mut self.grid_mut().rows[row].cells;
        if col >= cells.len() {
            return;
        }
        let flags = cells[col].flags;
        if flags.contains(Flags::WIDE_SPACER) && col > 0 {
            cells[col - 1] = Cell::blank(&template);
        } else if flags.contains(Flags::WIDE) && col + 1 < cols {
            cells[col + 1] = Cell::blank(&template);
        }
    }

    /// Blanks columns `start..end` of `row`.
    fn erase_cells(&mut self, row: usize, start: usize, end: usize) {
        let end = end.min(self.cols);
        if start >= end {
            return;
        }
        self.split_wide(row, start);
        self.split_wide(row, end - 1);
        let blank = Cell::blank(&self.cursor.template);
        self.grid_mut().rows[row].cells[start..end].fill(blank);
        self.dirty = true;
    }

    fn erase_rows(&mut self, start: usize, end: usize) {
        for row in start..end {
            self.erase_cells(row, 0, self.cols);
            let row = &mut self.grid_mut().rows[row];
            row.wrapped = false;
            row.mark = None;
        }
    }

    fn insert_blanks(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col.min(self.cols - 1));
        let n = n.min(self.cols - col);
        self.split_wide(row, col);
        let blank = Cell::blank(&self.cursor.template);
        let cols = self.cols;
        let cells = &mut self.grid_mut().rows[row].cells;
        cells[col..].rotate_right(n);
        cells[col..col + n].fill(blank);
        if cells[cols - 1].flags.contains(Flags::WIDE) {
            cells[cols - 1] = blank;
        }
        self.dirty = true;
    }

    fn delete_chars(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col.min(self.cols - 1));
        let n = n.min(self.cols - col);
        self.split_wide(row, col);
        self.split_wide(row, col + n - 1);
        let blank = Cell::blank(&self.cursor.template);
        let cols = self.cols;
        let cells = &mut self.grid_mut().rows[row].cells;
        cells[col..].rotate_left(n);
        cells[cols - n..].fill(blank);
        self.cursor.pending_wrap = false;
        self.dirty = true;
    }

    fn insert_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let template = self.cursor.template;
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_down(row, bottom, n, &template);
        self.carriage_return();
        self.dirty = true;
    }

    fn delete_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let template = self.cursor.template;
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_up(row, bottom, n, &template, false);
        self.carriage_return();
        self.dirty = true;
    }

    fn print_char(&mut self, c: char) {
        let c = self.cursor.charsets[self.cursor.active_charset].map(c);
        if c == EMOJI_PRESENTATION {
            self.apply_emoji_presentation();
            return;
        }
        // Other zero-width characters (combining marks, ZWJ) are not
        // supported yet.
        let width = match c.width() {
            Some(w @ 1..=2) => w,
            _ => return,
        };

        if self.cursor.pending_wrap {
            self.wrap_line();
        }
        if width == 2 && self.cursor.col + 1 >= self.cols {
            if self.modes.autowrap && self.cols >= 2 {
                self.erase_cells(self.cursor.row, self.cursor.col, self.cols);
                self.wrap_line();
            } else if self.cols >= 2 {
                self.cursor.col = self.cols - 2;
            } else {
                return;
            }
        }
        if self.modes.insert {
            self.insert_blanks(width);
        }

        let (row, col) = (self.cursor.row, self.cursor.col);
        self.split_wide(row, col);
        if width == 2 {
            self.split_wide(row, col + 1);
        }
        let mut cell = self.cursor.template;
        cell.c = c;
        if width == 2 {
            cell.flags.insert(Flags::WIDE);
        }
        let cells = &mut self.grid_mut().rows[row].cells;
        cells[col] = cell;
        if width == 2 {
            cells[col + 1] = Cell {
                c: ' ',
                flags: cell.flags | Flags::WIDE_SPACER,
                ..cell
            };
            cells[col + 1].flags.remove(Flags::WIDE);
        }

        if col + width >= self.cols {
            self.cursor.col = self.cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.col += width;
        }
        self.last_char = Some(c);
        self.last_printed = Some((row, col));
        self.dirty = true;
    }

    /// Handles VS16 after a character that has an emoji form (like `❄` or
    /// `☀`): the pair is two cells wide, as in other terminals and in the
    /// width calculations of programs such as starship.
    fn apply_emoji_presentation(&mut self) {
        let Some((row, col)) = self.last_printed.take() else {
            return;
        };
        let cell = self.grid().rows[row].cells[col];
        let pair: String = [cell.c, EMOJI_PRESENTATION].into_iter().collect();
        if row != self.cursor.row
            || col + 1 >= self.cols
            || cell.flags.contains(Flags::WIDE)
            || pair.width() != 2
        {
            return;
        }
        self.split_wide(row, col + 1);
        let cells = &mut self.grid_mut().rows[row].cells;
        cells[col].flags.insert(Flags::WIDE);
        cells[col + 1] = Cell {
            c: ' ',
            flags: cell.flags | Flags::WIDE_SPACER,
            ..cell
        };
        if col + 2 >= self.cols {
            self.cursor.col = self.cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.col = col + 2;
        }
        self.dirty = true;
    }

    fn wrap_line(&mut self) {
        let row = self.cursor.row;
        self.grid_mut().rows[row].wrapped = true;
        self.carriage_return();
        self.linefeed();
    }

    // ---- Modes and state --------------------------------------------------

    fn save_cursor(&mut self) {
        self.saved[self.alt_active as usize] = Some(SavedCursor {
            cursor: self.cursor,
            origin: self.modes.origin,
        });
    }

    fn restore_cursor(&mut self) {
        match self.saved[self.alt_active as usize] {
            Some(saved) => {
                self.cursor = saved.cursor;
                self.modes.origin = saved.origin;
                self.cursor.row = self.cursor.row.min(self.rows - 1);
                self.cursor.col = self.cursor.col.min(self.cols - 1);
            }
            None => {
                self.cursor = Cursor::default();
                self.modes.origin = false;
            }
        }
    }

    fn set_alt_screen(&mut self, on: bool, clear: bool) {
        if on == self.alt_active {
            return;
        }
        self.alt_active = on;
        if on && clear {
            self.erase_rows(0, self.rows);
        }
        self.display_offset = 0;
        self.dirty = true;
    }

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.modes.app_cursor = on,
            6 => {
                self.modes.origin = on;
                self.goto(0, 0);
            }
            7 => self.modes.autowrap = on,
            25 => self.modes.show_cursor = on,
            47 => self.set_alt_screen(on, false),
            1047 => {
                if self.alt_active && !on {
                    self.erase_rows(0, self.rows);
                }
                self.set_alt_screen(on, false);
            }
            1048 => {
                if on {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => {
                if on {
                    self.save_cursor();
                    self.set_alt_screen(true, true);
                    // The alternate screen starts with the same cursor.
                    self.saved[1] = self.saved[0];
                } else {
                    self.set_alt_screen(false, false);
                    self.restore_cursor();
                }
            }
            9 | 1000 | 1002 | 1003 => {
                let mode = match mode {
                    9 => MouseMode::X10,
                    1000 => MouseMode::Normal,
                    1002 => MouseMode::ButtonMotion,
                    _ => MouseMode::AnyMotion,
                };
                if on {
                    self.modes.mouse = mode;
                } else if self.modes.mouse == mode {
                    self.modes.mouse = MouseMode::Off;
                }
            }
            1006 => self.modes.mouse_sgr = on,
            1004 => self.modes.focus_events = on,
            2004 => self.modes.bracketed_paste = on,
            2026 => self.modes.synchronized = on,
            _ => log::debug!("unhandled private mode {mode} = {on}"),
        }
        self.dirty = true;
    }

    /// Whether a private mode is set, for DECRQM; `None` if unsupported.
    fn private_mode(&self, mode: u16) -> Option<bool> {
        let m = &self.modes;
        Some(match mode {
            1 => m.app_cursor,
            6 => m.origin,
            7 => m.autowrap,
            25 => m.show_cursor,
            47 | 1047 | 1049 => self.alt_active,
            9 => m.mouse == MouseMode::X10,
            1000 => m.mouse == MouseMode::Normal,
            1002 => m.mouse == MouseMode::ButtonMotion,
            1003 => m.mouse == MouseMode::AnyMotion,
            1004 => m.focus_events,
            1006 => m.mouse_sgr,
            2004 => m.bracketed_paste,
            2026 => m.synchronized,
            _ => return None,
        })
    }

    /// Replies to a color query with the request's terminator, as xterm
    /// does. `code` is the OSC number with any index, such as `"4;1"`.
    fn report_color(&mut self, code: &str, (r, g, b): Rgb, bell_terminated: bool) {
        let end = if bell_terminated { "\x07" } else { "\x1b\\" };
        let reply = format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}{end}");
        self.responses.extend_from_slice(reply.as_bytes());
    }

    /// OSC 10, 11 and 12 (`which` 0, 1, 2): sets or queries the foreground,
    /// background or cursor color.
    fn dynamic_color(&mut self, which: usize, spec: &[u8], bell_terminated: bool) {
        let current = match which {
            0 => self
                .colors
                .foreground
                .unwrap_or(self.default_colors.foreground),
            1 => self
                .colors
                .background
                .unwrap_or(self.default_colors.background),
            _ => self.cursor_color.unwrap_or(self.default_cursor_color),
        };
        if spec == b"?" {
            self.report_color(&(10 + which).to_string(), current, bell_terminated);
            return;
        }
        let Some(rgb) = parse_color(spec) else {
            log::debug!("bad color {:?}", String::from_utf8_lossy(spec));
            return;
        };
        match which {
            0 => self.colors.foreground = Some(rgb),
            1 => self.colors.background = Some(rgb),
            _ => self.cursor_color = Some(rgb),
        }
        self.dirty = true;
    }

    /// Answers DECRQM (`CSI ? Ps $ p` or `CSI Ps $ p`).
    fn report_mode(&mut self, mode: u16, private: bool) {
        let state = if private {
            self.private_mode(mode)
        } else {
            match mode {
                4 => Some(self.modes.insert),
                20 => Some(self.modes.linefeed_newline),
                _ => None,
            }
        };
        // 1: set, 2: reset, 0: not recognized.
        let value = state.map_or(0, |on| if on { 1 } else { 2 });
        let prefix = if private { "?" } else { "" };
        let reply = format!("\x1b[{prefix}{mode};{value}$y");
        self.responses.extend_from_slice(reply.as_bytes());
    }

    fn set_mode(&mut self, mode: u16, on: bool) {
        match mode {
            4 => self.modes.insert = on,
            20 => self.modes.linefeed_newline = on,
            _ => log::debug!("unhandled mode {mode} = {on}"),
        }
    }

    fn reset(&mut self) {
        let scrollback = self.primary.max_history;
        let default_colors = self.default_colors.clone();
        *self = Screen::new(self.cols, self.rows, scrollback);
        self.set_default_colors(default_colors);
    }

    /// The current attributes as SGR parameters, for DECRQSS replies.
    fn sgr_report(&self) -> String {
        let t = &self.cursor.template;
        let mut parts = vec!["0".to_string()];
        for (flag, code) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::BLINK, "5"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKE, "9"),
        ] {
            if t.flags.contains(flag) {
                parts.push(code.into());
            }
        }
        match t.underline {
            Underline::None => {}
            Underline::Single => parts.push("4".into()),
            Underline::Double => parts.push("4:2".into()),
            Underline::Curly => parts.push("4:3".into()),
            Underline::Dotted => parts.push("4:4".into()),
            Underline::Dashed => parts.push("4:5".into()),
        }
        let color = |color: Color, base: u16, bright: u16, extended: u16| match color {
            Color::Default => None,
            Color::Indexed(i @ 0..=7) if base > 0 => Some(format!("{}", base + u16::from(i))),
            Color::Indexed(i @ 8..=15) if bright > 0 => {
                Some(format!("{}", bright + u16::from(i) - 8))
            }
            Color::Indexed(i) => Some(format!("{extended}:5:{i}")),
            Color::Rgb(r, g, b) => Some(format!("{extended}:2::{r}:{g}:{b}")),
        };
        parts.extend(color(t.fg, 30, 90, 38));
        parts.extend(color(t.bg, 40, 100, 48));
        parts.extend(color(t.underline_color, 0, 0, 58));
        parts.join(";")
    }

    /// Answers a DECRQSS request for the setting named by `request`.
    fn report_setting(&mut self, request: &[u8]) {
        let value = match request {
            b"m" => Some(format!("{}m", self.sgr_report())),
            b"r" => Some(format!(
                "{};{}r",
                self.scroll_top + 1,
                self.scroll_bottom + 1
            )),
            b" q" => {
                let style = match self.cursor_shape {
                    CursorShape::Block => 2,
                    CursorShape::Underline => 4,
                    CursorShape::Beam => 6,
                };
                Some(format!("{style} q"))
            }
            _ => None,
        };
        let reply = match value {
            Some(value) => format!("\x1bP1$r{value}\x1b\\"),
            None => "\x1bP0$r\x1b\\".to_string(),
        };
        self.responses.extend_from_slice(reply.as_bytes());
    }

    fn sgr(&mut self, params: &Params) {
        let t = &mut self.cursor.template;
        // A hyperlink (OSC 8) is not an attribute; resets keep it.
        let reset = Cell {
            link: t.link,
            ..Cell::default()
        };
        if params.is_empty() {
            *t = reset;
            return;
        }
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            match param[0] {
                0 => *t = reset,
                1 => t.flags.insert(Flags::BOLD),
                2 => t.flags.insert(Flags::DIM),
                3 => t.flags.insert(Flags::ITALIC),
                4 => {
                    t.underline = match param.get(1) {
                        Some(0) => Underline::None,
                        Some(2) => Underline::Double,
                        Some(3) => Underline::Curly,
                        Some(4) => Underline::Dotted,
                        Some(5) => Underline::Dashed,
                        _ => Underline::Single,
                    }
                }
                5 | 6 => t.flags.insert(Flags::BLINK),
                7 => t.flags.insert(Flags::INVERSE),
                8 => t.flags.insert(Flags::HIDDEN),
                9 => t.flags.insert(Flags::STRIKE),
                21 => t.underline = Underline::Double,
                22 => {
                    t.flags.remove(Flags::BOLD);
                    t.flags.remove(Flags::DIM);
                }
                23 => t.flags.remove(Flags::ITALIC),
                24 => t.underline = Underline::None,
                25 => t.flags.remove(Flags::BLINK),
                27 => t.flags.remove(Flags::INVERSE),
                28 => t.flags.remove(Flags::HIDDEN),
                29 => t.flags.remove(Flags::STRIKE),
                n @ 30..=37 => t.fg = Color::Indexed((n - 30) as u8),
                38 => {
                    if let Some(color) = extended_color(param, &mut iter) {
                        t.fg = color;
                    }
                }
                39 => t.fg = Color::Default,
                n @ 40..=47 => t.bg = Color::Indexed((n - 40) as u8),
                48 => {
                    if let Some(color) = extended_color(param, &mut iter) {
                        t.bg = color;
                    }
                }
                49 => t.bg = Color::Default,
                58 => {
                    if let Some(color) = extended_color(param, &mut iter) {
                        t.underline_color = color;
                    }
                }
                59 => t.underline_color = Color::Default,
                n @ 90..=97 => t.fg = Color::Indexed((n - 90 + 8) as u8),
                n @ 100..=107 => t.bg = Color::Indexed((n - 100 + 8) as u8),
                n => log::debug!("unhandled SGR {n}"),
            }
        }
    }
}

/// Parses an X11 color specification as used by OSC color sequences:
/// `#rgb`, `#rrggbb`, or `rgb:r/g/b` with 1–4 hex digits per component.
fn parse_color(spec: &[u8]) -> Option<(u8, u8, u8)> {
    let spec = std::str::from_utf8(spec).ok()?;
    let scale = |hex: &str| -> Option<u8> {
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len())) - 1;
        Some((value * 255 / max) as u8)
    };
    if let Some(hex) = spec.strip_prefix('#') {
        if !matches!(hex.len(), 3 | 6) || !hex.is_ascii() {
            return None;
        }
        let n = hex.len() / 3;
        return Some((
            scale(&hex[..n])?,
            scale(&hex[n..2 * n])?,
            scale(&hex[2 * n..])?,
        ));
    }
    let mut parts = spec.strip_prefix("rgb:")?.split('/');
    let rgb = (
        scale(parts.next()?)?,
        scale(parts.next()?)?,
        scale(parts.next()?)?,
    );
    parts.next().is_none().then_some(rgb)
}

/// Parses the arguments of SGR 38/48/58 in either the colon form
/// (`38:2::r:g:b`, `38:5:n`) or the semicolon form (`38;2;r;g;b`, `38;5;n`).
fn extended_color<'a>(param: &[u16], iter: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    let byte = |v: u16| v.min(255) as u8;
    if param.len() > 1 {
        return match param[1] {
            5 => param.get(2).map(|&n| Color::Indexed(byte(n))),
            2 if param.len() >= 5 => {
                let rgb = &param[param.len() - 3..];
                Some(Color::Rgb(byte(rgb[0]), byte(rgb[1]), byte(rgb[2])))
            }
            _ => None,
        };
    }
    match iter.next()?[0] {
        5 => Some(Color::Indexed(byte(iter.next()?[0]))),
        2 => {
            let r = iter.next()?[0];
            let g = iter.next()?[0];
            let b = iter.next()?[0];
            Some(Color::Rgb(byte(r), byte(g), byte(b)))
        }
        _ => None,
    }
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        self.print_char(c);
    }

    fn execute(&mut self, byte: u8) {
        self.last_printed = None;
        match byte {
            0x07 => self.bell = true,
            0x08 => self.move_backward(1),
            0x09 => self.tab(1),
            0x0a..=0x0c => {
                if self.modes.linefeed_newline {
                    self.carriage_return();
                }
                self.linefeed();
            }
            0x0d => self.carriage_return(),
            0x0e => self.cursor.active_charset = 1,
            0x0f => self.cursor.active_charset = 0,
            _ => {}
        }
        self.dirty = true;
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.last_printed = None;
        let args: Vec<u16> = params.iter().map(|p| p[0]).collect();
        // Argument `i`, where 0 or absent means `default`.
        let arg = |i: usize, default: usize| match args.get(i) {
            Some(&0) | None => default,
            Some(&n) => n as usize,
        };
        // Argument `i`, where absent means 0.
        let raw = |i: usize| args.get(i).copied().unwrap_or(0);

        match (intermediates, action) {
            ([], 'A') => self.move_up(arg(0, 1)),
            ([], 'B' | 'e') => self.move_down(arg(0, 1)),
            ([], 'C' | 'a') => self.move_forward(arg(0, 1)),
            ([], 'D') => self.move_backward(arg(0, 1)),
            ([], 'E') => {
                self.move_down(arg(0, 1));
                self.carriage_return();
            }
            ([], 'F') => {
                self.move_up(arg(0, 1));
                self.carriage_return();
            }
            ([], 'G' | '`') => {
                self.cursor.col = (arg(0, 1) - 1).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            ([], 'H' | 'f') => self.goto(arg(0, 1) - 1, arg(1, 1) - 1),
            ([], 'd') => {
                let col = self.cursor.col;
                self.goto(arg(0, 1) - 1, col);
            }
            ([], 'I') => self.tab(arg(0, 1)),
            ([], 'Z') => self.back_tab(arg(0, 1)),
            ([] | [b'?'], 'J') => {
                let (row, col) = (self.cursor.row, self.cursor.col);
                match raw(0) {
                    0 => {
                        self.erase_cells(row, col, self.cols);
                        self.erase_rows(row + 1, self.rows);
                    }
                    1 => {
                        self.erase_rows(0, row);
                        self.erase_cells(row, 0, col + 1);
                    }
                    2 => self.erase_rows(0, self.rows),
                    3 => {
                        self.grid_mut().clear_history();
                        self.display_offset = 0;
                    }
                    _ => {}
                }
            }
            ([] | [b'?'], 'K') => {
                let (row, col) = (self.cursor.row, self.cursor.col);
                match raw(0) {
                    0 => self.erase_cells(row, col, self.cols),
                    1 => self.erase_cells(row, 0, col + 1),
                    2 => self.erase_cells(row, 0, self.cols),
                    _ => {}
                }
            }
            ([], 'X') => {
                let (row, col) = (self.cursor.row, self.cursor.col);
                self.erase_cells(row, col, col + arg(0, 1));
            }
            ([], '@') => self.insert_blanks(arg(0, 1)),
            ([], 'P') => self.delete_chars(arg(0, 1)),
            ([], 'L') => self.insert_lines(arg(0, 1)),
            ([], 'M') => self.delete_lines(arg(0, 1)),
            ([], 'S') => self.scroll_up(arg(0, 1)),
            ([], 'T') => self.scroll_down(arg(0, 1)),
            ([], 'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..arg(0, 1).min(self.cols * self.rows) {
                        self.print_char(c);
                    }
                }
            }
            ([], 'c') if raw(0) == 0 => self.responses.extend_from_slice(b"\x1b[?62;22c"),
            ([b'>'], 'c') if raw(0) == 0 => self.responses.extend_from_slice(b"\x1b[>1;10;0c"),
            ([], 'n') => match raw(0) {
                5 => self.responses.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let top = if self.modes.origin {
                        self.scroll_top
                    } else {
                        0
                    };
                    let (row, col) = self.cursor();
                    let report = format!("\x1b[{};{}R", row - top + 1, col + 1);
                    self.responses.extend_from_slice(report.as_bytes());
                }
                _ => {}
            },
            ([], 'g') => match raw(0) {
                0 => {
                    let col = self.cursor.col.min(self.cols - 1);
                    self.tabs[col] = false;
                }
                3 => self.tabs.fill(false),
                _ => {}
            },
            ([], 'h') => args.iter().for_each(|&m| self.set_mode(m, true)),
            ([], 'l') => args.iter().for_each(|&m| self.set_mode(m, false)),
            ([b'?'], 'h') => args.iter().for_each(|&m| self.set_private_mode(m, true)),
            ([b'?'], 'l') => args.iter().for_each(|&m| self.set_private_mode(m, false)),
            ([b'?', b'$'], 'p') => self.report_mode(raw(0), true),
            ([b'$'], 'p') => self.report_mode(raw(0), false),
            ([], 'm') => self.sgr(params),
            ([], 'r') => {
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows).min(self.rows) - 1;
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.goto(0, 0);
                }
            }
            ([], 's') => self.save_cursor(),
            ([], 'u') => self.restore_cursor(),
            ([b' '], 'q') => {
                self.cursor_shape = match raw(0) {
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Beam,
                    _ => CursorShape::Block,
                };
            }
            _ => log::debug!(
                "unhandled CSI {:?} {:?} {action}",
                args,
                String::from_utf8_lossy(intermediates)
            ),
        }
        self.dirty = true;
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.last_printed = None;
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.carriage_return();
                self.linefeed();
            }
            ([], b'M') => self.reverse_index(),
            ([], b'H') => {
                let col = self.cursor.col.min(self.cols - 1);
                self.tabs[col] = true;
            }
            ([], b'c') => self.reset(),
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([b'#'], b'8') => {
                for row in &mut self.grid_mut().rows {
                    for cell in &mut row.cells {
                        *cell = Cell {
                            c: 'E',
                            ..Cell::default()
                        };
                    }
                }
            }
            ([slot @ (b'(' | b')')], set) => {
                let charset = match set {
                    b'0' => Charset::LineDrawing,
                    _ => Charset::Ascii,
                };
                self.cursor.charsets[(*slot == b')') as usize] = charset;
            }
            _ => log::debug!(
                "unhandled ESC {:?} {}",
                String::from_utf8_lossy(intermediates),
                byte as char
            ),
        }
        self.dirty = true;
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.last_printed = None;
        if intermediates == b"$" && action == 'q' {
            self.decrqss = Some(Vec::new());
        }
    }

    fn put(&mut self, byte: u8) {
        if let Some(data) = &mut self.decrqss
            && data.len() < 16
        {
            data.push(byte);
        }
    }

    fn unhook(&mut self) {
        if let Some(request) = self.decrqss.take() {
            self.report_setting(&request);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        match params {
            // Each further argument applies to the next color, as in xterm:
            // `OSC 10 ; fg ; bg` also sets the background.
            [code @ (b"10" | b"11" | b"12"), specs @ ..] => {
                let first = (code[1] - b'0') as usize;
                for (i, spec) in specs.iter().enumerate().take(3 - first) {
                    self.dynamic_color(first + i, spec, bell_terminated);
                }
            }
            // OSC 8 ; params ; URI. The URI may itself contain ';'.
            [b"8", _, uri @ ..] => {
                let uri = uri
                    .iter()
                    .map(|p| String::from_utf8_lossy(p))
                    .collect::<Vec<_>>()
                    .join(";");
                self.set_link(&uri);
            }
            [b"110", ..] => {
                self.colors.foreground = None;
                self.dirty = true;
            }
            [b"111", ..] => {
                self.colors.background = None;
                self.dirty = true;
            }
            [b"112", ..] => {
                self.cursor_color = None;
                self.dirty = true;
            }
            [b"4", pairs @ ..] => {
                for [index, spec] in pairs.as_chunks::<2>().0 {
                    let Some(index) = std::str::from_utf8(index)
                        .ok()
                        .and_then(|s| s.parse::<u8>().ok())
                    else {
                        continue;
                    };
                    let i = usize::from(index);
                    if *spec == b"?" {
                        let rgb = self.colors.palette[i].unwrap_or(self.default_colors.palette[i]);
                        self.report_color(&format!("4;{index}"), rgb, bell_terminated);
                    } else if let Some(rgb) = parse_color(spec) {
                        self.colors.palette[i] = Some(rgb);
                        self.dirty = true;
                    }
                }
            }
            [b"104"] | [b"104", b""] => {
                self.colors.palette = [None; 256];
                self.dirty = true;
            }
            [b"104", indexes @ ..] => {
                for index in indexes {
                    if let Some(i) = std::str::from_utf8(index)
                        .ok()
                        .and_then(|s| s.parse::<u8>().ok())
                    {
                        self.colors.palette[usize::from(i)] = None;
                    }
                }
                self.dirty = true;
            }
            [b"0" | b"2", rest @ ..] => {
                let title = rest
                    .iter()
                    .map(|p| String::from_utf8_lossy(p))
                    .collect::<Vec<_>>()
                    .join(";");
                self.title = Some(title);
                self.title_changed = true;
            }
            [b"133", kind, ..] => {
                let (row, col) = self.cursor();
                let mark = match kind.first() {
                    Some(b'A') => Mark::Prompt,
                    Some(b'B') => Mark::Input,
                    Some(b'C') => Mark::Output { col },
                    Some(b'D') => {
                        self.prompt_end = None;
                        return;
                    }
                    _ => return,
                };
                self.prompt_end = (mark == Mark::Input).then_some((row, col));
                self.grid_mut().rows[row].mark = Some(mark);
            }
            _ => log::debug!(
                "unhandled OSC {:?}",
                params.first().map(|p| String::from_utf8_lossy(p))
            ),
        }
    }
}

#[cfg(test)]
mod tests;
