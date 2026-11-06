//! Rendering: turns the screen into a list of textured quads.
//!
//! [`build`] is GPU-independent: it produces a [`Batch`] of vertices for
//! cell backgrounds, glyphs, decorations, the cursor and the input-method
//! preedit. [`gl`] uploads and draws it with OpenGL ES 3 on an EGL surface;
//! [`software`] draws it on the CPU into shared-memory buffers.

pub mod atlas;
pub mod gl;
pub mod software;

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::{Config, Rgb};
use crate::font::{CellMetrics, Style, is_icon};
use crate::ime::Preedit;
use crate::selection::{Point, Selection};
use crate::terminal::graphics::{self, PLACEHOLDER, PlaceholderCell};
use crate::terminal::{Cell, Color, CursorShape, Flags, Screen, Underline};

/// One of the two backends, which draw the same [`Batch`].
pub enum Renderer {
    Gpu(Box<gl::Renderer>),
    Cpu(Box<software::Renderer>),
}

impl Renderer {
    pub fn cell(&self) -> CellMetrics {
        match self {
            Renderer::Gpu(r) => r.cell(),
            Renderer::Cpu(r) => r.cell(),
        }
    }

    /// Resizes the drawable, in physical pixels.
    pub fn resize(&mut self, size: (u32, u32)) {
        match self {
            Renderer::Gpu(r) => r.resize(size),
            Renderer::Cpu(r) => r.resize(size),
        }
    }

    pub fn set_font_size(&mut self, px: f32) {
        match self {
            Renderer::Gpu(r) => r.set_font_size(px),
            Renderer::Cpu(r) => r.set_font_size(px),
        }
    }

    pub fn set_fonts(&mut self, fonts: crate::font::Fonts) {
        match self {
            Renderer::Gpu(r) => r.set_fonts(fonts),
            Renderer::Cpu(r) => r.set_fonts(fonts),
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        match self {
            Renderer::Gpu(r) => r.theme = theme,
            Renderer::Cpu(r) => r.theme = theme,
        }
    }

    /// Draws and presents a frame (this commits the surface). Returns
    /// whether another frame is needed because the glyph atlas overflowed.
    pub fn draw(&mut self, frame: &Frame) -> Result<bool, String> {
        match self {
            Renderer::Gpu(r) => r.draw(frame),
            Renderer::Cpu(r) => r.draw(frame),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    /// Color of the character under a block cursor.
    pub cursor_text: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,
    pub selection_background: Option<Rgb>,
    pub palette: [Rgb; 16],
    pub bold_is_bright: bool,
    /// Opacity of the default background.
    pub opacity: f32,
    /// Also apply `opacity` to cells with an explicit background color.
    pub transparent_cells: bool,
    /// 256-color entries the program changed with OSC 4.
    pub overrides: [Option<Rgb>; 256],
}

impl Theme {
    pub fn from_config(config: &Config) -> Theme {
        Theme {
            foreground: config.colors.foreground,
            background: config.colors.background,
            cursor: config.colors.cursor(),
            cursor_text: config.colors.cursor_text,
            selection_foreground: config.colors.selection_foreground,
            selection_background: config.colors.selection_background,
            palette: config.colors.palette(),
            bold_is_bright: config.terminal.bold_is_bright,
            opacity: config.window.opacity,
            transparent_cells: config.window.transparent_cell_backgrounds,
            overrides: [None; 256],
        }
    }

    /// The configured colors, as color queries report them.
    pub fn default_colors(&self) -> crate::terminal::DefaultColors {
        let rgb = |c: Rgb| (c.0, c.1, c.2);
        crate::terminal::DefaultColors {
            foreground: rgb(self.foreground),
            background: rgb(self.background),
            cursor: rgb(self.cursor),
            palette: std::array::from_fn(|i| rgb(self.indexed(i as u8))),
        }
    }

    /// The theme with the colors the program changed (OSC 4, 10, 11).
    pub fn effective(&self, screen: &Screen) -> Cow<'_, Theme> {
        let colors = screen.color_overrides();
        if colors.is_empty() {
            return Cow::Borrowed(self);
        }
        let rgb = |(r, g, b): (u8, u8, u8)| Rgb(r, g, b);
        let mut theme = self.clone();
        if let Some(c) = colors.foreground {
            theme.foreground = rgb(c);
        }
        if let Some(c) = colors.background {
            theme.background = rgb(c);
        }
        theme.overrides = colors.palette.map(|c| c.map(rgb));
        Cow::Owned(theme)
    }

    /// Resolves a 256-color index.
    pub fn indexed(&self, i: u8) -> Rgb {
        if let Some(c) = self.overrides[usize::from(i)] {
            return c;
        }
        match i {
            0..=15 => self.palette[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            232..=255 => {
                let v = 8 + (i - 232) * 10;
                Rgb(v, v, v)
            }
        }
    }

    /// Final foreground and background of a cell. `block_cursor` is the
    /// cursor color when a block cursor covers the cell.
    pub fn cell_colors(
        &self,
        cell: &Cell,
        selected: bool,
        block_cursor: Option<Rgb>,
    ) -> (Rgb, Rgb) {
        let mut fg = match cell.fg {
            Color::Default => self.foreground,
            Color::Indexed(i @ 0..=7)
                if self.bold_is_bright && cell.flags.contains(Flags::BOLD) =>
            {
                self.indexed(i + 8)
            }
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => Rgb(r, g, b),
        };
        let mut bg = match cell.bg {
            Color::Default => self.background,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => Rgb(r, g, b),
        };
        if cell.flags.contains(Flags::DIM) {
            fg = blend(fg, bg, 0.66);
        }
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if selected {
            // Each selection color defaults to inverse video, like foot.
            let (inverse_fg, inverse_bg) = (bg, fg);
            fg = self.selection_foreground.unwrap_or(inverse_fg);
            bg = self.selection_background.unwrap_or(inverse_bg);
        }
        if let Some(cursor) = block_cursor {
            // Without a configured text color, the character takes the
            // cell's background, falling back to the default background
            // when that would make it invisible.
            fg = match self.cursor_text {
                Some(text) => text,
                None if bg == cursor => self.background,
                None => bg,
            };
            bg = cursor;
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        (fg, bg)
    }
}

/// Mixes `a` into `b` with weight `t` for `a`.
fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let mix = |x: u8, y: u8| (f32::from(x) * t + f32::from(y) * (1.0 - t)).round() as u8;
    Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// A glyph's location in the atlas and placement relative to its cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphInfo {
    /// Normalized texture coordinates: left, top, right, bottom.
    pub uv: [f32; 4],
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    /// In the color (RGBA) atlas: drawn as is, not tinted.
    pub color: bool,
}

pub trait GlyphCache {
    /// The glyph for `c` in a one-cell slot, or a two-cell slot when `wide`.
    fn glyph(&mut self, c: char, style: Style, wide: bool) -> Option<GlyphInfo>;

    /// An underline bitmap of `style` spanning `cells` cells.
    fn underline(&mut self, style: Underline, cells: u32) -> Option<GlyphInfo>;

    /// The glyph for a grapheme cluster (a character with combining marks,
    /// or an emoji sequence).
    fn cluster(&mut self, text: &str, style: Style, wide: bool) -> Option<GlyphInfo>;
}

/// Floats per vertex: position (2), texture coordinate (2), color (4), mode (1).
pub const VERTEX_FLOATS: usize = 9;

#[derive(Debug, Default)]
pub struct Batch {
    pub vertices: Vec<f32>,
    /// The image quads, in drawing order: the index of the first vertex
    /// and the image's uid.
    pub images: Vec<(usize, u64)>,
}

impl Batch {
    /// Adds a quad covering `[x, y, width, height]`; `mode` 0 is a solid
    /// color, 1 samples the glyph atlas at `uv`, 2 the color atlas, 3 an
    /// image.
    fn quad(&mut self, [x, y, w, h]: [f32; 4], uv: [f32; 4], color: Rgb, alpha: f32, mode: f32) {
        let [r, g, b] = color.to_f32();
        let [u0, v0, u1, v1] = uv;
        let corners = [
            (x, y, u0, v0),
            (x + w, y, u1, v0),
            (x, y + h, u0, v1),
            (x + w, y, u1, v0),
            (x + w, y + h, u1, v1),
            (x, y + h, u0, v1),
        ];
        for (px, py, u, v) in corners {
            self.vertices
                .extend_from_slice(&[px, py, u, v, r, g, b, alpha, mode]);
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        self.fill(x, y, w, h, color, 1.0);
    }

    /// A solid rectangle with the given opacity.
    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb, alpha: f32) {
        self.quad([x, y, w, h], [0.0; 4], color, alpha, 0.0);
    }

    /// A glyph at a cell's top-left corner; mode 1 tints a coverage
    /// bitmap with `color`, mode 2 draws a color emoji.
    pub fn glyph(&mut self, x: f32, y: f32, glyph: &GlyphInfo, color: Rgb) {
        let (w, h) = (glyph.width as f32, glyph.height as f32);
        let mode = if glyph.color { 2.0 } else { 1.0 };
        self.quad(
            [x + glyph.left as f32, y + glyph.top as f32, w, h],
            glyph.uv,
            color,
            1.0,
            mode,
        );
    }

    /// Part of an image: `uv` normalized within it, drawn into `rect`.
    fn image(&mut self, rect: [f32; 4], uv: [f32; 4], uid: u64) {
        self.images.push((self.vertices.len() / VERTEX_FLOATS, uid));
        self.quad(rect, uv, Rgb(255, 255, 255), 1.0, 3.0);
    }

    #[cfg(test)]
    pub fn quads(&self) -> usize {
        self.vertices.len() / (6 * VERTEX_FLOATS)
    }

    fn append(&mut self, mut other: Batch) {
        let base = self.vertices.len() / VERTEX_FLOATS;
        self.images
            .extend(other.images.iter().map(|&(first, uid)| (base + first, uid)));
        self.vertices.append(&mut other.vertices);
    }
}

/// Images with a z-index below this are drawn under cell backgrounds;
/// others below 0 between backgrounds and text, the rest over text.
const BELOW_BACKGROUNDS: i32 = i32::MIN / 2;

/// Part of an image to draw.
struct ImageQuad {
    z: i32,
    uid: u64,
    rect: [f32; 4],
    uv: [f32; 4],
}

impl ImageQuad {
    /// The part of `source` (pixels of an image of `size`) drawn into
    /// `rect`, cut to `clip`; `None` when nothing is left.
    fn clipped(
        rect: [f32; 4],
        source: [f32; 4],
        size: (u32, u32),
        clip: [f32; 4],
    ) -> Option<([f32; 4], [f32; 4])> {
        let [x, y, w, h] = rect;
        let x0 = x.max(clip[0]);
        let y0 = y.max(clip[1]);
        let x1 = (x + w).min(clip[0] + clip[2]);
        let y1 = (y + h).min(clip[1] + clip[3]);
        if x1 <= x0 || y1 <= y0 || w <= 0.0 || h <= 0.0 {
            return None;
        }
        let [sx, sy, sw, sh] = source;
        let (iw, ih) = (size.0 as f32, size.1 as f32);
        let u = |px: f32| (sx + (px - x) / w * sw) / iw;
        let v = |py: f32| (sy + (py - y) / h * sh) / ih;
        Some(([x0, y0, x1 - x0, y1 - y0], [u(x0), v(y0), u(x1), v(y1)]))
    }
}

/// The parts of images that placements show on the visible rows.
fn placement_quads(frame: &Frame, m: CellMetrics, out: &mut Vec<ImageQuad>) {
    let screen = frame.screen;
    let images = screen.graphics();
    let (cw, ch) = (m.width as f32, m.height as f32);
    let (ox, oy) = (frame.origin.0 as f32, frame.origin.1 as f32);
    let clip = [ox, oy, screen.cols() as f32 * cw, screen.rows() as f32 * ch];
    // The line shown in the top row.
    let top = screen.grid().scrolled as i64 - screen.display_offset() as i64;
    for p in images.placements().iter().filter(|p| !p.is_virtual) {
        let Some(image) = images.image(p.image) else {
            continue;
        };
        let (dx, dy) = (p.offset.0 as f32, p.offset.1 as f32);
        let x = ox + p.col as f32 * cw + dx;
        let y = oy + (p.line - top) as f32 * ch + dy;
        let [sx, sy, sw, sh] = p.source.map(|v| v as f32);
        let (w, h) = p.pixel_size((m.width, m.height));
        if let Some((rect, uv)) = ImageQuad::clipped(
            [x, y, w, h],
            [sx, sy, sw, sh],
            (image.width, image.height),
            clip,
        ) {
            out.push(ImageQuad {
                z: p.z,
                uid: image.uid,
                rect,
                uv,
            });
        }
    }
}

/// The part of an image a placeholder cell at `(px, py)` shows. The image
/// is fit into its virtual placement's cells, keeping its aspect ratio, and
/// centered.
fn placeholder_quad(
    screen: &Screen,
    cell: PlaceholderCell,
    (px, py): (f32, f32),
    m: CellMetrics,
) -> Option<ImageQuad> {
    let images = screen.graphics();
    let image = images.image_by_id(cell.image)?;
    let (cw, ch) = (m.width as f32, m.height as f32);
    let placement = images.virtual_placement(image.uid, cell.placement);
    let source = placement.map_or([0, 0, image.width, image.height], |p| p.source);
    let (cols, rows) = match placement {
        Some(p) => p.size_in_cells((m.width, m.height)),
        None => (
            image.width.div_ceil(m.width.max(1)),
            image.height.div_ceil(m.height.max(1)),
        ),
    };
    let [sx, sy, sw, sh] = source.map(|v| v as f32);
    // Nothing to show, and the scale below would be NaN.
    if sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    let (bw, bh) = (cols as f32 * cw, rows as f32 * ch);
    let scale = (bw / sw).min(bh / sh);
    let (w, h) = (sw * scale, sh * scale);
    // The image's rectangle with this cell's top-left corner at (px, py).
    let x = px - cell.col as f32 * cw + (bw - w) / 2.0;
    let y = py - cell.row as f32 * ch + (bh - h) / 2.0;
    let (rect, uv) = ImageQuad::clipped(
        [x, y, w, h],
        [sx, sy, sw, sh],
        (image.width, image.height),
        [px, py, cw, ch],
    )?;
    Some(ImageQuad {
        z: placement.map_or(0, |p| p.z),
        uid: image.uid,
        rect,
        uv,
    })
}

/// Everything besides the screen that affects what is drawn.
pub struct Frame<'a> {
    pub screen: &'a Screen,
    pub selection: Option<&'a Selection>,
    pub preedit: Option<&'a Preedit>,
    pub focused: bool,
    /// Grid origin in physical pixels.
    pub origin: (u32, u32),
    /// First and last cell of a link to underline (Ctrl is held over it).
    pub link: Option<(Point, Point)>,
    /// A cell marked by Shift+click, drawn like the selection.
    pub mark: Option<Point>,
    /// The vi mode cursor, drawn as a block instead of the terminal's.
    pub vi_cursor: Option<Point>,
    /// A short label drawn in the top-right corner, such as vi mode's.
    pub label: Option<&'a str>,
    /// The search bar's text, drawn over the bottom row while searching.
    pub search: Option<&'a str>,
}

fn style_of(cell: &Cell) -> Style {
    Style::new(
        cell.flags.contains(Flags::BOLD),
        cell.flags.contains(Flags::ITALIC),
    )
}

pub fn build(frame: &Frame, theme: &Theme, m: CellMetrics, glyphs: &mut impl GlyphCache) -> Batch {
    let screen = frame.screen;
    let (cw, ch) = (m.width as f32, m.height as f32);
    let (ox, oy) = (frame.origin.0 as f32, frame.origin.1 as f32);
    let stroke = m.stroke as f32;
    let mut back = Batch::default();
    let mut front = Batch::default();

    let cursor_visible =
        screen.cursor_visible() && frame.preedit.is_none() && frame.vi_cursor.is_none();
    let (cursor_row, cursor_col) = screen.cursor();
    let block_cursor = cursor_visible && frame.focused && screen.cursor_shape == CursorShape::Block;
    let cursor_color = screen
        .cursor_color()
        .map_or(theme.cursor, |(r, g, b)| Rgb(r, g, b));
    let offset = screen.display_offset() as isize;
    let mut images = Vec::new();
    if !screen.graphics().is_empty() {
        placement_quads(frame, m, &mut images);
    }

    // Reused: this runs for every clustered cell of every frame.
    let mut cluster_text = String::new();
    for y in 0..screen.rows() {
        let row = screen.visible_row(y);
        let line = y as isize - offset;
        let py = oy + y as f32 * ch;
        // The placeholder cell to the left, which the next one continues.
        let mut placeholder: Option<PlaceholderCell> = None;
        for x in 0..screen.cols() {
            let cell = row.cells.get(x).copied().unwrap_or_default();
            if cell.flags.contains(Flags::WIDE_SPACER) {
                continue;
            }
            let shown = (cell.c == PLACEHOLDER).then(|| {
                let decoded =
                    graphics::decode_placeholder(&cell, screen.cluster(cell.cluster), placeholder);
                let px = ox + x as f32 * cw;
                images.extend(placeholder_quad(screen, decoded, (px, py), m));
                decoded
            });
            placeholder = shown;
            let span = if cell.flags.contains(Flags::WIDE) {
                2.0
            } else {
                1.0
            };
            let selected = frame
                .selection
                .is_some_and(|s| s.contains(Point { line, col: x }))
                || frame.mark == Some(Point { line, col: x });
            let is_cursor = match frame.vi_cursor {
                Some(vi) => vi == Point { line, col: x },
                None => block_cursor && y == cursor_row && x == cursor_col,
            };
            let (fg, bg) = theme.cell_colors(&cell, selected, is_cursor.then_some(cursor_color));
            let px = ox + x as f32 * cw;

            // Whether the program chose this background, as opposed to the
            // default one showing through. This depends on how the cell was
            // set, not on the color: themes often paint the same color as
            // the configured background, and that should stay opaque.
            let explicit_bg = cell.bg != Color::Default
                || cell.flags.contains(Flags::INVERSE)
                || selected
                || is_cursor;
            if explicit_bg {
                // Selection and cursor stay opaque so they remain visible.
                let alpha = if theme.transparent_cells && !selected && !is_cursor {
                    theme.opacity
                } else {
                    1.0
                };
                back.fill(px, py, cw * span, ch, bg, alpha);
            }
            let glyph = if cell.flags.intersects(Flags::HIDDEN | Flags::CONTINUATION)
                || cell.c == PLACEHOLDER
            {
                None
            } else if cell.cluster != 0 {
                cluster_text.clear();
                screen.cell_text(&cell, &mut cluster_text);
                // Emoji sequences span several cells; draw them two wide.
                let wide = span > 1.0
                    || row
                        .cells
                        .get(x + 1)
                        .is_some_and(|c| c.flags.contains(Flags::CONTINUATION));
                glyphs.cluster(&cluster_text, style_of(&cell), wide)
            } else if cell.c != ' ' {
                // A Nerd Font icon followed by a space gets both cells, as in
                // kitty and Ghostty; otherwise it is shrunk to fit its own.
                let wide = span > 1.0
                    || is_icon(cell.c)
                        && row.cells.get(x + 1).is_some_and(|next| {
                            next.c == ' '
                                && next.cluster == 0
                                && !next.flags.intersects(Flags::WIDE | Flags::WIDE_SPACER)
                        });
                glyphs.glyph(cell.c, style_of(&cell), wide)
            } else {
                None
            };
            if let Some(glyph) = glyph {
                front.glyph(px, py, &glyph, fg);
            }
            if cell.underline != Underline::None
                && let Some(glyph) = glyphs.underline(cell.underline, span as u32)
            {
                let color = match cell.underline_color {
                    Color::Default => fg,
                    Color::Indexed(i) => theme.indexed(i),
                    Color::Rgb(r, g, b) => Rgb(r, g, b),
                };
                front.glyph(px, py, &glyph, color);
            }
            let point = Point { line, col: x };
            if cell.underline == Underline::None
                && frame
                    .link
                    .is_some_and(|(start, end)| start <= point && point <= end)
            {
                front.rect(px, py + m.underline as f32, cw * span, stroke, fg);
            }
            if cell.flags.contains(Flags::STRIKE) {
                let mid = (m.baseline as f32 * 0.65).round();
                front.rect(px, py + mid, cw * span, stroke, fg);
            }
        }
    }

    if cursor_visible && !block_cursor {
        let px = ox + cursor_col as f32 * cw;
        let py = oy + cursor_row as f32 * ch;
        let beam = (stroke * 2.0).max(1.0).min(cw);
        match (frame.focused, screen.cursor_shape) {
            (true, CursorShape::Beam) => front.rect(px, py, beam, ch, cursor_color),
            (true, CursorShape::Underline) => {
                front.rect(px, py + ch - beam, cw, beam, cursor_color)
            }
            _ => {
                // Hollow block for an unfocused window.
                front.rect(px, py, cw, stroke, cursor_color);
                front.rect(px, py + ch - stroke, cw, stroke, cursor_color);
                front.rect(px, py, stroke, ch, cursor_color);
                front.rect(px + cw - stroke, py, stroke, ch, cursor_color);
            }
        }
    }

    let mut overlay = Batch::default();
    if let Some(label) = frame.label {
        draw_label(&mut overlay, label, frame, theme, m, glyphs);
    }
    if let Some(text) = frame.search {
        draw_search_bar(&mut overlay, text, frame, theme, m, glyphs);
    } else if let Some(preedit) = frame.preedit {
        draw_preedit(&mut overlay, preedit, frame, theme, cursor_color, m, glyphs);
    }

    // Images go in three layers by z-index: under the cell backgrounds,
    // under the text, and over it; in placement order within a z-index.
    images.sort_by_key(|i| i.z);
    let mut batch = Batch::default();
    let mut images = images.into_iter().peekable();
    for (layer, end) in [(back, BELOW_BACKGROUNDS), (front, 0), (overlay, i32::MAX)] {
        while let Some(image) = images.next_if(|i| i.z < end) {
            batch.image(image.rect, image.uv, image.uid);
        }
        batch.append(layer);
    }
    for image in images {
        batch.image(image.rect, image.uv, image.uid);
    }
    batch
}

/// Draws `text` over the right end of the top row, in inverse colors.
fn draw_label(
    batch: &mut Batch,
    text: &str,
    frame: &Frame,
    theme: &Theme,
    m: CellMetrics,
    glyphs: &mut impl GlyphCache,
) {
    let cols = frame.screen.cols();
    let width: usize = text.chars().filter_map(|c| c.width()).sum();
    // A cell of padding on each side.
    let start = cols.saturating_sub(width + 2);
    let (cw, ch) = (m.width as f32, m.height as f32);
    let x = |col: usize| frame.origin.0 as f32 + col as f32 * cw;
    let py = frame.origin.1 as f32;
    batch.rect(
        x(start),
        py,
        (cols - start) as f32 * cw,
        ch,
        theme.foreground,
    );
    let mut col = start + 1;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if w == 0 {
            continue;
        }
        if col + w > cols {
            break;
        }
        if let Some(glyph) = glyphs.glyph(c, Style::Bold, w == 2) {
            batch.glyph(x(col), py, &glyph, theme.background);
        }
        col += w;
    }
}

/// Draws the search bar over the bottom row, in inverse colors, with a
/// beam cursor after the text.
fn draw_search_bar(
    batch: &mut Batch,
    text: &str,
    frame: &Frame,
    theme: &Theme,
    m: CellMetrics,
    glyphs: &mut impl GlyphCache,
) {
    let screen = frame.screen;
    let (cw, ch) = (m.width as f32, m.height as f32);
    let x = |col: usize| frame.origin.0 as f32 + col as f32 * cw;
    let py = frame.origin.1 as f32 + (screen.rows() - 1) as f32 * ch;
    batch.rect(x(0), py, screen.cols() as f32 * cw, ch, theme.foreground);
    let mut col = 0;
    for c in text.chars() {
        let width = c.width().unwrap_or(0);
        if width == 0 {
            continue;
        }
        if col + width > screen.cols() {
            return;
        }
        if c != ' '
            && let Some(glyph) = glyphs.glyph(c, Style::Regular, width == 2)
        {
            batch.glyph(x(col), py, &glyph, theme.background);
        }
        col += width;
    }
    if col < screen.cols() {
        let beam = (m.stroke as f32 * 2.0).max(1.0);
        batch.rect(x(col), py, beam, ch, theme.background);
    }
}

/// Draws the preedit at the cursor, over the grid, clipped at the line end.
fn draw_preedit(
    batch: &mut Batch,
    preedit: &Preedit,
    frame: &Frame,
    theme: &Theme,
    cursor_color: Rgb,
    m: CellMetrics,
    glyphs: &mut impl GlyphCache,
) {
    let screen = frame.screen;
    let (row, start) = screen.cursor();
    let (cw, ch) = (m.width as f32, m.height as f32);
    let py = frame.origin.1 as f32 + row as f32 * ch;
    let col_x = |col: usize| frame.origin.0 as f32 + col as f32 * cw;

    let mut col = start;
    let mut cursor_col = None;
    // By grapheme: an input method may send a letter and its tone mark apart.
    for (i, text) in preedit.text.grapheme_indices(true) {
        if preedit
            .cursor
            .is_some_and(|(begin, _)| (i..i + text.len()).contains(&begin))
        {
            cursor_col = Some(col);
        }
        let width = text.width().min(2);
        if width == 0 {
            continue;
        }
        if col + width > screen.cols() {
            break;
        }
        let w = cw * width as f32;
        batch.fill(col_x(col), py, w, ch, theme.background, theme.opacity);
        let mut chars = text.chars();
        let glyph = match (chars.next(), chars.next()) {
            (Some(c), None) => glyphs.glyph(c, Style::Regular, width == 2),
            _ => glyphs.cluster(text, Style::Regular, width == 2),
        };
        if let Some(glyph) = glyph {
            batch.glyph(col_x(col), py, &glyph, theme.foreground);
        }
        batch.rect(
            col_x(col),
            py + m.underline as f32,
            w,
            m.stroke as f32,
            theme.foreground,
        );
        col += width;
    }
    if preedit
        .cursor
        .is_some_and(|(begin, _)| begin >= preedit.text.len())
    {
        cursor_col = Some(col);
    }
    if let Some(col) = cursor_col.filter(|&c| c < screen.cols()) {
        let beam = (m.stroke as f32 * 2.0).max(1.0);
        batch.rect(col_x(col), py, beam, ch, cursor_color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    const METRICS: CellMetrics = CellMetrics {
        width: 10,
        height: 20,
        baseline: 15,
        underline: 17,
        stroke: 1,
    };

    /// Records requested glyphs and underlines; every glyph is a 1×1 bitmap
    /// at the cell origin.
    #[derive(Default)]
    /// Records the glyphs drawn, the underlines, and the glyphs drawn in a
    /// two-cell slot.
    struct FakeGlyphs(
        Vec<(char, Style)>,
        Vec<(Underline, u32)>,
        Vec<char>,
        Vec<String>,
    );

    impl GlyphCache for FakeGlyphs {
        fn underline(&mut self, style: Underline, cells: u32) -> Option<GlyphInfo> {
            self.1.push((style, cells));
            Some(GlyphInfo {
                uv: [0.0; 4],
                width: cells,
                height: 1,
                left: 0,
                top: 0,
                color: false,
            })
        }

        fn cluster(&mut self, text: &str, style: Style, wide: bool) -> Option<GlyphInfo> {
            self.3.push(text.to_string());
            self.glyph(text.chars().next()?, style, wide)
        }

        fn glyph(&mut self, c: char, style: Style, wide: bool) -> Option<GlyphInfo> {
            self.0.push((c, style));
            if wide {
                self.2.push(c);
            }
            Some(GlyphInfo {
                uv: [0.0; 4],
                width: 1,
                height: 1,
                left: 0,
                top: 0,
                color: false,
            })
        }
    }

    #[test]
    fn clusters_draw_once_in_their_first_cell() {
        let mut t = Terminal::new(10, 1, 0);
        t.feed("e\u{301}🇻🇳x".as_bytes());
        let mut glyphs = FakeGlyphs::default();
        build(&frame(t.screen()), &theme(), METRICS, &mut glyphs);
        let drawn: Vec<char> = glyphs.0.iter().map(|g| g.0).collect();
        assert_eq!(drawn, ['e', '🇻', 'x']);
    }

    #[test]
    fn marked_cell_is_drawn_selected() {
        let mut t = Terminal::new(4, 1, 0);
        t.feed(b"ab\x1b[?25l");
        let theme = theme();
        let mut f = frame(t.screen());
        f.mark = Some(Point { line: 0, col: 1 });
        let batch = build(&f, &theme, METRICS, &mut FakeGlyphs::default());
        // One background quad: the marked cell, in the text color.
        let backgrounds: Vec<_> = batch
            .vertices
            .chunks(6 * VERTEX_FLOATS)
            .filter(|q| q[8] == 0.0)
            .map(|q| (q[0], [q[4], q[5], q[6]]))
            .collect();
        assert_eq!(backgrounds, [(10.0, theme.foreground.to_f32())]);
    }

    fn theme() -> Theme {
        Theme::from_config(&Config::default())
    }

    #[test]
    fn effective_theme_applies_program_colors() {
        let base = theme();
        let mut t = Terminal::new(10, 1, 0);
        assert!(matches!(base.effective(t.screen()), Cow::Borrowed(_)));
        t.feed(b"\x1b]11;#010203\x07\x1b]4;1;#00ff00\x07\x1b]4;100;#0000ff\x07");
        let theme = base.effective(t.screen());
        assert_eq!(theme.background, Rgb(1, 2, 3));
        assert_eq!(theme.foreground, base.foreground);
        assert_eq!(theme.indexed(1), Rgb(0, 255, 0));
        assert_eq!(theme.indexed(100), Rgb(0, 0, 255));
        assert_eq!(theme.indexed(2), base.indexed(2));
    }

    fn frame(screen: &Screen) -> Frame<'_> {
        Frame {
            screen,
            selection: None,
            preedit: None,
            focused: true,
            origin: (0, 0),
            link: None,
            mark: None,
            vi_cursor: None,
            label: None,
            search: None,
        }
    }

    /// Alpha of the quad's first vertex.
    fn alpha_at(batch: &Batch, i: usize) -> f32 {
        batch.vertices[i * 6 * VERTEX_FLOATS + 7]
    }

    /// Color and position of the quad's first vertex.
    fn quad_at(batch: &Batch, i: usize) -> ((f32, f32), [f32; 3], f32) {
        let v = &batch.vertices[i * 6 * VERTEX_FLOATS..];
        ((v[0], v[1]), [v[4], v[5], v[6]], v[8])
    }

    #[test]
    fn indexed_colors() {
        let t = theme();
        assert_eq!(t.indexed(3), t.palette[3]);
        assert_eq!(t.indexed(16), Rgb(0, 0, 0));
        assert_eq!(t.indexed(196), Rgb(255, 0, 0));
        assert_eq!(t.indexed(231), Rgb(255, 255, 255));
        assert_eq!(t.indexed(232), Rgb(8, 8, 8));
        assert_eq!(t.indexed(255), Rgb(238, 238, 238));
    }

    #[test]
    fn cell_color_attributes() {
        let t = theme();
        let mut cell = Cell {
            fg: Color::Indexed(1),
            ..Cell::default()
        };
        assert_eq!(
            t.cell_colors(&cell, false, None),
            (t.palette[1], t.background)
        );
        cell.flags.insert(Flags::BOLD);
        assert_eq!(t.cell_colors(&cell, false, None).0, t.palette[9]);
        cell.flags.insert(Flags::INVERSE);
        assert_eq!(
            t.cell_colors(&cell, false, None),
            (t.background, t.palette[9])
        );
        // Selecting inverted text un-inverts it.
        assert_eq!(
            t.cell_colors(&cell, true, None),
            (t.palette[9], t.background)
        );

        let hidden = Cell {
            flags: Flags::HIDDEN,
            ..Cell::default()
        };
        let (fg, bg) = t.cell_colors(&hidden, false, None);
        assert_eq!(fg, bg);
    }

    #[test]
    fn selection_colors() {
        let mut t = theme();
        let red = Cell {
            fg: Color::Indexed(1),
            ..Cell::default()
        };
        // Only the background configured: the text is still inverted.
        t.selection_background = Some(Rgb(1, 2, 3));
        assert_eq!(
            t.cell_colors(&red, true, None),
            (t.background, Rgb(1, 2, 3))
        );
        t.selection_foreground = Some(Rgb(4, 5, 6));
        assert_eq!(
            t.cell_colors(&red, true, None),
            (Rgb(4, 5, 6), Rgb(1, 2, 3))
        );
        // Inverse cells use the configured colors too.
        let inverse = Cell {
            flags: Flags::INVERSE,
            ..red
        };
        assert_eq!(
            t.cell_colors(&inverse, true, None),
            (Rgb(4, 5, 6), Rgb(1, 2, 3))
        );
        // Unselected cells are unaffected.
        assert_eq!(
            t.cell_colors(&red, false, None),
            (t.palette[1], t.background)
        );
    }

    #[test]
    fn block_cursor_colors() {
        let t = theme();
        let (fg, bg) = t.cell_colors(&Cell::default(), false, Some(t.cursor));
        assert_eq!(bg, t.cursor);
        assert_eq!(fg, t.background);
    }

    #[test]
    fn cursor_text_color() {
        let mut t = theme();
        t.cursor_text = Some(Rgb(1, 2, 3));
        let cell = Cell {
            fg: Color::Indexed(2),
            ..Cell::default()
        };
        assert_eq!(
            t.cell_colors(&cell, false, Some(Rgb(9, 9, 9))),
            (Rgb(1, 2, 3), Rgb(9, 9, 9))
        );
    }

    #[test]
    fn program_cursor_color_overrides_theme() {
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b]12;#00ff00\x07");
        let batch = build(
            &frame(term.screen()),
            &theme(),
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(quad_at(&batch, 0).1, Rgb(0, 255, 0).to_f32());

        // Other cursor shapes and the unfocused outline use it too.
        term.feed(b"\x1b[6 q");
        let batch = build(
            &frame(term.screen()),
            &theme(),
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(quad_at(&batch, 0).1, Rgb(0, 255, 0).to_f32());

        term.feed(b"\x1b]112\x07");
        let batch = build(
            &frame(term.screen()),
            &theme(),
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(quad_at(&batch, 0).1, theme().cursor.to_f32());
    }

    #[test]
    fn dim_blends_toward_background() {
        let t = theme();
        let cell = Cell {
            flags: Flags::DIM,
            ..Cell::default()
        };
        let fg = t.cell_colors(&cell, false, None).0;
        assert!(fg.0 < t.foreground.0 && fg.0 > t.background.0);
    }

    #[test]
    fn draws_text_and_block_cursor() {
        let mut term = Terminal::new(4, 2, 0);
        term.feed(b"ab");
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        assert_eq!(glyphs.0, [('a', Style::Regular), ('b', Style::Regular)]);
        // Cursor background, then two glyphs.
        assert_eq!(batch.quads(), 3);
        let ((x, y), color, mode) = quad_at(&batch, 0);
        assert_eq!((x, y, mode), (20.0, 0.0, 0.0));
        assert_eq!(color, theme().cursor.to_f32());
    }

    #[test]
    fn icons_followed_by_a_space_get_two_cells() {
        // Arch logo then a space, a powerline arrow right after the second
        // icon, and an icon in the last column.
        let mut term = Terminal::new(6, 2, 0);
        term.feed("\u{F08C7} \u{F43A}\u{E0B0} \u{E7A8}".as_bytes());
        let mut glyphs = FakeGlyphs::default();
        build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        assert_eq!(glyphs.2, ['\u{F08C7}']);
        // Powerline separators are not icons, even before a space.
        let drawn: Vec<char> = glyphs.0.iter().map(|g| g.0).collect();
        assert_eq!(drawn, ['\u{F08C7}', '\u{F43A}', '\u{E0B0}', '\u{E7A8}']);
    }

    /// A terminal with a 2×2 image placed at (row 0, column 1) with `z`.
    fn with_image(z: i32) -> Terminal {
        let mut term = Terminal::new(4, 2, 0);
        term.screen_mut().set_cell_size(10, 20);
        term.feed(b"\x1b_Ga=t,i=1,f=24,s=2,v=2;AAAAAAAAAAAAAAAA\x1b\\");
        term.feed(format!("\x1b[1;2H\x1b_Ga=p,i=1,c=2,r=1,z={z},C=1\x1b\\x").as_bytes());
        term
    }

    #[test]
    fn images_are_layered_by_z_index() {
        let mut glyphs = FakeGlyphs::default();
        // Over the text: last.
        let term = with_image(0);
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        let last = batch.quads() - 1;
        assert_eq!(batch.images.len(), 1);
        assert_eq!(batch.images[0].0, last * 6);
        let ((x, y), _, mode) = quad_at(&batch, last);
        assert_eq!((x, y, mode), (10.0, 0.0, 3.0));
        // Scaled to two cells.
        let v = &batch.vertices[(last * 6 + 4) * VERTEX_FLOATS..];
        assert_eq!((v[0], v[1], v[2], v[3]), (30.0, 20.0, 1.0, 1.0));
        // Under the text: before the glyph.
        let term = with_image(-1);
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        let (_, _, mode) = quad_at(&batch, batch.quads() - 1);
        assert_eq!(mode, 1.0);
        assert_eq!(quad_at(&batch, batch.images[0].0 / 6).2, 3.0);
    }

    #[test]
    fn placeholder_cells_show_their_part_of_the_image() {
        let mut term = Terminal::new(4, 2, 0);
        term.screen_mut().set_cell_size(10, 20);
        term.feed(b"\x1b_Ga=T,U=1,i=5,f=24,s=2,v=2,c=2,r=1;AAAAAAAAAAAAAAAA\x1b\\");
        // Image 5 in the foreground; row 0, columns 0 and 1 (inherited).
        term.feed("\x1b[38;5;5m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}".as_bytes());
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        assert!(glyphs.0.is_empty());
        assert_eq!(batch.images.len(), 2);
        // The 2×2 image fits a 20×20 box in the 20×20 cells, centered.
        let ((x, y), _, _) = quad_at(&batch, batch.images[1].0 / 6);
        assert_eq!((x, y), (10.0, 0.0));
        let v = &batch.vertices[batch.images[1].0 * VERTEX_FLOATS..];
        assert_eq!((v[2], v[3]), (0.5, 0.0));
    }

    #[test]
    fn styles_and_decorations() {
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b[?25l\x1b[1;3;4mx");
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        assert_eq!(glyphs.0, [('x', Style::BoldItalic)]);
        assert_eq!(glyphs.1, [(Underline::Single, 1)]);
        // Glyph and underline, the underline in the text color.
        assert_eq!(batch.quads(), 2);
        assert_eq!(quad_at(&batch, 1).2, 1.0);
        assert_eq!(quad_at(&batch, 1).1, quad_at(&batch, 0).1);
    }

    #[test]
    fn curly_underline_with_color() {
        let mut term = Terminal::new(6, 1, 0);
        // Neovim's diagnostic style, on a narrow and a wide character.
        term.feed("\x1b[?25l\x1b[4:3;58:2::255:0:0ma中".as_bytes());
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        assert_eq!(glyphs.1, [(Underline::Curly, 1), (Underline::Curly, 2)]);
        // Each cell: glyph, then its underline in red.
        assert_eq!(quad_at(&batch, 1).1, Rgb(255, 0, 0).to_f32());
        assert_eq!(quad_at(&batch, 3).1, Rgb(255, 0, 0).to_f32());

        // An indexed underline color resolves through the palette.
        let mut term = Terminal::new(6, 1, 0);
        term.feed(b"\x1b[?25l\x1b[4;58;5;4ma");
        let batch = build(
            &frame(term.screen()),
            &theme(),
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(quad_at(&batch, 1).1, theme().palette[4].to_f32());
    }

    #[test]
    fn cell_backgrounds_follow_transparency_setting() {
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b[41ma\x1b[0m");
        let mut theme = theme();
        theme.opacity = 0.5;
        let batch = build(
            &frame(term.screen()),
            &theme,
            METRICS,
            &mut FakeGlyphs::default(),
        );
        // Red cell background, then the block cursor: both opaque by default.
        assert_eq!(alpha_at(&batch, 0), 1.0);

        theme.transparent_cells = true;
        let batch = build(
            &frame(term.screen()),
            &theme,
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(quad_at(&batch, 0).1, theme.palette[1].to_f32());
        assert_eq!(alpha_at(&batch, 0), 0.5);
        // The cursor (second background) stays opaque.
        assert_eq!(quad_at(&batch, 1).1, theme.cursor.to_f32());
        assert_eq!(alpha_at(&batch, 1), 1.0);
    }

    #[test]
    fn explicit_background_matching_default_stays_opaque() {
        let mut theme = theme();
        theme.opacity = 0.5;
        let Rgb(r, g, b) = theme.background;
        // A theme painting the configured background color explicitly.
        let mut term = Terminal::new(4, 1, 0);
        term.feed(format!("\x1b[?25l\x1b[48;2;{r};{g};{b}m ").as_bytes());
        let batch = build(
            &frame(term.screen()),
            &theme,
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(batch.quads(), 1);
        assert_eq!(quad_at(&batch, 0).1, theme.background.to_f32());
        assert_eq!(alpha_at(&batch, 0), 1.0);

        theme.transparent_cells = true;
        let batch = build(
            &frame(term.screen()),
            &theme,
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(alpha_at(&batch, 0), 0.5);

        // Cells left at the default background draw nothing; the clear
        // color provides it.
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b[?25l ");
        let batch = build(
            &frame(term.screen()),
            &theme,
            METRICS,
            &mut FakeGlyphs::default(),
        );
        assert_eq!(batch.quads(), 0);
    }

    #[test]
    fn hollow_cursor_when_unfocused() {
        let term = Terminal::new(4, 1, 0);
        let mut f = frame(term.screen());
        f.focused = false;
        let batch = build(&f, &theme(), METRICS, &mut FakeGlyphs::default());
        assert_eq!(batch.quads(), 4);
    }

    #[test]
    fn origin_offsets_everything() {
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b[?25lx");
        let mut f = frame(term.screen());
        f.origin = (5, 7);
        let batch = build(&f, &theme(), METRICS, &mut FakeGlyphs::default());
        assert_eq!(quad_at(&batch, 0).0, (5.0, 7.0));
    }

    #[test]
    fn selection_is_highlighted() {
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b[?25lab");
        let mut selection = Selection::new(Point { line: 0, col: 0 });
        selection.update(Point { line: 0, col: 1 });
        let mut f = frame(term.screen());
        f.selection = Some(&selection);
        let batch = build(&f, &theme(), METRICS, &mut FakeGlyphs::default());
        // Two highlighted backgrounds come before the glyphs.
        assert_eq!(batch.quads(), 4);
        assert_eq!(quad_at(&batch, 0).1, theme().foreground.to_f32());
        assert_eq!(quad_at(&batch, 2).1, theme().background.to_f32());
    }

    #[test]
    fn preedit_replaces_cursor() {
        let mut term = Terminal::new(10, 1, 0);
        term.feed(b"$ ");
        let preedit = Preedit {
            text: "日本".into(),
            cursor: Some((3, 3)),
        };
        let mut f = frame(term.screen());
        f.preedit = Some(&preedit);
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&f, &theme(), METRICS, &mut glyphs);
        assert_eq!(
            glyphs
                .0
                .iter()
                .filter(|(c, _)| *c == '日' || *c == '本')
                .count(),
            2
        );
        // Per character: background, glyph, underline; plus the beam.
        let beam = quad_at(&batch, batch.quads() - 1);
        assert_eq!(beam.0, (40.0, 0.0));
    }

    #[test]
    fn preedit_keeps_marks_sent_apart_from_their_letter() {
        let term = Terminal::new(10, 1, 0);
        let preedit = Preedit {
            text: "e\u{301}a".into(),
            cursor: Some((3, 3)),
        };
        let mut f = frame(term.screen());
        f.preedit = Some(&preedit);
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&f, &theme(), METRICS, &mut glyphs);
        let clusters = glyphs.3.clone();
        assert_eq!(clusters, ["e\u{301}"]);
        // The mark takes no cell: the cursor is after the one letter.
        let beam = quad_at(&batch, batch.quads() - 1);
        assert_eq!(beam.0.0, METRICS.width as f32);
    }

    #[test]
    fn preedit_is_clipped_at_line_end() {
        let mut term = Terminal::new(3, 1, 0);
        term.feed(b"ab");
        let preedit = Preedit {
            text: "xyz".into(),
            cursor: None,
        };
        let mut f = frame(term.screen());
        f.preedit = Some(&preedit);
        let mut glyphs = FakeGlyphs::default();
        build(&f, &theme(), METRICS, &mut glyphs);
        assert_eq!(glyphs.0.last(), Some(&('x', Style::Regular)));
    }
}
