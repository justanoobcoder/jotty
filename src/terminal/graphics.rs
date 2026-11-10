//! The kitty graphics protocol: images that programs transmit in APC
//! sequences (`ESC _ G … ESC \`) and place on the grid.
//!
//! Each screen buffer has its own [`Graphics`]: the images, and the
//! placements that show them at a cell. A placement's row is counted in
//! rows since the grid was created (the grid's `scrolled` count plus the
//! screen row), so placements scroll into the history with the text.
//! Virtual placements (`U=1`) are not drawn at a position; cells holding
//! [`PLACEHOLDER`] show parts of them instead.

use std::collections::HashSet;
use std::ffi::{CString, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::io::SeekFrom;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::grid::{Cell, Color};

/// U+10EEEE: a cell showing part of an image through a virtual placement.
/// Combining diacritics after it give the image row and column.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The diacritics that encode numbers in placeholder cells: the n-th one
/// stands for n.
#[rustfmt::skip]
const DIACRITICS: [char; 297] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
    '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}',
    '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}',
    '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}',
    '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}',
    '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}',
    '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}',
    '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}',
    '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}',
    '\u{A8EE}', '\u{A8EF}', '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}', '\u{AAB3}', '\u{AAB7}',
    '\u{AAB8}', '\u{AABE}', '\u{AABF}', '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}', '\u{1D185}', '\u{1D186}', '\u{1D187}',
    '\u{1D188}', '\u{1D189}', '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}', '\u{1D242}', '\u{1D243}',
    '\u{1D244}',
];

/// The largest image accepted, in pixels per side.
const MAX_SIDE: u32 = 10_000;
/// The largest transmitted or decompressed data accepted.
const MAX_DATA: usize = 400 << 20;
/// How much image data a screen keeps before evicting the oldest images.
const QUOTA: usize = 320 << 20;
/// How many images and placements a screen keeps; past that the oldest
/// go, so that floods of tiny ones can't slow every frame and scroll.
const MAX_IMAGES: usize = 1024;
const MAX_PLACEMENTS: usize = 4096;

/// Identifies an image across both screens and its retransmissions, so the
/// renderer can cache it.
static NEXT_UID: AtomicU64 = AtomicU64::new(1);

pub struct Image {
    pub uid: u64,
    /// The id the program refers to it by; 0 for an anonymous image.
    pub id: u32,
    /// The number (`I`) it was transmitted with, if any.
    pub number: u32,
    pub width: u32,
    pub height: u32,
    /// RGBA, not premultiplied.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The image's uid.
    pub image: u64,
    /// The placement id (`p`); 0 when the program gave none.
    id: u32,
    /// The top row, in rows since the grid was created.
    pub line: i64,
    pub col: usize,
    /// Pixel offset within the first cell (`X`, `Y`).
    pub offset: (u32, u32),
    /// The part of the image shown: x, y, width, height in pixels.
    pub source: [u32; 4],
    /// Columns and rows to scale the image to (`c`, `r`); 0 keeps the
    /// source size.
    pub cols: u32,
    pub rows: u32,
    pub z: i32,
    pub is_virtual: bool,
}

impl Placement {
    /// The size the image is drawn at, in pixels, with cells of `cell`
    /// pixels: scaled to the columns and rows given, keeping the aspect
    /// ratio when only one of them is.
    pub fn pixel_size(&self, (cw, ch): (u32, u32)) -> (f32, f32) {
        let [_, _, sw, sh] = self.source.map(|v| v as f32);
        let (dx, dy) = (self.offset.0 as f32, self.offset.1 as f32);
        let width = (self.cols as f32 * cw as f32 - dx).max(1.0);
        let height = (self.rows as f32 * ch as f32 - dy).max(1.0);
        match (self.cols, self.rows) {
            (0, 0) => (sw, sh),
            (0, _) => (height * sw / sh.max(1.0), height),
            (_, 0) => (width, width * sh / sw.max(1.0)),
            _ => (width, height),
        }
    }

    /// The columns and rows the placement covers with cells of `cell`
    /// pixels.
    pub fn size_in_cells(&self, cell: (u32, u32)) -> (u32, u32) {
        let (w, h) = self.pixel_size(cell);
        let span = |given: u32, pixels: f32, offset: u32, size: u32| match given {
            0 => ((pixels + offset as f32) / size.max(1) as f32).ceil() as u32,
            n => n,
        };
        (
            span(self.cols, w, self.offset.0, cell.0).max(1),
            span(self.rows, h, self.offset.1, cell.1).max(1),
        )
    }

    fn covers(&self, (line, col): (i64, usize), cell: (u32, u32)) -> bool {
        let (cols, rows) = self.size_in_cells(cell);
        (self.line..self.line + i64::from(rows)).contains(&line)
            && (self.col..self.col + cols as usize).contains(&col)
    }

    fn lines(&self, cell: (u32, u32)) -> Range<i64> {
        self.line..self.line + i64::from(self.size_in_cells(cell).1)
    }
}

/// A parsed command: the keys of the control data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Command {
    action: u8,
    quiet: u8,
    format: u32,
    medium: u8,
    compressed: bool,
    width: u32,
    height: u32,
    size: u32,
    offset: u32,
    id: u32,
    number: u32,
    placement: u32,
    more: bool,
    source: [u32; 4],
    cell_offset: (u32, u32),
    cols: u32,
    rows: u32,
    no_move: bool,
    is_virtual: bool,
    z: i32,
    delete: u8,
}

impl Default for Command {
    fn default() -> Self {
        Command {
            action: b't',
            quiet: 0,
            format: 32,
            medium: b'd',
            compressed: false,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            id: 0,
            number: 0,
            placement: 0,
            more: false,
            source: [0; 4],
            cell_offset: (0, 0),
            cols: 0,
            rows: 0,
            no_move: false,
            is_virtual: false,
            z: 0,
            delete: b'a',
        }
    }
}

impl Command {
    /// Parses `key=value` pairs separated by commas. Unknown keys are
    /// ignored; a malformed value rejects the command.
    fn parse(control: &[u8]) -> Option<Command> {
        let mut cmd = Command::default();
        for pair in control.split(|&b| b == b',').filter(|p| !p.is_empty()) {
            let [key, b'=', value @ ..] = pair else {
                return None;
            };
            let value = std::str::from_utf8(value).ok()?;
            let char = || match value.as_bytes() {
                [c] => Some(*c),
                _ => None,
            };
            let num = || value.parse::<u32>().ok();
            match key {
                b'a' => cmd.action = char()?,
                b'q' => cmd.quiet = num()?.min(2) as u8,
                b'f' => cmd.format = num()?,
                b't' => cmd.medium = char()?,
                b'o' => cmd.compressed = char()? == b'z',
                b's' => cmd.width = num()?,
                b'v' => cmd.height = num()?,
                b'S' => cmd.size = num()?,
                b'O' => cmd.offset = num()?,
                b'i' => cmd.id = num()?,
                b'I' => cmd.number = num()?,
                b'p' => cmd.placement = num()?,
                b'm' => cmd.more = num()? == 1,
                b'x' => cmd.source[0] = num()?,
                b'y' => cmd.source[1] = num()?,
                b'w' => cmd.source[2] = num()?,
                b'h' => cmd.source[3] = num()?,
                b'X' => cmd.cell_offset.0 = num()?,
                b'Y' => cmd.cell_offset.1 = num()?,
                b'c' => cmd.cols = num()?,
                b'r' => cmd.rows = num()?,
                b'C' => cmd.no_move = num()? == 1,
                b'U' => cmd.is_virtual = num()? == 1,
                b'z' => cmd.z = value.parse().ok()?,
                b'd' => cmd.delete = char()?,
                _ => {}
            }
        }
        Some(cmd)
    }
}

/// Where the command runs: the screen and the cursor.
pub struct Context {
    /// The line of the top screen row.
    pub top: i64,
    pub rows: usize,
    /// The cursor as (line, column).
    pub cursor: (i64, usize),
}

/// What the screen must do after a command.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The reply to send to the program.
    pub response: Option<String>,
    /// The columns and rows to move the cursor past an image placed at it.
    pub moved: Option<(u32, u32)>,
}

/// A transmission split into chunks (`m=1`), being received.
struct Loading {
    command: Command,
    /// The base64 payload so far.
    payload: Vec<u8>,
}

/// The images and placements of one screen buffer.
pub struct Graphics {
    /// Oldest first, which is also by increasing uid.
    images: Vec<Image>,
    /// In the order they were placed, which is the drawing order within a
    /// z-index.
    placements: Vec<Placement>,
    loading: Option<Loading>,
    /// The next id to try for an image transmitted with only a number.
    next_id: u32,
    /// The cell size in pixels, which sizes placements in cells.
    pub cell: (u32, u32),
}

impl Default for Graphics {
    fn default() -> Self {
        Graphics {
            images: Vec::new(),
            placements: Vec::new(),
            loading: None,
            next_id: 1 << 31,
            cell: (10, 20),
        }
    }
}

impl Graphics {
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    pub fn image(&self, uid: u64) -> Option<&Image> {
        let i = self.images.binary_search_by_key(&uid, |i| i.uid).ok()?;
        Some(&self.images[i])
    }

    /// The uids of the images that have a placement.
    fn placed(&self) -> HashSet<u64> {
        self.placements.iter().map(|p| p.image).collect()
    }

    pub fn image_by_id(&self, id: u32) -> Option<&Image> {
        self.images.iter().rfind(|i| i.id == id && id != 0)
    }

    /// The virtual placement of an image with id `placement`, or its latest
    /// one when `placement` is 0.
    pub fn virtual_placement(&self, image: u64, placement: u32) -> Option<&Placement> {
        self.placements
            .iter()
            .rfind(|p| p.is_virtual && p.image == image && (placement == 0 || p.id == placement))
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Handles a command: the APC body after the `G`.
    pub fn command(&mut self, data: &[u8], ctx: &Context) -> Outcome {
        let (control, payload) = match data.iter().position(|&b| b == b';') {
            Some(i) => (&data[..i], &data[i + 1..]),
            None => (data, &[][..]),
        };
        let Some(cmd) = Command::parse(control) else {
            log::debug!(
                "bad graphics command {:?}",
                String::from_utf8_lossy(control)
            );
            return Outcome::default();
        };
        // Later chunks of a transmission carry only `m` (and maybe `q`).
        if let Some(mut loading) = self.loading.take()
            && matches!(cmd.action, b't' | b'T' | b'q')
        {
            if loading.payload.len() + payload.len() > MAX_DATA * 4 / 3 {
                return Outcome {
                    response: reply(&loading.command, Err("EFBIG: too much data".into())),
                    moved: None,
                };
            }
            loading.payload.extend_from_slice(payload);
            if cmd.more {
                self.loading = Some(loading);
                return Outcome::default();
            }
            // Free the base64 text before the later stages make more
            // copies of the image.
            let Loading { command, payload } = loading;
            let data = base64(&payload);
            drop(payload);
            return self.finish(command, data, ctx);
        }
        match cmd.action {
            b't' | b'T' | b'q' if cmd.more => {
                self.loading = Some(Loading {
                    command: cmd,
                    payload: payload.to_vec(),
                });
                Outcome::default()
            }
            b't' | b'T' | b'q' => self.finish(cmd, base64(payload), ctx),
            b'p' => {
                let image = if cmd.id != 0 {
                    self.image_by_id(cmd.id)
                } else {
                    self.images
                        .iter()
                        .rfind(|i| i.number == cmd.number && cmd.number != 0)
                };
                match image {
                    Some(image) => {
                        let cmd = Command {
                            id: image.id,
                            ..cmd
                        };
                        self.place(image.uid, &cmd, ctx)
                    }
                    None => Outcome {
                        response: reply(&cmd, Err("ENOENT: no such image".into())),
                        moved: None,
                    },
                }
            }
            b'd' => {
                self.delete(&cmd, ctx);
                Outcome::default()
            }
            action => {
                log::debug!("unsupported graphics action {}", action as char);
                Outcome {
                    response: reply(&cmd, Err("EINVAL: unsupported action".into())),
                    moved: None,
                }
            }
        }
    }

    /// Finishes a transmission (`a=t`, `T` or `q`) with its whole payload,
    /// decoded from base64.
    fn finish(&mut self, mut cmd: Command, data: Option<Vec<u8>>, ctx: &Context) -> Outcome {
        let image = data
            .ok_or_else(|| "EINVAL: bad base64 payload".to_string())
            .and_then(|data| read_medium(&cmd, data))
            .and_then(|data| decompress(&cmd, data))
            .and_then(|data| decode(&cmd, data));
        let error = |cmd: &Command, e| Outcome {
            response: reply(cmd, Err(e)),
            moved: None,
        };
        match (cmd.action, image) {
            (b'q', result) => Outcome {
                response: reply(&cmd, result.map(|_| ())),
                moved: None,
            },
            (_, Err(e)) => {
                log::debug!("graphics transmission failed: {e}");
                error(&cmd, e)
            }
            (action, Ok((width, height, pixels))) => {
                if cmd.id == 0 && cmd.number != 0 {
                    cmd.id = self.free_id();
                }
                let uid = self.store(&cmd, width, height, pixels);
                if action == b'T' {
                    self.place(uid, &cmd, ctx)
                } else {
                    Outcome {
                        response: reply(&cmd, Ok(())),
                        moved: None,
                    }
                }
            }
        }
    }

    /// An id no image has, for an image transmitted with only a number.
    fn free_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if self.image_by_id(id).is_none() {
                return id;
            }
        }
    }

    /// Adds an image, replacing one with the same id. Returns its uid.
    fn store(&mut self, cmd: &Command, width: u32, height: u32, pixels: Vec<u8>) -> u64 {
        if cmd.id != 0 {
            let old: Vec<u64> = self
                .images
                .iter()
                .filter(|i| i.id == cmd.id)
                .map(|i| i.uid)
                .collect();
            for uid in old {
                self.remove_image(uid);
            }
        }
        let uid = NEXT_UID.fetch_add(1, Ordering::Relaxed);
        self.images.push(Image {
            uid,
            id: cmd.id,
            number: cmd.number,
            width,
            height,
            pixels,
        });
        self.enforce_quota();
        uid
    }

    /// Evicts the oldest images, unplaced ones first, until the images fit
    /// the quota. The newest image is always kept.
    fn enforce_quota(&mut self) {
        let mut total: usize = self.images.iter().map(|i| i.pixels.len()).sum();
        let over = |total: usize, count: usize| total > QUOTA || count > MAX_IMAGES;
        if !over(total, self.images.len()) {
            return;
        }
        let placed = self.placed();
        let mut evicted = HashSet::new();
        let mut count = self.images.len();
        for include_placed in [false, true] {
            for image in &self.images[..self.images.len() - 1] {
                if !over(total, count) {
                    break;
                }
                if evicted.contains(&image.uid) || (!include_placed && placed.contains(&image.uid))
                {
                    continue;
                }
                evicted.insert(image.uid);
                total -= image.pixels.len();
                count -= 1;
            }
        }
        self.images.retain(|i| !evicted.contains(&i.uid));
        self.placements.retain(|p| !evicted.contains(&p.image));
    }

    fn remove_image(&mut self, uid: u64) {
        self.images.retain(|i| i.uid != uid);
        self.placements.retain(|p| p.image != uid);
    }

    /// Places image `uid` at the cursor, or virtually.
    fn place(&mut self, uid: u64, cmd: &Command, ctx: &Context) -> Outcome {
        let Some(image) = self.image(uid) else {
            return Outcome::default();
        };
        let (w, h) = (image.width, image.height);
        let [x, y, sw, sh] = cmd.source;
        let (x, y) = (x.min(w), y.min(h));
        let clamp = |requested: u32, available: u32| match requested {
            0 => available,
            n => n.min(available),
        };
        let placement = Placement {
            image: uid,
            id: cmd.placement,
            line: ctx.cursor.0,
            col: ctx.cursor.1,
            offset: cmd.cell_offset,
            source: [x, y, clamp(sw, w - x), clamp(sh, h - y)],
            cols: cmd.cols,
            rows: cmd.rows,
            z: cmd.z,
            is_virtual: cmd.is_virtual,
        };
        if cmd.placement != 0 {
            self.placements
                .retain(|p| !(p.image == uid && p.id == cmd.placement));
        }
        self.placements.push(placement);
        if self.placements.len() > MAX_PLACEMENTS {
            self.placements.remove(0);
            self.drop_unreachable();
        }
        let moved = (!cmd.is_virtual && !cmd.no_move).then(|| placement.size_in_cells(self.cell));
        Outcome {
            response: reply(cmd, Ok(())),
            moved,
        }
    }

    /// Deletes placements (`a=d`) and, for an upper-case specifier, the
    /// images left without any.
    fn delete(&mut self, cmd: &Command, ctx: &Context) {
        let cell = self.cell;
        let which = cmd.delete.to_ascii_lowercase();
        let screen = ctx.top..ctx.top + ctx.rows as i64;
        // A cell given in 1-based screen coordinates.
        let at = |x: u32, y: u32| (ctx.top + i64::from(y) - 1, (x as usize).saturating_sub(1));
        let images = &self.images;
        let newest = |number: u32| {
            images
                .iter()
                .rfind(|i| i.number == number && number != 0)
                .map(|i| i.uid)
        };
        // The images a specifier names, which upper case deletes even
        // without placements.
        let named: Vec<u64> = images
            .iter()
            .filter(|i| match which {
                b'i' => i.id == cmd.id && cmd.id != 0 && cmd.placement == 0,
                b'n' => Some(i.uid) == newest(cmd.number) && cmd.placement == 0,
                b'r' => (cmd.source[0]..=cmd.source[1]).contains(&i.id) && i.id != 0,
                _ => false,
            })
            .map(|i| i.uid)
            .collect();
        let id_of = |uid: u64| {
            images
                .binary_search_by_key(&uid, |i| i.uid)
                .map_or(0, |i| images[i].id)
        };
        let hit = |p: &Placement| match which {
            b'a' => {
                !p.is_virtual
                    && p.lines(cell).start < screen.end
                    && p.lines(cell).end > screen.start
            }
            b'i' => {
                cmd.id != 0
                    && id_of(p.image) == cmd.id
                    && (cmd.placement == 0 || p.id == cmd.placement)
            }
            b'n' => {
                Some(p.image) == newest(cmd.number) && (cmd.placement == 0 || p.id == cmd.placement)
            }
            b'c' => !p.is_virtual && p.covers(ctx.cursor, cell),
            b'p' => !p.is_virtual && p.covers(at(cmd.source[0], cmd.source[1]), cell),
            b'q' => {
                !p.is_virtual && p.z == cmd.z && p.covers(at(cmd.source[0], cmd.source[1]), cell)
            }
            b'r' => {
                let id = id_of(p.image);
                id != 0 && (cmd.source[0]..=cmd.source[1]).contains(&id)
            }
            b'x' => {
                let (cols, _) = p.size_in_cells(cell);
                let col = (cmd.source[0] as usize).saturating_sub(1);
                !p.is_virtual && (p.col..p.col + cols as usize).contains(&col)
            }
            b'y' => !p.is_virtual && p.lines(cell).contains(&at(1, cmd.source[1]).0),
            b'z' => !p.is_virtual && p.z == cmd.z,
            _ => false,
        };
        let mut touched = named;
        let mut kept = Vec::with_capacity(self.placements.len());
        for p in std::mem::take(&mut self.placements) {
            if hit(&p) {
                touched.push(p.image);
            } else {
                kept.push(p);
            }
        }
        self.placements = kept;
        if cmd.delete.is_ascii_uppercase() {
            let placed = self.placed();
            let unplaced: HashSet<u64> = touched
                .into_iter()
                .filter(|u| !placed.contains(u))
                .collect();
            self.images.retain(|i| !unplaced.contains(&i.uid));
        }
        self.drop_unreachable();
    }

    /// Removes anonymous images without placements: nothing can show them
    /// again.
    fn drop_unreachable(&mut self) {
        if self.images.iter().all(|i| i.id != 0) {
            return;
        }
        let placed = self.placed();
        self.images.retain(|i| i.id != 0 || placed.contains(&i.uid));
    }

    /// Moves the placements whose top row is in `top..=bottom` up by `n`
    /// rows (down when negative), as when that region scrolls, dropping
    /// those that leave it.
    pub fn scroll(&mut self, top: i64, bottom: i64, n: i64) {
        let cell = self.cell;
        self.placements.retain_mut(|p| {
            if p.is_virtual || !(top..=bottom).contains(&p.line) {
                return true;
            }
            p.line -= n;
            p.lines(cell).end > top && p.line <= bottom
        });
        self.drop_unreachable();
    }

    /// Moves every placement whose top row is at or below `from` by
    /// `delta` rows.
    pub fn shift(&mut self, from: i64, delta: i64) {
        for p in &mut self.placements {
            if p.line >= from {
                p.line += delta;
            }
        }
    }

    /// Deletes the placements that touch rows `start..end`.
    pub fn clear(&mut self, start: i64, end: i64) {
        let cell = self.cell;
        self.placements.retain(|p| {
            let lines = p.lines(cell);
            p.is_virtual || lines.end <= start || lines.start >= end
        });
        self.drop_unreachable();
    }

    /// Deletes everything.
    pub fn clear_all(&mut self) {
        self.images.clear();
        self.placements.clear();
        self.loading = None;
    }
}

/// The reply to a command, if it asks for one: commands that name no image
/// get none, and `q` silences successes (1) or everything (2).
fn reply(cmd: &Command, result: Result<(), String>) -> Option<String> {
    if cmd.id == 0 && cmd.number == 0 {
        return None;
    }
    let message = match result {
        Ok(()) if cmd.quiet == 0 => "OK".to_string(),
        Err(e) if cmd.quiet < 2 => e,
        _ => return None,
    };
    let mut keys = format!("i={}", cmd.id);
    if cmd.number != 0 {
        keys.push_str(&format!(",I={}", cmd.number));
    }
    if cmd.placement != 0 {
        keys.push_str(&format!(",p={}", cmd.placement));
    }
    Some(format!("\x1b_G{keys};{message}\x1b\\"))
}

/// Decodes standard base64, ignoring padding and whitespace.
fn base64(text: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in text {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Gets the image data from where the transmission medium (`t`) says.
fn read_medium(cmd: &Command, data: Vec<u8>) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStringExt;
    let (offset, size) = (u64::from(cmd.offset), cmd.size as usize);
    match cmd.medium {
        b'd' => Ok(data),
        b'f' => read_file(Path::new(&OsString::from_vec(data)), offset, size),
        b't' => {
            let path = PathBuf::from(OsString::from_vec(data));
            if !is_temp_file(&path) {
                return Err("EPERM: not a temporary file for graphics".into());
            }
            let result = read_file(&path, offset, size);
            let _ = std::fs::remove_file(&path);
            result
        }
        b's' => read_shm(&data, offset, size),
        _ => Err("EINVAL: unknown transmission medium".into()),
    }
}

/// Whether a program may have the terminal delete `path` (`t=t`): a file
/// named for the protocol in a temporary directory.
fn is_temp_file(path: &Path) -> bool {
    let named = path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains("tty-graphics-protocol"));
    let Some(dir) = path.parent().and_then(|d| d.canonicalize().ok()) else {
        return false;
    };
    let temp = [std::env::temp_dir(), "/tmp".into(), "/dev/shm".into()];
    named
        && temp
            .iter()
            .filter_map(|t| t.canonicalize().ok())
            .any(|t| dir.starts_with(t))
}

/// Reads `size` bytes (all when 0) at `offset` from a regular file.
fn read_from(mut file: File, offset: u64, size: usize) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek};
    let error = |e: io::Error| format!("EBADF: {e}");
    if !file.metadata().map_err(error)?.is_file() {
        return Err("EBADF: not a regular file".into());
    }
    file.seek(SeekFrom::Start(offset)).map_err(error)?;
    let limit = if size == 0 {
        MAX_DATA
    } else {
        size.min(MAX_DATA)
    };
    let mut data = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut data)
        .map_err(error)?;
    Ok(data)
}

fn read_file(path: &Path, offset: u64, size: usize) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    // Without O_NONBLOCK, opening a FIFO waits for a writer, forever if
    // there is none, and the whole terminal hangs. Regular files ignore it.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(|e| format!("EBADF: {e}"))?;
    read_from(file, offset, size)
}

/// Reads and unlinks the POSIX shared memory object named by `name`.
fn read_shm(name: &[u8], offset: u64, size: usize) -> Result<Vec<u8>, String> {
    use std::os::unix::io::FromRawFd;
    let name = CString::new(name).map_err(|_| "EINVAL: bad shared memory name")?;
    // SAFETY: `name` is a valid C string; the result is checked.
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(format!("EBADF: {}", io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just opened and is owned by nothing else.
    let file = unsafe { File::from_raw_fd(fd) };
    // SAFETY: `name` is a valid C string.
    unsafe { libc::shm_unlink(name.as_ptr()) };
    read_from(file, offset, size)
}

fn decompress(cmd: &Command, data: Vec<u8>) -> Result<Vec<u8>, String> {
    if !cmd.compressed {
        return Ok(data);
    }
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data, MAX_DATA)
        .map_err(|_| "EINVAL: cannot decompress the data".to_string())
}

/// Decodes the image data into (width, height, RGBA pixels).
fn decode(cmd: &Command, data: Vec<u8>) -> Result<(u32, u32, Vec<u8>), String> {
    let bytes_per_pixel = match cmd.format {
        100 => return decode_png(&data),
        24 => 3,
        32 => 4,
        _ => return Err("EINVAL: unknown format".into()),
    };
    let (w, h) = (cmd.width, cmd.height);
    if w == 0 || h == 0 {
        return Err("EINVAL: missing width or height".into());
    }
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err("EFBIG: image too large".into());
    }
    let pixels = w as usize * h as usize;
    if data.len() < pixels * bytes_per_pixel {
        return Err("ENODATA: insufficient image data".into());
    }
    let rgba = if bytes_per_pixel == 4 {
        let mut data = data;
        data.truncate(pixels * 4);
        data
    } else {
        data.as_chunks::<3>().0[..pixels]
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect()
    };
    Ok((w, h, rgba))
}

fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let bad = |e: png::DecodingError| format!("EBADPNG: {e}");
    let mut decoder =
        png::Decoder::new_with_limits(std::io::Cursor::new(data), png::Limits { bytes: MAX_DATA });
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(bad)?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err("EFBIG: image too large".into());
    }
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("EFBIG: image too large")?
    ];
    let info = reader.next_frame(&mut buf).map_err(bad)?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        _ => return Err("EBADPNG: unexpected color type".into()),
    };
    Ok((info.width, info.height, rgba))
}

/// What a placeholder cell shows: a cell of an image's virtual placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceholderCell {
    pub image: u32,
    pub placement: u32,
    pub row: u32,
    pub col: u32,
}

/// The number a cell's color encodes: 24 bits of RGB, or a palette index.
fn color_number(color: Color) -> u32 {
    match color {
        Color::Default => 0,
        Color::Indexed(i) => u32::from(i),
        Color::Rgb(r, g, b) => u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
    }
}

/// Decodes a placeholder cell: the foreground color holds the image id
/// (with the third diacritic as its high byte), its `underline_color` the
/// placement id, and the first two diacritics in `marks` the row and
/// column. Omitted diacritics continue the cell to the left, `left`.
pub fn decode_placeholder(
    cell: &Cell,
    marks: &str,
    underline_color: Color,
    left: Option<PlaceholderCell>,
) -> PlaceholderCell {
    let mut values = marks
        .chars()
        .map(|c| DIACRITICS.binary_search(&c).ok().map(|i| i as u32));
    let (row, col, high) = (
        values.next().flatten(),
        values.next().flatten(),
        values.next().flatten(),
    );
    let low = color_number(cell.fg);
    let placement = color_number(underline_color);
    // The left cell continues into this one if it shows the same placement.
    let left = left.filter(|l| l.image & 0xff_ffff == low && l.placement == placement);
    let row = row.unwrap_or_else(|| left.map_or(0, |l| l.row));
    let col = col.unwrap_or_else(|| left.filter(|l| l.row == row).map_or(0, |l| l.col + 1));
    let high = high.unwrap_or_else(|| {
        left.filter(|l| l.row == row && l.col + 1 == col)
            .map_or(0, |l| l.image >> 24)
    });
    PlaceholderCell {
        image: high << 24 | low,
        placement,
        row,
        col,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ctx() -> Context {
        Context {
            top: 0,
            rows: 24,
            cursor: (0, 0),
        }
    }

    fn run(g: &mut Graphics, command: &str) -> Outcome {
        g.command(command.as_bytes(), &ctx())
    }

    #[test]
    fn reading_a_fifo_fails_instead_of_blocking() {
        let dir = std::env::temp_dir().join(format!("jotty-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe");
        let c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `c` is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (done, wait) = std::sync::mpsc::channel();
        let path = fifo;
        std::thread::spawn(move || done.send(read_file(&path, 0, 0)).unwrap());
        let result = wait.recv_timeout(Duration::from_secs(5));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(result.expect("read_file blocked on a FIFO").is_err());
    }

    #[test]
    fn image_and_placement_counts_are_bounded() {
        let mut g = Graphics::default();
        for i in 1..=MAX_IMAGES as u32 + 10 {
            run(&mut g, &format!("a=t,i={i},f=24,s=1,v=1;AAAA"));
        }
        assert_eq!(g.images.len(), MAX_IMAGES);
        for _ in 0..MAX_PLACEMENTS + 10 {
            run(&mut g, &format!("a=p,i={}", MAX_IMAGES + 10));
        }
        assert_eq!(g.placements.len(), MAX_PLACEMENTS);
    }

    #[test]
    fn deleting_by_id_zero_keeps_anonymous_images() {
        let mut g = Graphics::default();
        run(&mut g, "a=T,f=24,s=1,v=1;AAAA");
        run(&mut g, "a=d,d=i");
        assert_eq!(g.placements.len(), 1);
    }

    #[test]
    fn diacritics_are_sorted() {
        assert!(DIACRITICS.is_sorted());
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64(b"AAAA").unwrap(), [0, 0, 0]);
        assert_eq!(base64(b"aGk=").unwrap(), b"hi");
        assert!(base64(b"a*").is_none());
    }

    #[test]
    fn query_replies_without_storing() {
        let mut g = Graphics::default();
        let out = run(&mut g, "i=31,s=1,v=1,a=q,t=d,f=24;AAAA");
        assert_eq!(out.response.as_deref(), Some("\x1b_Gi=31;OK\x1b\\"));
        assert!(g.is_empty());
        let out = run(&mut g, "i=31,s=2,v=2,a=q,t=d,f=24;AAAA");
        assert!(out.response.unwrap().contains("ENODATA"));
    }

    #[test]
    fn chunked_transmission_then_put() {
        let mut g = Graphics::default();
        // 2x1 RGB: six bytes in two chunks.
        assert_eq!(
            run(&mut g, "a=t,i=5,f=24,s=2,v=1,q=2,m=1;AAAA"),
            Outcome::default()
        );
        assert_eq!(run(&mut g, "m=0;AAAA"), Outcome::default());
        let image = g.image_by_id(5).unwrap();
        assert_eq!((image.width, image.height, image.pixels.len()), (2, 1, 8));

        let out = run(&mut g, "a=p,i=5,p=7,c=3,r=2");
        assert_eq!(out.response.as_deref(), Some("\x1b_Gi=5,p=7;OK\x1b\\"));
        assert_eq!(out.moved, Some((3, 2)));
        // The same placement id replaces the placement.
        let out = run(&mut g, "a=p,i=5,p=7,C=1,q=1");
        assert_eq!(out, Outcome::default());
        assert_eq!(g.placements().len(), 1);
    }

    #[test]
    fn put_of_missing_image_fails() {
        let mut g = Graphics::default();
        let out = run(&mut g, "a=p,i=9");
        assert!(out.response.unwrap().starts_with("\x1b_Gi=9;ENOENT"));
    }

    #[test]
    fn number_gets_an_id() {
        let mut g = Graphics::default();
        let out = run(&mut g, "a=t,I=3,f=24,s=1,v=1;AAAA");
        let response = out.response.unwrap();
        assert!(response.contains(",I=3;OK"), "{response}");
        assert!(!response.starts_with("\x1b_Gi=0"));
    }

    #[test]
    fn source_rect_sizes_the_placement() {
        let mut g = Graphics {
            cell: (10, 20),
            ..Graphics::default()
        };
        run(
            &mut g,
            "a=t,i=1,f=32,s=25,v=30,o=z;eJztwTEBAAAAwqD1T20ND6AAAIB3Awu4AAE=",
        );
        let out = run(&mut g, "a=p,i=1,x=5,w=100");
        assert_eq!(out.moved, Some((2, 2)));
        assert_eq!(g.placements()[0].source, [5, 0, 20, 30]);
        // Only rows: the width follows the aspect ratio, 40×60 pixels.
        let out = run(&mut g, "a=p,i=1,x=5,r=3");
        assert_eq!(out.moved, Some((4, 3)));
        assert_eq!(g.placements()[1].pixel_size(g.cell), (40.0, 60.0));
    }

    #[test]
    fn delete_by_id_and_free_with_upper_case() {
        let mut g = Graphics::default();
        run(&mut g, "a=T,i=1,f=24,s=1,v=1;AAAA");
        run(&mut g, "a=T,i=2,f=24,s=1,v=1;AAAA");
        run(&mut g, "a=d,d=i,i=1");
        assert_eq!(g.placements().len(), 1);
        assert!(g.image_by_id(1).is_some());
        run(&mut g, "a=d,d=I,i=2");
        assert!(g.image_by_id(2).is_none());
        run(&mut g, "a=d,d=A");
        assert!(g.placements().is_empty());
    }

    #[test]
    fn anonymous_images_go_with_their_placements() {
        let mut g = Graphics::default();
        let out = run(&mut g, "a=T,f=24,s=1,v=1;AAAA");
        assert_eq!(out.response, None);
        assert!(!g.is_empty());
        run(&mut g, "a=d");
        assert!(g.is_empty());
    }

    #[test]
    fn scrolling_moves_and_drops_placements() {
        let mut g = Graphics::default();
        run(&mut g, "a=T,i=1,f=24,s=1,v=1;AAAA");
        g.shift(0, 3);
        assert_eq!(g.placements()[0].line, 3);
        g.scroll(2, 5, 1);
        assert_eq!(g.placements()[0].line, 2);
        g.scroll(2, 5, 1);
        assert!(g.placements().is_empty());
    }

    #[test]
    fn placeholder_cells_inherit_from_the_left() {
        let cell = Cell {
            c: PLACEHOLDER,
            fg: Color::Rgb(0, 0, 42),
            ..Cell::default()
        };
        let first = decode_placeholder(&cell, "\u{0305}\u{030D}\u{030E}", Color::Default, None);
        assert_eq!(
            first,
            PlaceholderCell {
                image: 2 << 24 | 42,
                placement: 0,
                row: 0,
                col: 1,
            }
        );
        let next = decode_placeholder(&cell, "", Color::Default, Some(first));
        assert_eq!((next.image, next.row, next.col), (first.image, 0, 2));
    }
}
