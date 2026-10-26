//! Rendering: turns the screen into a list of textured quads.
//!
//! [`build`] is GPU-independent: it produces a [`Batch`] of vertices for
//! cell backgrounds, glyphs, decorations, the cursor and the input-method
//! preedit. [`gl`] uploads and draws it with OpenGL ES 3 on an EGL surface.

pub mod atlas;
pub mod gl;

use unicode_width::UnicodeWidthChar;

use crate::config::{Config, Rgb};
use crate::font::{CellMetrics, Style};
use crate::ime::Preedit;
use crate::selection::{Point, Selection};
use crate::terminal::{Cell, Color, CursorShape, Flags, Screen};

pub use gl::Renderer;

#[derive(Debug, Clone)]
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection_foreground: Option<Rgb>,
    pub selection_background: Option<Rgb>,
    pub palette: [Rgb; 16],
    pub bold_is_bright: bool,
    /// Opacity of the default background.
    pub opacity: f32,
    /// Also apply `opacity` to cells with an explicit background color.
    pub transparent_cells: bool,
}

impl Theme {
    pub fn from_config(config: &Config) -> Theme {
        Theme {
            foreground: config.colors.foreground,
            background: config.colors.background,
            cursor: config.colors.cursor(),
            selection_foreground: config.colors.selection_foreground,
            selection_background: config.colors.selection_background,
            palette: config.colors.palette(),
            bold_is_bright: config.terminal.bold_is_bright,
            opacity: config.window.opacity,
            transparent_cells: config.window.transparent_cell_backgrounds,
        }
    }

    /// Resolves a 256-color index.
    pub fn indexed(&self, i: u8) -> Rgb {
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

    /// Final foreground and background of a cell.
    pub fn cell_colors(&self, cell: &Cell, selected: bool, block_cursor: bool) -> (Rgb, Rgb) {
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
        if block_cursor {
            fg = bg;
            bg = self.cursor;
            if fg == bg {
                fg = self.background;
            }
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
}

pub trait GlyphCache {
    /// The glyph for `c` in a one-cell slot, or a two-cell slot when `wide`.
    fn glyph(&mut self, c: char, style: Style, wide: bool) -> Option<GlyphInfo>;
}

/// Floats per vertex: position (2), texture coordinate (2), color (4), mode (1).
pub const VERTEX_FLOATS: usize = 9;

#[derive(Debug, Default)]
pub struct Batch {
    pub vertices: Vec<f32>,
}

impl Batch {
    /// Adds a quad covering `[x, y, width, height]`; `mode` 0 is a solid
    /// color, 1 samples the glyph atlas at `uv`.
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

    pub fn glyph(&mut self, x: f32, y: f32, glyph: &GlyphInfo, color: Rgb) {
        let (w, h) = (glyph.width as f32, glyph.height as f32);
        self.quad(
            [x + glyph.left as f32, y + glyph.top as f32, w, h],
            glyph.uv,
            color,
            1.0,
            1.0,
        );
    }

    #[cfg(test)]
    pub fn quads(&self) -> usize {
        self.vertices.len() / (6 * VERTEX_FLOATS)
    }

    fn append(&mut self, mut other: Batch) {
        self.vertices.append(&mut other.vertices);
    }
}

/// Everything besides the screen that affects what is drawn.
pub struct Frame<'a> {
    pub screen: &'a Screen,
    pub selection: Option<&'a Selection>,
    pub preedit: Option<&'a Preedit>,
    pub focused: bool,
    /// Grid origin in physical pixels.
    pub origin: (u32, u32),
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

    let cursor_visible = screen.cursor_visible() && frame.preedit.is_none();
    let (cursor_row, cursor_col) = screen.cursor();
    let block_cursor = cursor_visible && frame.focused && screen.cursor_shape == CursorShape::Block;
    let offset = screen.display_offset() as isize;

    for y in 0..screen.rows() {
        let row = screen.visible_row(y);
        let line = y as isize - offset;
        let py = oy + y as f32 * ch;
        for x in 0..screen.cols() {
            let cell = row.cells.get(x).copied().unwrap_or_default();
            if cell.flags.contains(Flags::WIDE_SPACER) {
                continue;
            }
            let span = if cell.flags.contains(Flags::WIDE) {
                2.0
            } else {
                1.0
            };
            let selected = frame
                .selection
                .is_some_and(|s| s.contains(Point { line, col: x }));
            let is_cursor = block_cursor && y == cursor_row && x == cursor_col;
            let (fg, bg) = theme.cell_colors(&cell, selected, is_cursor);
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
            if cell.c != ' '
                && !cell.flags.contains(Flags::HIDDEN)
                && let Some(glyph) = glyphs.glyph(cell.c, style_of(&cell), span > 1.0)
            {
                front.glyph(px, py, &glyph, fg);
            }
            if cell.flags.contains(Flags::UNDERLINE) {
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
            (true, CursorShape::Beam) => front.rect(px, py, beam, ch, theme.cursor),
            (true, CursorShape::Underline) => {
                front.rect(px, py + ch - beam, cw, beam, theme.cursor)
            }
            _ => {
                // Hollow block for an unfocused window.
                front.rect(px, py, cw, stroke, theme.cursor);
                front.rect(px, py + ch - stroke, cw, stroke, theme.cursor);
                front.rect(px, py, stroke, ch, theme.cursor);
                front.rect(px + cw - stroke, py, stroke, ch, theme.cursor);
            }
        }
    }

    if let Some(preedit) = frame.preedit {
        draw_preedit(&mut front, preedit, frame, theme, m, glyphs);
    }

    back.append(front);
    back
}

/// Draws the preedit at the cursor, over the grid, clipped at the line end.
fn draw_preedit(
    batch: &mut Batch,
    preedit: &Preedit,
    frame: &Frame,
    theme: &Theme,
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
    for (i, c) in preedit.text.char_indices() {
        if preedit.cursor.is_some_and(|(begin, _)| begin == i) {
            cursor_col = Some(col);
        }
        let width = c.width().unwrap_or(0);
        if width == 0 {
            continue;
        }
        if col + width > screen.cols() {
            break;
        }
        let w = cw * width as f32;
        batch.fill(col_x(col), py, w, ch, theme.background, theme.opacity);
        if let Some(glyph) = glyphs.glyph(c, Style::Regular, width == 2) {
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
        batch.rect(col_x(col), py, beam, ch, theme.cursor);
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

    /// Records requested glyphs; every glyph is a 1×1 bitmap at the cell origin.
    #[derive(Default)]
    struct FakeGlyphs(Vec<(char, Style)>);

    impl GlyphCache for FakeGlyphs {
        fn glyph(&mut self, c: char, style: Style, _wide: bool) -> Option<GlyphInfo> {
            self.0.push((c, style));
            Some(GlyphInfo {
                uv: [0.0; 4],
                width: 1,
                height: 1,
                left: 0,
                top: 0,
            })
        }
    }

    fn theme() -> Theme {
        Theme::from_config(&Config::default())
    }

    fn frame(screen: &Screen) -> Frame<'_> {
        Frame {
            screen,
            selection: None,
            preedit: None,
            focused: true,
            origin: (0, 0),
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
            t.cell_colors(&cell, false, false),
            (t.palette[1], t.background)
        );
        cell.flags.insert(Flags::BOLD);
        assert_eq!(t.cell_colors(&cell, false, false).0, t.palette[9]);
        cell.flags.insert(Flags::INVERSE);
        assert_eq!(
            t.cell_colors(&cell, false, false),
            (t.background, t.palette[9])
        );
        // Selecting inverted text un-inverts it.
        assert_eq!(
            t.cell_colors(&cell, true, false),
            (t.palette[9], t.background)
        );

        let hidden = Cell {
            flags: Flags::HIDDEN,
            ..Cell::default()
        };
        let (fg, bg) = t.cell_colors(&hidden, false, false);
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
            t.cell_colors(&red, true, false),
            (t.background, Rgb(1, 2, 3))
        );
        t.selection_foreground = Some(Rgb(4, 5, 6));
        assert_eq!(
            t.cell_colors(&red, true, false),
            (Rgb(4, 5, 6), Rgb(1, 2, 3))
        );
        // Inverse cells use the configured colors too.
        let inverse = Cell {
            flags: Flags::INVERSE,
            ..red
        };
        assert_eq!(
            t.cell_colors(&inverse, true, false),
            (Rgb(4, 5, 6), Rgb(1, 2, 3))
        );
        // Unselected cells are unaffected.
        assert_eq!(
            t.cell_colors(&red, false, false),
            (t.palette[1], t.background)
        );
    }

    #[test]
    fn block_cursor_colors() {
        let t = theme();
        let (fg, bg) = t.cell_colors(&Cell::default(), false, true);
        assert_eq!(bg, t.cursor);
        assert_eq!(fg, t.background);
    }

    #[test]
    fn dim_blends_toward_background() {
        let t = theme();
        let cell = Cell {
            flags: Flags::DIM,
            ..Cell::default()
        };
        let fg = t.cell_colors(&cell, false, false).0;
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
    fn styles_and_decorations() {
        let mut term = Terminal::new(4, 1, 0);
        term.feed(b"\x1b[?25l\x1b[1;3;4mx");
        let mut glyphs = FakeGlyphs::default();
        let batch = build(&frame(term.screen()), &theme(), METRICS, &mut glyphs);
        assert_eq!(glyphs.0, [('x', Style::BoldItalic)]);
        // Glyph and underline.
        assert_eq!(batch.quads(), 2);
        let ((_, y), _, _) = quad_at(&batch, 1);
        assert_eq!(y, METRICS.underline as f32);
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
