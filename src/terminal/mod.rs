//! Terminal emulation: turns the child's output into a grid of cells.
//!
//! [`Terminal`] owns the escape-sequence parser and a [`Screen`], which holds
//! all state and implements the control functions (a practical subset of
//! xterm's, enough for shells, editors, pagers and tmux).

pub mod graphics;
pub mod grid;
mod sixel;
mod strings;

use graphics::Graphics;
pub use grid::{Cell, Color, Extra, Flags, Grid, Mark, Row, Underline};
use strings::{StringTable, Table};
use unicode_segmentation::GraphemeCursor;
use vte::{Params, Perform};

use crate::color::Rgb;

/// The longest APC string kept; the rest of a longer one is dropped.
const MAX_APC: usize = 64 << 20;
/// The longest OSC string kept (vte drops the rest): room for long
/// hyperlinks and notifications, but an unterminated OSC can't grow
/// without bound.
const MAX_OSC_BYTES: usize = 16 << 10;

/// A terminal: the parser and the screen it drives.
pub struct Terminal {
    parser: vte::Parser<MAX_OSC_BYTES>,
    screen: Screen,
    apc: Apc,
}

/// Where the byte stream is relative to an APC string (`ESC _ … ESC \`),
/// which vte ignores; graphics commands come in them.
#[derive(Default)]
enum Apc {
    #[default]
    Outside,
    /// An `ESC` ended the last read.
    Escape,
    /// Inside, with the body so far (`None` when it grew too long).
    Body(Option<Vec<u8>>),
    /// Inside, after an `ESC`.
    BodyEscape(Option<Vec<u8>>),
}

/// The string tables that extras refer to, see `Screen::make_room`.
#[derive(Clone, Copy)]
enum Strings {
    Links,
    Clusters,
}

impl Terminal {
    #[must_use]
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Terminal {
            parser: vte::Parser::new_with_size(),
            screen: Screen::new(cols, rows, scrollback),
            apc: Apc::Outside,
        }
    }

    /// Processes output from the child process. APC strings are taken out
    /// before the parser sees the bytes.
    pub fn feed(&mut self, mut bytes: &[u8]) {
        while let Some(&first) = bytes.first() {
            match std::mem::take(&mut self.apc) {
                Apc::Outside => {
                    // The first `ESC _`, or an `ESC` at the end.
                    let start = memchr::memchr_iter(0x1b, bytes)
                        .find(|&i| bytes.get(i + 1).is_none_or(|&b| b == b'_'));
                    let Some(i) = start else {
                        self.parser.advance(&mut self.screen, bytes);
                        return;
                    };
                    self.parser.advance(&mut self.screen, &bytes[..i]);
                    if i + 1 == bytes.len() {
                        self.apc = Apc::Escape;
                        return;
                    }
                    self.begin_apc();
                    bytes = &bytes[i + 2..];
                }
                Apc::Escape if first == b'_' => {
                    self.begin_apc();
                    bytes = &bytes[1..];
                }
                Apc::Escape => self.parser.advance(&mut self.screen, b"\x1b"),
                Apc::Body(mut body) => {
                    let end = memchr::memchr3(0x1b, 0x18, 0x1a, bytes);
                    let part = &bytes[..end.unwrap_or(bytes.len())];
                    if body
                        .as_ref()
                        .is_some_and(|b| b.len() + part.len() > MAX_APC)
                    {
                        log::debug!("APC string too long; dropping it");
                        body = None;
                    }
                    if let Some(body) = &mut body {
                        body.extend_from_slice(part);
                    }
                    match end {
                        // CAN and SUB cancel the string.
                        Some(i) if bytes[i] != 0x1b => bytes = &bytes[i + 1..],
                        Some(i) => {
                            self.apc = Apc::BodyEscape(body);
                            bytes = &bytes[i + 1..];
                        }
                        None => {
                            self.apc = Apc::Body(body);
                            return;
                        }
                    }
                }
                Apc::BodyEscape(body) if first == b'\\' => {
                    if let Some(body) = body {
                        self.screen.apc_dispatch(&body);
                    }
                    bytes = &bytes[1..];
                }
                // Another string starts, cutting this one short.
                Apc::BodyEscape(_) if first == b'_' => {
                    self.begin_apc();
                    bytes = &bytes[1..];
                }
                // Another escape sequence cuts the string short.
                Apc::BodyEscape(_) => self.parser.advance(&mut self.screen, b"\x1b"),
            }
        }
    }

    /// Enters an APC string. Its `ESC` also ends whatever sequence the
    /// parser was in, which never gets to see it otherwise.
    fn begin_apc(&mut self) {
        self.parser.advance(&mut self.screen, b"\x1b\x18");
        self.apc = Apc::Body(Some(Vec::new()));
    }

    #[must_use]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn screen_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }
}

/// How the cursor is drawn (DECSCUSR).
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
    /// The last column was written; the next printable character wraps
    /// first if autowrap is on then, and overwrites that column if not.
    /// Autowrap is checked when wrapping, as in xterm, so turning it on
    /// after the column was written still wraps.
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

/// The modes a program sets with SM and DECSET.
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

/// All the state of a terminal: both screen buffers, the cursor, modes,
/// colors, and the replies and events waiting for the application.
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
    /// Scratch text for `continues_cluster`.
    cluster_scratch: String,
    /// Cursor color set by the program (OSC 12), overriding the configured one.
    cursor_color: Option<Rgb>,
    /// The configured cursor color, reported by OSC 12 queries.
    default_cursor_color: Rgb,
    /// Colors set by the program with OSC 4, 10 and 11.
    colors: ColorOverrides,
    /// Hyperlink targets (OSC 8), by `Extra::link`.
    links: StringTable,
    /// Grapheme cluster tails, by `Extra::cluster`.
    clusters: StringTable,
    /// What cells carry besides colors and flags, by `Cell::extra`.
    extras: Table<Extra>,
    /// The first cell of the grapheme cluster being printed, which later
    /// characters of the cluster join.
    cluster_start: Option<(usize, usize)>,
    /// The configured colors, reported by OSC 4, 10 and 11 queries.
    default_colors: DefaultColors,
    /// Where the last character was printed, if nothing else happened
    /// since; a following variation selector applies to it.
    last_printed: Option<(usize, usize)>,
    title: Option<String>,
    title_changed: bool,
    /// How many synchronized updates (mode 2026) have begun.
    synchronized_updates: u64,
    responses: Vec<u8>,
    /// Data of a DECRQSS request (`DCS $ q … ST`) being received.
    decrqss: Option<Vec<u8>>,
    /// The sixel image (`DCS q … ST`) being received.
    sixel: Option<Box<sixel::Decoder>>,
    bell: bool,
    /// Desktop notifications programs asked for (OSC 9, OSC 777), oldest
    /// first.
    notifications: Vec<Notification>,
    dirty: bool,
    /// How many history rows the view is scrolled back.
    display_offset: usize,
    /// Where the shell's prompt ended (OSC 133;B), as (row, column).
    prompt_end: Option<(usize, usize)>,
    /// Images of the primary and alternate screen (kitty graphics).
    graphics: [Graphics; 2],
    /// The cell size in pixels, for pixel size reports.
    cell_size: (u32, u32),
    /// Kitty keyboard protocol flags of the primary and alternate screen,
    /// with the flags saved by pushes.
    keyboard: [KeyboardFlags; 2],
}

/// The kitty keyboard protocol's progressive enhancement flags of a screen.
#[derive(Debug, Clone, Default)]
struct KeyboardFlags {
    current: u8,
    /// Flags saved by `CSI > u`, newest last.
    stack: Vec<u8>,
}

/// How many pushed keyboard flags a screen keeps.
const KEYBOARD_STACK: usize = 16;

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
    #[must_use]
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
            foreground: Rgb::new(255, 255, 255),
            background: Rgb::new(0, 0, 0),
            cursor: Rgb::new(255, 255, 255),
            palette: [Rgb::new(0, 0, 0); 256],
        }
    }
}

/// A desktop notification a program asked for (OSC 9 or OSC 777).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// OSC 9 has none; the window title stands in for it.
    pub title: Option<String>,
    pub body: String,
}

/// Notifications kept between two reads of the pty, so a flood of them
/// can't fill the desktop.
const MAX_PENDING_NOTIFICATIONS: usize = 4;
/// Characters kept of a notification's title and body.
const MAX_NOTIFICATION_TEXT: usize = 1024;
/// Characters kept of a window title (OSC 0/2).
const MAX_TITLE_CHARS: usize = 256;

fn truncate(mut s: String, max_chars: usize) -> String {
    if let Some((i, _)) = s.char_indices().nth(max_chars) {
        s.truncate(i);
    }
    s
}

/// How many characters a grapheme cluster keeps after its first one.
/// Real clusters (a family emoji, stacked Vietnamese marks) need far
/// fewer; more only come from floods of combining marks.
const MAX_CLUSTER_CHARS: usize = 32;

/// VARIATION SELECTOR-16: display the preceding character as an emoji.
const EMOJI_PRESENTATION: char = '\u{FE0F}';

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0 && c != 0).collect()
}

impl Screen {
    #[must_use]
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
            cluster_scratch: String::new(),
            cursor_color: None,
            default_cursor_color: Rgb::new(255, 255, 255),
            colors: ColorOverrides::default(),
            links: StringTable::default(),
            clusters: StringTable::default(),
            extras: Table::default(),
            cluster_start: None,
            default_colors: DefaultColors::default(),
            last_printed: None,
            title: None,
            title_changed: false,
            synchronized_updates: 0,
            responses: Vec::new(),
            decrqss: None,
            sixel: None,
            bell: false,
            notifications: Vec::new(),
            dirty: true,
            display_offset: 0,
            prompt_end: None,
            graphics: Default::default(),
            cell_size: (10, 20),
            keyboard: Default::default(),
        }
    }

    // ---- Public queries -------------------------------------------------

    #[must_use]
    pub fn cols(&self) -> usize {
        self.cols
    }

    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The grid on display: the alternate screen's while it is active.
    #[must_use]
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

    #[must_use]
    pub fn is_alt_screen(&self) -> bool {
        self.alt_active
    }

    /// The cursor position as (row, column), always within the screen.
    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col.min(self.cols - 1))
    }

    /// Whether the cursor should be drawn: shown and not scrolled out of view.
    #[must_use]
    pub fn cursor_visible(&self) -> bool {
        self.modes.show_cursor && self.display_offset == 0
    }

    /// The row displayed at screen line `line`, taking the scrollback view
    /// into account. Rows from history may be narrower or wider than the
    /// screen.
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
                .is_none_or(|r| r.cells().iter().all(|c| c.c == ' '))
        };
        while last >= first && blank(last) {
            last -= 1;
        }
        (first <= line && line <= last).then_some((first, last))
    }

    /// How many rows the view is scrolled back into the history.
    #[must_use]
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

    /// Scrolls the view so that `line` is visible, centering it if it
    /// wasn't.
    pub fn show_line(&mut self, line: isize) {
        let rows = self.rows as isize;
        let top = -(self.display_offset as isize);
        if (top..top + rows).contains(&line) {
            return;
        }
        let offset = (rows / 2 - line).clamp(0, self.grid().history.len() as isize);
        self.scroll_display(offset - self.display_offset as isize);
    }

    /// Scrolls the view back to the bottom.
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

    /// How many synchronized updates have begun, to tell them apart.
    #[must_use]
    pub fn synchronized_updates(&self) -> u64 {
        self.synchronized_updates
    }

    /// The title a program set (OSC 0/2), if any.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Whether the bell rang since the last call.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    /// Desktop notifications asked for since the last call.
    pub fn take_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.notifications)
    }

    fn notify(&mut self, title: Option<&[u8]>, body: &[&[u8]]) {
        if self.notifications.len() >= MAX_PENDING_NOTIFICATIONS {
            return;
        }
        let text = |s: &[u8]| {
            truncate(
                String::from_utf8_lossy(s).into_owned(),
                MAX_NOTIFICATION_TEXT,
            )
        };
        let body = text(&body.join(&b';'));
        let title = title.map(text).filter(|t| !t.is_empty());
        if body.is_empty() && title.is_none() {
            return;
        }
        self.notifications.push(Notification { title, body });
    }

    /// Whether anything visible changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The cursor color requested by the program with OSC 12, if any.
    #[must_use]
    pub fn cursor_color(&self) -> Option<Rgb> {
        self.cursor_color
    }

    /// What `cell` carries besides its character, colors and flags.
    #[must_use]
    pub fn extra(&self, cell: &Cell) -> Extra {
        self.extra_by_id(cell.extra)
    }

    fn extra_by_id(&self, id: u16) -> Extra {
        self.extras.get(id).copied().unwrap_or_default()
    }

    /// The characters after a cell's `c` in its grapheme cluster.
    pub fn cluster(&self, cell: &Cell) -> &str {
        if cell.extra == 0 {
            return "";
        }
        self.clusters
            .get(self.extra(cell).cluster)
            .map_or("", String::as_str)
    }

    /// A cell's whole text: its character and the rest of its cluster.
    /// Continuation cells have none; the cluster's first cell holds it.
    pub fn cell_text(&self, cell: &Cell, out: &mut String) {
        if !cell.flags.contains(Flags::CONTINUATION) {
            out.push(cell.c);
            out.push_str(self.cluster(cell));
        }
    }

    /// Whether `c` continues the grapheme cluster that `start` begins,
    /// rather than starting one of its own. Boundary rules look back but
    /// never ahead, so only the boundary before `c` can be new.
    fn continues_cluster(&mut self, start: &Cell, c: char) -> bool {
        // Reused: this runs for most non-ASCII characters printed.
        let mut text = std::mem::take(&mut self.cluster_scratch);
        text.clear();
        self.cell_text(start, &mut text);
        let before = text.len();
        text.push(c);
        let boundary =
            GraphemeCursor::new(before, text.len(), /* is_extended= */ true).is_boundary(&text, 0);
        self.cluster_scratch = text;
        // With the whole text given, no more context is ever asked for.
        boundary == Ok(false)
    }

    /// Whether `cell` is an ASCII character with nothing joined to it, which
    /// no ASCII character after it can join either (see `join_cluster`).
    fn is_lone_ascii(&self, cell: &Cell) -> bool {
        cell.c.is_ascii() && (cell.extra == 0 || self.extra(cell).cluster == 0)
    }

    /// The id of `extra`, adding it if new. Doesn't make room: `None` when
    /// the table is full; callers make room first (`make_room`), since that
    /// renumbers ids.
    fn intern_extra(&mut self, extra: &Extra) -> Option<u16> {
        if *extra == Extra::default() {
            return Some(0);
        }
        self.extras.insert(extra)
    }

    /// Drops unused strings and extras if adding `string` (when given)
    /// and then the extra that `extra` makes from its id won't fit
    /// otherwise. Only then: a collection scans the whole history.
    ///
    /// This renumbers ids, so callers read ids only after it, and add the
    /// string after it: it would drop a new string no cell refers to yet.
    fn make_room(&mut self, string: Option<(Strings, &str)>, extra: impl FnOnce(u16) -> Extra) {
        let id = match string {
            None => 0,
            Some((kind, s)) => {
                let table = match kind {
                    Strings::Links => &self.links,
                    Strings::Clusters => &self.clusters,
                };
                match table.find(s) {
                    Some(id) => id,
                    // A new string makes a new extra too.
                    None => {
                        if table.is_full() || self.extras.is_full() {
                            self.collect_strings();
                        }
                        return;
                    }
                }
            }
        };
        let extra = extra(id);
        if self.extras.is_full() && extra != Extra::default() && self.extras.find(&extra).is_none()
        {
            self.collect_strings();
        }
    }

    /// Changes the extra of the cursor's template (its link or underline
    /// color); new cells get it. Callers make room first; this doesn't, as
    /// they may have just added a string it would drop.
    ///
    /// If the table is still full of extras in use, the underline color is
    /// dropped to make it fit, and failing that the link: a link must still
    /// end when the program ends it. Only an underline color that can't be
    /// set leaves the template as it was.
    fn update_template_extra(&mut self, update: impl FnOnce(&mut Extra)) {
        let current = self.extra(&self.cursor.template);
        let mut wanted = current;
        update(&mut wanted);
        let plain = Extra {
            underline_color: Color::Default,
            ..wanted
        };
        let id = self
            .intern_extra(&wanted)
            .or_else(|| self.intern_extra(&plain))
            .or((wanted.link != current.link).then_some(0));
        if id.is_none_or(|id| self.extra_by_id(id) != wanted) {
            log::debug!("too many distinct links and underline colors; dropping one");
        }
        if let Some(id) = id {
            self.cursor.template.extra = id;
        }
    }

    /// Lowers the capacity of the string and extra tables, to fill them in
    /// tests.
    #[cfg(test)]
    pub(super) fn limit_tables(&mut self, capacity: usize) {
        self.links.set_capacity(capacity);
        self.clusters.set_capacity(capacity);
        self.extras.set_capacity(capacity);
    }

    /// Drops the hyperlinks, cluster tails and extras no cell refers to any
    /// more, renumbering the rest, so that the tables don't fill up over a
    /// long session.
    fn collect_strings(&mut self) {
        let mut extras = self.extras.unused();
        let mut mark = |cell: &Cell| extras[usize::from(cell.extra)] = true;
        for grid in [&self.primary, &self.alternate] {
            for row in grid.rows.iter().chain(&grid.history) {
                row.cells().iter().for_each(&mut mark);
            }
        }
        mark(&self.cursor.template);
        for saved in self.saved.iter().flatten() {
            mark(&saved.cursor.template);
        }
        let mut links = self.links.unused();
        let mut clusters = self.clusters.unused();
        let used_ids = extras.iter().enumerate().filter(|(_, used)| **used);
        for (id, _) in used_ids {
            if let Some(extra) = self.extras.get(id as u16) {
                links[usize::from(extra.link)] = true;
                clusters[usize::from(extra.cluster)] = true;
            }
        }
        let links = self.links.retain(&links);
        let clusters = self.clusters.retain(&clusters);
        // Distinct extras stay distinct: kept strings keep distinct ids.
        let extras = self.extras.retain_map(&extras, |extra| Extra {
            link: links[usize::from(extra.link)],
            cluster: clusters[usize::from(extra.cluster)],
            ..*extra
        });
        let remap = |cell: &mut Cell| cell.extra = extras[usize::from(cell.extra)];
        for grid in [&mut self.primary, &mut self.alternate] {
            for row in grid.rows.iter_mut().chain(&mut grid.history) {
                row.cells_mut().iter_mut().for_each(remap);
            }
        }
        remap(&mut self.cursor.template);
        for saved in self.saved.iter_mut().flatten() {
            remap(&mut saved.cursor.template);
        }
        self.dirty = true;
    }

    /// Adds `c` to the grapheme cluster being printed, if it continues it.
    /// Zero-width characters (combining marks, ZWJ, variation selectors)
    /// only join it. Others (the emoji after a ZWJ, a skin tone, the second
    /// letter of a flag) also take up their usual width, as programs that
    /// count with `wcwidth` expect, in continuation cells. Returns `false`
    /// when `c` starts a new cluster.
    fn join_cluster(&mut self, c: char, width: usize) -> bool {
        let Some((row, col)) = self.cluster_start else {
            return false;
        };
        let Some(&start) = self.grid().rows.get(row).and_then(|r| r.cells().get(col)) else {
            return false;
        };
        // The cell must still hold the cluster's start: its text is what
        // the tail is cut from below.
        if start.flags.contains(Flags::CONTINUATION) {
            return false;
        }
        // Two printable ASCII characters never form one cluster: none is
        // Extend, ZWJ, SpacingMark, Prepend, a regional indicator or Hangul.
        // This skips the segmentation below for nearly all output.
        if c.is_ascii() && self.is_lone_ascii(&start) {
            return false;
        }
        if !self.continues_cluster(&start, c) {
            return false;
        }
        let mut text = String::new();
        self.cell_text(&start, &mut text);
        let base = start.c.len_utf8();
        text.push(c);
        // Continuation cells must stay on the cluster's row, after its
        // first cell. Without autowrap the cursor stays on the last column,
        // which may be that cell; the character then overwrites it instead.
        let wraps = self.cursor.pending_wrap || self.cursor.col + width > self.cols;
        if width > 0 && (wraps || row != self.cursor.row || self.cursor.col <= col) {
            return false;
        }
        // `text` starts with the start cell's character, so this always
        // succeeds; should that ever break, the character starts a cluster
        // of its own instead of output crashing the terminal.
        let Some(tail) = text.get(base..) else {
            return false;
        };
        // Past the limit the character is still consumed, so that a flood of
        // combining marks costs constant time each, but it isn't kept.
        if tail.chars().count() <= MAX_CLUSTER_CHARS {
            let joined = self.extra(&start);
            self.make_room(Some((Strings::Clusters, tail)), |cluster| Extra {
                cluster,
                ..joined
            });
            let extra = match self.clusters.insert(tail) {
                Some(cluster) => {
                    let start = self.grid().rows[row].cells()[col];
                    let extra = Extra {
                        cluster,
                        ..self.extra(&start)
                    };
                    self.intern_extra(&extra)
                }
                None => None,
            };
            match extra {
                Some(id) => self.grid_mut().rows[row].cell_mut(col).extra = id,
                None => log::debug!("too many distinct grapheme clusters; dropping one"),
            }
        }
        self.dirty = true;
        if c == EMOJI_PRESENTATION {
            self.apply_emoji_presentation();
        }
        if width > 0 {
            self.print_cell(c, width, Flags::CONTINUATION);
        }
        true
    }

    /// The target of the hyperlink with `id`, as cells refer to it.
    pub fn link(&self, id: u16) -> Option<&str> {
        self.links.get(id).map(String::as_str)
    }

    /// Starts (`uri` non-empty) or ends a hyperlink (OSC 8). Identical
    /// targets share an id; when all ids are used by cells still on the
    /// screen or in the history, new links are ignored.
    fn set_link(&mut self, uri: &str) {
        // The current link ends first, so that a collection to make room
        // for the new one can drop it if no cell has it.
        let ended = Extra {
            link: 0,
            ..self.extra(&self.cursor.template)
        };
        self.make_room(None, |_| ended);
        self.update_template_extra(|extra| extra.link = 0);
        if uri.is_empty() {
            return;
        }
        let unlinked = self.extra(&self.cursor.template);
        self.make_room(Some((Strings::Links, uri)), |link| Extra {
            link,
            ..unlinked
        });
        match self.links.insert(uri) {
            Some(link) => self.update_template_extra(|extra| extra.link = link),
            None => log::debug!("too many hyperlinks; ignoring {uri}"),
        }
    }

    /// Colors the program set with OSC 4, 10 and 11.
    #[must_use]
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
    #[must_use]
    pub fn input_start(&self) -> usize {
        match self.prompt_end {
            Some((row, col)) if row == self.cursor.row && !self.alt_active => col,
            _ => 0,
        }
    }

    // ---- Graphics ---------------------------------------------------------

    /// Sets the cell size in pixels, which sizes images in cells.
    pub fn set_cell_size(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        self.cell_size = size;
        for graphics in &mut self.graphics {
            graphics.cell = size;
        }
        self.dirty = true;
    }

    /// The kitty keyboard protocol flags in effect (progressive
    /// enhancements; 0 for legacy key encoding).
    #[must_use]
    pub fn keyboard_flags(&self) -> u8 {
        self.keyboard[self.alt_active as usize].current
    }

    /// Handles the kitty keyboard protocol's `CSI … u` sequences: query
    /// (`?`), push (`>`), pop (`<`) and set (`=`).
    fn keyboard_mode(&mut self, marker: u8, args: &[u16]) {
        let flags = &mut self.keyboard[self.alt_active as usize];
        let arg = |i: usize| args.get(i).copied();
        match marker {
            b'?' => {
                let reply = format!("\x1b[?{}u", flags.current);
                self.responses.extend_from_slice(reply.as_bytes());
            }
            b'>' => {
                if flags.stack.len() == KEYBOARD_STACK {
                    flags.stack.remove(0);
                }
                flags.stack.push(flags.current);
                flags.current = arg(0).unwrap_or(0).min(31) as u8;
            }
            b'<' => {
                for _ in 0..arg(0).unwrap_or(1).max(1) {
                    flags.current = flags.stack.pop().unwrap_or(0);
                }
            }
            _ => {
                let value = arg(0).unwrap_or(0).min(31) as u8;
                flags.current = match arg(1).unwrap_or(1) {
                    2 => flags.current | value,
                    3 => flags.current & !value,
                    _ => value,
                };
            }
        }
    }

    /// The images of the screen being shown.
    #[must_use]
    pub fn graphics(&self) -> &Graphics {
        &self.graphics[self.alt_active as usize]
    }

    fn graphics_mut(&mut self) -> &mut Graphics {
        &mut self.graphics[self.alt_active as usize]
    }

    /// An image of either screen by its uid.
    #[must_use]
    pub fn image(&self, uid: u64) -> Option<&graphics::Image> {
        self.graphics.iter().find_map(|g| g.image(uid))
    }

    /// The line of the top screen row, the unit placements are anchored in.
    fn top_line(&self) -> i64 {
        self.grid().scrolled as i64
    }

    /// Handles an APC string; kitty graphics commands start with `G`.
    fn apc_dispatch(&mut self, body: &[u8]) {
        self.last_printed = None;
        self.cluster_start = None;
        let Some(command) = body.strip_prefix(b"G") else {
            return;
        };
        let ctx = self.graphics_context();
        let outcome = self.graphics_mut().command(command, &ctx);
        if let Some(response) = outcome.response {
            self.responses.extend_from_slice(response.as_bytes());
        }
        if let Some((cols, rows)) = outcome.moved {
            // Past the image: after its last column, on its last row.
            let col = self.cursor.col + cols as usize;
            let row = self.cursor.row + rows as usize - 1;
            if col >= self.cols {
                self.move_past_image(row + 1, 0);
            } else {
                self.move_past_image(row, col);
            }
        }
        self.dirty = true;
    }

    /// Shows a finished sixel image at the cursor and moves the cursor to
    /// its last row, in the column it started at, as xterm and foot do.
    fn place_sixel(&mut self, image: sixel::Image) {
        let ctx = self.graphics_context();
        let (_, rows) = self.graphics_mut().place_sixel(image, &ctx);
        self.move_past_image(self.cursor.row + rows as usize - 1, ctx.cursor.1);
        self.dirty = true;
    }

    /// The screen and the cursor, for placing images.
    fn graphics_context(&self) -> graphics::Context {
        let top = self.top_line();
        graphics::Context {
            top,
            rows: self.rows,
            cursor: (
                top + self.cursor.row as i64,
                self.cursor.col.min(self.cols - 1),
            ),
        }
    }

    /// Moves the cursor to `row` and `col` past an image, scrolling the
    /// region up when `row` is below it.
    fn move_past_image(&mut self, mut row: usize, col: usize) {
        if self.cursor.row <= self.scroll_bottom && row > self.scroll_bottom {
            self.scroll_up(row - self.scroll_bottom);
            row = self.scroll_bottom;
        }
        self.cursor.row = row.min(self.rows - 1);
        self.cursor.col = col;
        self.cursor.pending_wrap = false;
    }

    /// Text written to columns `start..end` of `row` replaces the sixel
    /// images there.
    fn erase_sixels(&mut self, row: usize, start: usize, end: usize) {
        // Printing runs this for every character.
        if !self.graphics().may_have_sixels() {
            return;
        }
        let line = self.top_line() + row as i64;
        self.graphics_mut().erase_sixels(line..line + 1, start..end);
    }

    // ---- Resizing -------------------------------------------------------

    /// Resizes both screens. The primary screen and its history reflow to the
    /// new width; the alternate screen is cut or padded.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }

        // Positions of the last printed character are stale.
        self.last_printed = None;
        self.cluster_start = None;
        if cols != self.cols {
            // Images stay by the primary cursor, as the text near it does.
            let cursor_row = |s: &Screen| {
                if s.alt_active {
                    s.saved[0].map_or(0, |c| c.cursor.row)
                } else {
                    s.cursor.row
                }
            };
            let before = cursor_row(self);
            self.reflow_primary(cols, rows);
            let delta = cursor_row(self) as i64 - before as i64;
            self.graphics[0].shift(i64::MIN, delta);
            let first = self.primary.scrolled as i64 - self.primary.history.len() as i64;
            self.graphics[0].clear(i64::MIN, first);
        } else {
            let primary_row = if self.alt_active {
                self.saved[0].map_or(0, |s| s.cursor.row)
            } else {
                self.cursor.row
            };
            let moved = self.primary.resize(cols, rows, primary_row);
            if !self.alt_active {
                self.cursor.row = self.cursor.row.saturating_add_signed(moved);
            }
            if let Some(saved) = &mut self.saved[0] {
                saved.cursor.row = saved.cursor.row.saturating_add_signed(moved);
            }
            self.prompt_end = self
                .prompt_end
                .and_then(|(r, c)| Some((r.checked_add_signed(moved)?, c)));
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
            saved.cursor.pending_wrap = false;
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
            let (row, col) = points
                .next()
                .expect("reflow returns every point it is given");
            self.cursor.row = row;
            self.cursor.col = col;
        }
        if let Some(saved) = &mut self.saved[0] {
            let (row, col) = points
                .next()
                .expect("reflow returns every point it is given");
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
        let line = self.top_line();
        let scrolled = self.grid_mut().scroll_up(top, bottom, n, &template, keep);
        if keep {
            // Images in the region went up with the screen; those below it
            // stay, and those past the history are gone.
            let graphics = &mut self.graphics[0];
            graphics.shift(line + bottom as i64 + 1, scrolled as i64);
            let first = self.primary.scrolled as i64 - self.primary.history.len() as i64;
            graphics.clear(i64::MIN, first);
        } else {
            let n = n.min(bottom + 1 - top) as i64;
            self.graphics_mut()
                .scroll(line + top as i64, line + bottom as i64, n);
        }
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
        self.scroll_graphics(top, bottom, -(n as i64));
        self.move_prompt_end(top, bottom, n as isize);
        self.dirty = true;
    }

    /// The text in rows `top..=bottom` moved down by `n` rows (up when
    /// negative): the prompt's end moves along, or is gone if it left them.
    fn move_prompt_end(&mut self, top: usize, bottom: usize, n: isize) {
        self.prompt_end = self.prompt_end.and_then(|(row, col)| {
            if !(top..=bottom).contains(&row) {
                return Some((row, col));
            }
            let row = row.checked_add_signed(n)?;
            (top..=bottom).contains(&row).then_some((row, col))
        });
    }

    /// Moves the images in rows `top..=bottom` up by `n` rows (down when
    /// negative), as the region's text moved.
    fn scroll_graphics(&mut self, top: usize, bottom: usize, n: i64) {
        let line = self.top_line();
        let n = n.clamp(-((bottom + 1 - top) as i64), (bottom + 1 - top) as i64);
        self.graphics_mut()
            .scroll(line + top as i64, line + bottom as i64, n);
    }

    // ---- Editing --------------------------------------------------------------

    /// Ensures column `col` of `row` is not half of a wide character, blanking
    /// the other half if it is.
    fn split_wide(&mut self, row: usize, col: usize) {
        let template = self.cursor.template;
        let cols = self.cols;
        let row = &mut self.grid_mut().rows[row];
        let Some(&cell) = row.cells().get(col) else {
            return;
        };
        if cell.flags.contains(Flags::WIDE_SPACER) && col > 0 {
            *row.cell_mut(col - 1) = Cell::blank(&template);
        } else if cell.flags.contains(Flags::WIDE) && col + 1 < cols {
            *row.cell_mut(col + 1) = Cell::blank(&template);
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
        self.erase_sixels(row, start, end);
        let blank = Cell::blank(&self.cursor.template);
        self.grid_mut().rows[row].cells_in(start..end).fill(blank);
        self.dirty = true;
    }

    /// Blanks `row` from column `start` on. The row no longer flows into the
    /// next one, so a later reflow must not join them.
    fn erase_to_end_of_row(&mut self, row: usize, start: usize) {
        self.erase_cells(row, start, self.cols);
        self.grid_mut().rows[row].wrapped = false;
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
        // A wide character at `col` moves whole; one the insertion lands in
        // the middle of can't.
        if self.grid().rows[row].cells()[col]
            .flags
            .contains(Flags::WIDE_SPACER)
        {
            self.split_wide(row, col);
        }
        let blank = Cell::blank(&self.cursor.template);
        let cols = self.cols;
        let cells = self.grid_mut().rows[row].cells_mut();
        cells[col..].rotate_right(n);
        cells[col..col + n].fill(blank);
        if cells[cols - 1].flags.contains(Flags::WIDE) {
            cells[cols - 1] = blank;
        }
        self.cursor.pending_wrap = false;
        self.dirty = true;
    }

    fn delete_chars(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col.min(self.cols - 1));
        let n = n.min(self.cols - col);
        self.split_wide(row, col);
        self.split_wide(row, col + n - 1);
        let blank = Cell::blank(&self.cursor.template);
        let cols = self.cols;
        let cells = self.grid_mut().rows[row].cells_mut();
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
        self.scroll_graphics(row, bottom, -(n as i64));
        self.move_prompt_end(row, bottom, n as isize);
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
        self.grid_mut()
            .scroll_up(row, bottom, n, &template, /* keep= */ false);
        self.scroll_graphics(row, bottom, n as i64);
        self.move_prompt_end(row, bottom, -(n as isize));
        self.carriage_return();
        self.dirty = true;
    }

    fn print_char(&mut self, c: char) {
        use unicode_width::UnicodeWidthChar;
        let c = self.cursor.charsets[self.cursor.active_charset].map(c);
        let width = c.width().unwrap_or(0).min(2);
        if self.join_cluster(c, width) || width == 0 {
            // A zero-width character with nothing to join is dropped.
            return;
        }
        self.cluster_start = self.print_cell(c, width, Flags::empty());
    }

    /// Prints printable ASCII a row at a time: one column each, nothing to
    /// look up per character. Whatever the general path must handle (a
    /// charset, insert mode, a pending wrap, wide characters to split, a
    /// cluster the first character might join) goes through `print_char`.
    fn write_ascii(&mut self, mut run: &[u8]) {
        if self.cursor.charsets[self.cursor.active_charset] != Charset::Ascii || self.modes.insert {
            for &b in run {
                self.print_char(b as char);
            }
            return;
        }
        // Only a lone ASCII base can't take the first character into its
        // cluster (see `join_cluster`).
        if let Some((row, col)) = self.cluster_start
            && let Some(start) = self.grid().rows.get(row).and_then(|r| r.cells().get(col))
            && !self.is_lone_ascii(start)
            && let Some((&first, rest)) = run.split_first()
        {
            self.print_char(first as char);
            run = rest;
        }
        while let Some(&first) = run.first() {
            if self.cursor.pending_wrap {
                self.print_char(first as char);
                run = &run[1..];
                continue;
            }
            let (row, col) = (self.cursor.row, self.cursor.col);
            let (chunk, rest) = run.split_at(run.len().min(self.cols - col));
            run = rest;
            self.erase_sixels(row, col, col + chunk.len());
            let template = self.cursor.template;
            let cells = self.grid_mut().rows[row].cells_in(col..col + chunk.len());
            // Branchless, so that it vectorizes.
            let wide = cells.iter().fold(false, |any, cell| {
                any | cell.flags.intersects(Flags::WIDE | Flags::WIDE_SPACER)
            });
            if wide {
                for &b in chunk {
                    self.print_char(b as char);
                }
                continue;
            }
            for (cell, &b) in cells.iter_mut().zip(chunk) {
                *cell = Cell {
                    c: b as char,
                    ..template
                };
            }
            let last = col + chunk.len() - 1;
            if last + 1 >= self.cols {
                self.cursor.col = self.cols - 1;
                self.cursor.pending_wrap = true;
            } else {
                self.cursor.col = last + 1;
            }
            self.last_char = Some(chunk[chunk.len() - 1] as char);
            self.last_printed = Some((row, last));
            self.cluster_start = Some((row, last));
            self.dirty = true;
        }
    }

    /// Writes `c` at the cursor with the current attributes and advances.
    /// Returns where it went, if it fit.
    fn print_cell(&mut self, c: char, width: usize, flags: Flags) -> Option<(usize, usize)> {
        if self.cursor.pending_wrap {
            if self.modes.autowrap {
                self.wrap_line();
            } else {
                self.cursor.pending_wrap = false;
            }
        }
        if width == 2 && self.cursor.col + 1 >= self.cols {
            if self.modes.autowrap && self.cols >= 2 {
                let (row, col) = (self.cursor.row, self.cursor.col);
                self.erase_cells(row, col, self.cols);
                self.grid_mut().rows[row]
                    .cell_mut(col)
                    .flags
                    .insert(Flags::WRAP_GAP);
                self.wrap_line();
            } else if self.cols >= 2 {
                self.cursor.col = self.cols - 2;
            } else {
                return None;
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
        self.erase_sixels(row, col, col + width);
        let mut cell = self.cursor.template;
        cell.c = c;
        cell.flags.insert(flags);
        if width == 2 {
            cell.flags.insert(Flags::WIDE);
        }
        let cells = self.grid_mut().rows[row].cells_in(col..col + width);
        cells[0] = cell;
        if width == 2 {
            cells[1] = Cell {
                c: ' ',
                flags: cell.flags | Flags::WIDE_SPACER,
                ..cell
            };
            cells[1].flags.remove(Flags::WIDE);
        }

        if col + width >= self.cols {
            self.cursor.col = self.cols - 1;
            self.cursor.pending_wrap = true;
        } else {
            self.cursor.col += width;
        }
        self.last_char = Some(c);
        self.last_printed = Some((row, col));
        self.dirty = true;
        Some((row, col))
    }

    /// Handles VS16 after a character that has an emoji form (like `❄` or
    /// `☀`): the pair is two cells wide, as in other terminals and in the
    /// width calculations of programs such as starship.
    fn apply_emoji_presentation(&mut self) {
        use unicode_width::UnicodeWidthStr;
        let Some((row, col)) = self.last_printed else {
            return;
        };
        let cell = self.grid().rows[row].cells()[col];
        let pair: String = [cell.c, EMOJI_PRESENTATION].into_iter().collect();
        if row != self.cursor.row
            || col + 1 >= self.cols
            || cell.flags.contains(Flags::WIDE)
            || pair.width() != 2
        {
            return;
        }
        self.split_wide(row, col + 1);
        let cells = self.grid_mut().rows[row].cells_in(col..col + 2);
        cells[0].flags.insert(Flags::WIDE);
        cells[1] = Cell {
            c: ' ',
            flags: cell.flags | Flags::WIDE_SPACER,
            ..cell
        };
        if col + 2 >= self.cols {
            self.cursor.col = self.cols - 1;
            self.cursor.pending_wrap = true;
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
                self.cursor.row = if saved.origin {
                    // The region may have changed since the cursor was saved.
                    self.cursor.row.clamp(self.scroll_top, self.scroll_bottom)
                } else {
                    self.cursor.row.min(self.rows - 1)
                };
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
            self.graphics[1].clear_all();
            self.keyboard[1] = KeyboardFlags::default();
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
            47 => self.set_alt_screen(on, /* clear= */ false),
            1047 => {
                if self.alt_active && !on {
                    self.erase_rows(0, self.rows);
                    self.graphics[1].clear_all();
                }
                self.set_alt_screen(on, /* clear= */ false);
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
                    self.set_alt_screen(/* on= */ true, /* clear= */ true);
                    // The alternate screen starts with the same cursor.
                    self.saved[1] = self.saved[0];
                } else {
                    self.set_alt_screen(/* on= */ false, /* clear= */ false);
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
            2026 => {
                if on && !self.modes.synchronized {
                    self.synchronized_updates += 1;
                }
                self.modes.synchronized = on;
            }
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
    fn report_color(&mut self, code: &str, Rgb { r, g, b }: Rgb, bell_terminated: bool) {
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
        let rgb = match parse_color(spec) {
            Ok(rgb) => rgb,
            Err(e) => {
                log::debug!("bad color {:?}: {e}", String::from_utf8_lossy(spec));
                return;
            }
        };
        match which {
            0 => self.colors.foreground = Some(rgb),
            1 => self.colors.background = Some(rgb),
            _ => self.cursor_color = Some(rgb),
        }
        self.dirty = true;
    }

    /// Answers XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`) for sixel graphics:
    /// reads of the number of color registers (`Pi = 1`) and of the image
    /// size (`Pi = 2`), current (`Pa = 1`) or largest (`Pa = 4`). Changing
    /// them fails.
    fn report_graphics_attribute(&mut self, item: u16, action: u16) {
        let reply = match (item, action) {
            (1, 1 | 4) => format!("\x1b[?1;0;{}S", sixel::REGISTERS),
            (2, 1) => {
                let max = sixel::MAX_SIDE as u32;
                let width = (self.cols as u32 * self.cell_size.0).min(max);
                let height = (self.rows as u32 * self.cell_size.1).min(max);
                format!("\x1b[?2;0;{width};{height}S")
            }
            (2, 4) => format!("\x1b[?2;0;{0};{0}S", sixel::MAX_SIDE),
            (1 | 2, _) => format!("\x1b[?{item};3;0S"),
            _ => format!("\x1b[?{item};1;0S"),
        };
        self.responses.extend_from_slice(reply.as_bytes());
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
        let (width, height) = self.cell_size;
        // Replies and notifications from before the reset are still due,
        // and the window keeps its title.
        let responses = std::mem::take(&mut self.responses);
        let notifications = std::mem::take(&mut self.notifications);
        let (title, title_changed) = (self.title.take(), self.title_changed);
        let bell = self.bell;
        *self = Screen::new(self.cols, self.rows, scrollback);
        self.set_default_colors(default_colors);
        self.set_cell_size(width, height);
        self.responses = responses;
        self.notifications = notifications;
        self.title = title;
        self.title_changed = title_changed;
        self.bell = bell;
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
        match t.flags.underline() {
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
            Color::Rgb(Rgb { r, g, b }) => Some(format!("{extended}:2::{r}:{g}:{b}")),
        };
        parts.extend(color(t.fg, 30, 90, 38));
        parts.extend(color(t.bg, 40, 100, 48));
        parts.extend(color(self.extra(t).underline_color, 0, 0, 58));
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
        // The underline color lives in the template's extra, which only
        // changes after the loop, if any parameter set it.
        let mut underline_color = None;
        let t = &mut self.cursor.template;
        // A hyperlink (OSC 8) is not an attribute; resets keep it.
        let reset = Cell {
            extra: t.extra,
            ..Cell::default()
        };
        if params.is_empty() {
            *t = reset;
            underline_color = Some(Color::Default);
        }
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            match param[0] {
                0 => {
                    *t = reset;
                    underline_color = Some(Color::Default);
                }
                1 => t.flags.insert(Flags::BOLD),
                2 => t.flags.insert(Flags::DIM),
                3 => t.flags.insert(Flags::ITALIC),
                4 => t.flags.set_underline(match param.get(1) {
                    Some(0) => Underline::None,
                    Some(2) => Underline::Double,
                    Some(3) => Underline::Curly,
                    Some(4) => Underline::Dotted,
                    Some(5) => Underline::Dashed,
                    _ => Underline::Single,
                }),
                5 | 6 => t.flags.insert(Flags::BLINK),
                7 => t.flags.insert(Flags::INVERSE),
                8 => t.flags.insert(Flags::HIDDEN),
                9 => t.flags.insert(Flags::STRIKE),
                21 => t.flags.set_underline(Underline::Double),
                22 => {
                    t.flags.remove(Flags::BOLD);
                    t.flags.remove(Flags::DIM);
                }
                23 => t.flags.remove(Flags::ITALIC),
                24 => t.flags.set_underline(Underline::None),
                25 => t.flags.remove(Flags::BLINK),
                27 => t.flags.remove(Flags::INVERSE),
                28 => t.flags.remove(Flags::HIDDEN),
                29 => t.flags.remove(Flags::STRIKE),
                n @ 30..=37 => t.fg = Color::Indexed((n - 30) as u8),
                38 => {
                    if let Ok(color) = extended_color(param, &mut iter) {
                        t.fg = color;
                    }
                }
                39 => t.fg = Color::Default,
                n @ 40..=47 => t.bg = Color::Indexed((n - 40) as u8),
                48 => {
                    if let Ok(color) = extended_color(param, &mut iter) {
                        t.bg = color;
                    }
                }
                49 => t.bg = Color::Default,
                58 => {
                    if let Ok(color) = extended_color(param, &mut iter) {
                        underline_color = Some(color);
                    }
                }
                59 => underline_color = Some(Color::Default),
                n @ 90..=97 => t.fg = Color::Indexed((n - 90 + 8) as u8),
                n @ 100..=107 => t.bg = Color::Indexed((n - 100 + 8) as u8),
                n => log::debug!("unhandled SGR {n}"),
            }
        }
        let current = self.extra(&self.cursor.template);
        if let Some(color) = underline_color
            && current.underline_color != color
        {
            self.make_room(None, |_| Extra {
                underline_color: color,
                ..current
            });
            self.update_template_extra(|extra| extra.underline_color = color);
        }
    }
}

/// Parses an X11 color specification as used by OSC color sequences:
/// `#rgb`, `#rrggbb`, or `rgb:r/g/b` with 1–4 hex digits per component.
fn parse_color(spec: &[u8]) -> Result<Rgb, &'static str> {
    const BAD_COMPONENT: &str = "a component is not 1-4 hex digits";
    let spec = std::str::from_utf8(spec).map_err(|_| "not UTF-8")?;
    let scale = |hex: &str| -> Result<u8, &'static str> {
        if hex.is_empty() || hex.len() > 4 {
            return Err(BAD_COMPONENT);
        }
        let value = u32::from_str_radix(hex, 16).map_err(|_| BAD_COMPONENT)?;
        let max = (1u32 << (4 * hex.len())) - 1;
        Ok((value * 255 / max) as u8)
    };
    if let Some(hex) = spec.strip_prefix('#') {
        if !matches!(hex.len(), 3 | 6) || !hex.is_ascii() {
            return Err("expected 3 or 6 hex digits after #");
        }
        let n = hex.len() / 3;
        return Ok(Rgb::new(
            scale(&hex[..n])?,
            scale(&hex[n..2 * n])?,
            scale(&hex[2 * n..])?,
        ));
    }
    let mut parts = spec
        .strip_prefix("rgb:")
        .ok_or("expected #rgb, #rrggbb or rgb:r/g/b")?
        .split('/');
    let mut component = || scale(parts.next().ok_or("expected three components")?);
    let rgb = Rgb::new(component()?, component()?, component()?);
    match parts.next() {
        None => Ok(rgb),
        Some(_) => Err("expected three components"),
    }
}

/// Parses the arguments of SGR 38/48/58 in either the colon form
/// (`38:2::r:g:b`, `38:5:n`) or the semicolon form (`38;2;r;g;b`, `38;5;n`).
fn extended_color<'a>(
    param: &[u16],
    iter: &mut impl Iterator<Item = &'a [u16]>,
) -> Result<Color, &'static str> {
    const TRUNCATED: &str = "the color is cut short";
    const UNKNOWN: &str = "the color space is not 2 (RGB) or 5 (indexed)";
    let byte = |v: u16| v.min(255) as u8;
    if param.len() > 1 {
        return match param[1] {
            5 => param
                .get(2)
                .map(|&n| Color::Indexed(byte(n)))
                .ok_or(TRUNCATED),
            2 if param.len() >= 5 => {
                let rgb = &param[param.len() - 3..];
                Ok(Color::Rgb(Rgb::new(
                    byte(rgb[0]),
                    byte(rgb[1]),
                    byte(rgb[2]),
                )))
            }
            2 => Err(TRUNCATED),
            _ => Err(UNKNOWN),
        };
    }
    let mut next = || iter.next().map(|p| p[0]).ok_or(TRUNCATED);
    match next()? {
        5 => Ok(Color::Indexed(byte(next()?))),
        2 => {
            let r = next()?;
            let g = next()?;
            let b = next()?;
            Ok(Color::Rgb(Rgb::new(byte(r), byte(g), byte(b))))
        }
        _ => Err(UNKNOWN),
    }
}

impl Perform for Screen {
    #[inline]
    fn print(&mut self, c: char) {
        self.print_char(c);
    }

    #[inline]
    fn print_ascii(&mut self, text: &str) {
        self.write_ascii(text.as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        self.last_printed = None;
        self.cluster_start = None;
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

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // Too many parameters or intermediates: acting on what's left would
        // do something the program didn't ask for.
        if ignore {
            return;
        }
        self.last_printed = None;
        self.cluster_start = None;
        // On the stack: this runs for every escape sequence that colors
        // output. vte keeps at most 32 parameters.
        let mut buf = [0u16; 32];
        let mut len = 0;
        for (slot, param) in buf.iter_mut().zip(params.iter()) {
            *slot = param[0];
            len += 1;
        }
        let args = &buf[..len];
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
                        self.erase_to_end_of_row(row, col);
                        self.erase_rows(row + 1, self.rows);
                    }
                    1 => {
                        self.erase_rows(0, row);
                        self.erase_cells(row, 0, col + 1);
                    }
                    2 => {
                        self.erase_rows(0, self.rows);
                        let top = self.top_line();
                        let bottom = top + self.rows as i64;
                        self.graphics_mut().clear(top, bottom);
                    }
                    3 => {
                        self.grid_mut().clear_history();
                        self.display_offset = 0;
                        let top = self.top_line();
                        self.graphics_mut().clear(i64::MIN, top);
                    }
                    _ => {}
                }
            }
            ([] | [b'?'], 'K') => {
                let (row, col) = (self.cursor.row, self.cursor.col);
                match raw(0) {
                    0 => self.erase_to_end_of_row(row, col),
                    1 => self.erase_cells(row, 0, col + 1),
                    2 => self.erase_to_end_of_row(row, 0),
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
                // At most a row's worth: programs repeat within a line, and
                // a huge count in 8 bytes would otherwise cost a screenful
                // of work each.
                if let Some(c) = self.last_char {
                    for _ in 0..arg(0, 1).min(self.cols) {
                        self.print_char(c);
                    }
                }
            }
            ([], 'c') if raw(0) == 0 => self.responses.extend_from_slice(b"\x1b[?62;4;22c"),
            ([b'?'], 'S') => self.report_graphics_attribute(raw(0), raw(1)),
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
                    let report = format!("\x1b[{};{}R", row.saturating_sub(top) + 1, col + 1);
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
            ([], 'h') => args.iter().for_each(|&m| self.set_mode(m, /* on= */ true)),
            ([], 'l') => args.iter().for_each(|&m| self.set_mode(m, /* on= */ false)),
            ([b'?'], 'h') => args
                .iter()
                .for_each(|&m| self.set_private_mode(m, /* on= */ true)),
            ([b'?'], 'l') => args
                .iter()
                .for_each(|&m| self.set_private_mode(m, /* on= */ false)),
            ([b'?', b'$'], 'p') => self.report_mode(raw(0), /* private= */ true),
            ([b'$'], 'p') => self.report_mode(raw(0), /* private= */ false),
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
            ([], 't') => {
                let (cw, ch) = self.cell_size;
                let (cols, rows) = (self.cols as u32, self.rows as u32);
                let report = match raw(0) {
                    14 => Some(format!("\x1b[4;{};{}t", rows * ch, cols * cw)),
                    16 => Some(format!("\x1b[6;{ch};{cw}t")),
                    18 => Some(format!("\x1b[8;{rows};{cols}t")),
                    _ => None,
                };
                if let Some(report) = report {
                    self.responses.extend_from_slice(report.as_bytes());
                }
            }
            ([], 's') => self.save_cursor(),
            ([], 'u') => self.restore_cursor(),
            ([marker @ (b'?' | b'>' | b'<' | b'=')], 'u') => self.keyboard_mode(*marker, args),
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

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        self.last_printed = None;
        self.cluster_start = None;
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
                    row.wrapped = false;
                    for cell in row.cells_mut() {
                        *cell = Cell {
                            c: 'E',
                            ..Cell::default()
                        };
                    }
                }
                // DECALN also resets the margins and homes the cursor.
                self.scroll_top = 0;
                self.scroll_bottom = self.rows - 1;
                self.modes.origin = false;
                self.cursor.row = 0;
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
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

    fn hook(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        self.last_printed = None;
        self.cluster_start = None;
        match (intermediates, action) {
            (b"$", 'q') => self.decrqss = Some(Vec::new()),
            (b"", 'q') => {
                // P2 = 1 leaves the pixels the image doesn't set transparent.
                let transparent = params.iter().nth(1).and_then(|p| p.first()) == Some(&1);
                let Rgb { r, g, b } = self
                    .colors
                    .background
                    .unwrap_or(self.default_colors.background);
                let decoder = sixel::Decoder::new(transparent, [r, g, b]);
                self.sixel = Some(Box::new(decoder));
            }
            _ => {}
        }
    }

    fn put(&mut self, byte: u8) {
        if let Some(decoder) = &mut self.sixel {
            decoder.put(byte);
        } else if let Some(data) = &mut self.decrqss
            && data.len() < 16
        {
            data.push(byte);
        }
    }

    fn unhook(&mut self) {
        if let Some(request) = self.decrqss.take() {
            self.report_setting(&request);
        }
        if let Some(image) = self.sixel.take().and_then(|d| d.finish()) {
            self.place_sixel(image);
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
                    } else if let Ok(rgb) = parse_color(spec) {
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
                // Wayland messages are limited to 4096 bytes; a longer
                // title would get the window disconnected.
                self.title = Some(truncate(title, MAX_TITLE_CHARS));
                self.title_changed = true;
            }
            // ConEmu uses OSC 9 ; <number> ; … for other things (9;4 is a
            // progress bar), so a numeric first field is not a message.
            [b"9", first, ..] if !first.is_empty() && first.iter().all(u8::is_ascii_digit) => {}
            // iTerm2: OSC 9 ; message. The message may contain ';'.
            [b"9", body @ ..] => self.notify(None, body),
            // rxvt-unicode: OSC 777 ; notify ; title ; body.
            [b"777", b"notify", title, body @ ..] => self.notify(Some(title), body),
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
