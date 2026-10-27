//! Font discovery (fontconfig) and glyph rasterization (ab_glyph).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fmt;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;
use unicode_width::UnicodeWidthChar;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, VariableFont, point};
use fontconfig_sys as fc;

use crate::emoji::{self, EmojiFont};
use crate::terminal::Underline;
use fontconfig_sys::constants::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Style {
    pub fn new(bold: bool, italic: bool) -> Style {
        match (bold, italic) {
            (false, false) => Style::Regular,
            (true, false) => Style::Bold,
            (false, true) => Style::Italic,
            (true, true) => Style::BoldItalic,
        }
    }

    fn fc_weight_slant(self) -> (i32, i32) {
        let weight = match self {
            Style::Bold | Style::BoldItalic => FC_WEIGHT_BOLD,
            _ => FC_WEIGHT_REGULAR,
        };
        let slant = match self {
            Style::Italic | Style::BoldItalic => FC_SLANT_ITALIC,
            _ => FC_SLANT_ROMAN,
        };
        (weight, slant)
    }
}

/// Cell geometry in pixels, derived from the primary font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    /// Distance from the cell top to the baseline.
    pub baseline: u32,
    /// Distance from the cell top to the underline.
    pub underline: u32,
    /// Thickness of underlines, strikethrough and beam cursors.
    pub stroke: u32,
}

impl CellMetrics {
    fn new(advance: f32, ascent: f32, descent: f32, line_gap: f32) -> CellMetrics {
        let width = advance.round().max(1.0) as u32;
        let height = (ascent - descent + line_gap).ceil().max(1.0) as u32;
        // Split the line gap evenly above and below the glyphs.
        let baseline = ((line_gap / 2.0) + ascent)
            .round()
            .clamp(0.0, height as f32 - 1.0) as u32;
        let stroke = (height as f32 / 16.0).round().max(1.0) as u32;
        let underline = (baseline + stroke).min(height - stroke);
        CellMetrics {
            width,
            height,
            baseline,
            underline,
            stroke,
        }
    }
}

/// A rasterized glyph: an 8-bit coverage bitmap, or premultiplied RGBA
/// pixels for a color emoji, and its placement.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub width: u32,
    pub height: u32,
    /// Offset of the bitmap's left edge from the cell's left edge.
    pub left: i32,
    /// Offset of the bitmap's top edge from the cell's top edge.
    pub top: i32,
    pub data: Vec<u8>,
    /// `data` is RGBA (4 bytes per pixel) rather than coverage.
    pub color: bool,
}

#[derive(Debug)]
pub enum FontError {
    NotFound(String),
    Load(PathBuf, &'static str),
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FontError::NotFound(family) => write!(f, "no font found for {family:?}"),
            FontError::Load(path, e) => write!(f, "{}: {e}", path.display()),
        }
    }
}

impl std::error::Error for FontError {}

/// A font file location as reported by fontconfig.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontFile {
    path: PathBuf,
    index: u32,
}

/// A handle to the fontconfig configuration.
struct Fontconfig(*mut fc::FcConfig);

impl Fontconfig {
    fn new() -> Option<Fontconfig> {
        // SAFETY: returns an owned config or null.
        let config = unsafe { fc::FcInitLoadConfigAndFonts() };
        (!config.is_null()).then_some(Fontconfig(config))
    }

    /// Finds the best font file for `family` in `style`, optionally
    /// requiring it to cover `c`.
    fn find(&self, family: &str, style: Style, c: Option<char>) -> Option<FontFile> {
        let family = CString::new(family).ok()?;
        let (weight, slant) = style.fc_weight_slant();
        // SAFETY: standard fontconfig pattern lifecycle; every object created
        // here is destroyed before returning, and strings read from the match
        // are copied before it is destroyed.
        unsafe {
            let pattern = fc::FcPatternCreate();
            if pattern.is_null() {
                return None;
            }
            fc::FcPatternAddString(pattern, FC_FAMILY.as_ptr(), family.as_ptr().cast());
            fc::FcPatternAddInteger(pattern, FC_WEIGHT.as_ptr(), weight);
            fc::FcPatternAddInteger(pattern, FC_SLANT.as_ptr(), slant);
            if let Some(c) = c {
                let charset = fc::FcCharSetCreate();
                fc::FcCharSetAddChar(charset, c as u32);
                fc::FcPatternAddCharSet(pattern, FC_CHARSET.as_ptr(), charset);
                fc::FcCharSetDestroy(charset);
                // Color (bitmap) fonts cannot be rendered by ab_glyph.
                fc::FcPatternAddBool(pattern, FC_COLOR.as_ptr(), 0);
            }
            fc::FcConfigSubstitute(self.0, pattern, fc::FcMatchPattern);
            fc::FcDefaultSubstitute(pattern);

            let mut result = fc::FcResultNoMatch;
            let matched = fc::FcFontMatch(self.0, pattern, &mut result);
            fc::FcPatternDestroy(pattern);
            if matched.is_null() {
                return None;
            }
            let mut file: *mut fc::FcChar8 = std::ptr::null_mut();
            let mut index = 0;
            let found = fc::FcPatternGetString(matched, FC_FILE.as_ptr(), 0, &mut file)
                == fc::FcResultMatch;
            fc::FcPatternGetInteger(matched, FC_INDEX.as_ptr(), 0, &mut index);
            let font = found.then(|| FontFile {
                path: PathBuf::from(CStr::from_ptr(file.cast()).to_string_lossy().into_owned()),
                index: index.max(0) as u32,
            });
            fc::FcPatternDestroy(matched);
            font
        }
    }
}

impl Fontconfig {
    /// The preferred color emoji font, if one is installed.
    fn find_color_emoji(&self) -> Option<FontFile> {
        // SAFETY: as in `find`.
        unsafe {
            let pattern = fc::FcPatternCreate();
            if pattern.is_null() {
                return None;
            }
            fc::FcPatternAddString(pattern, FC_FAMILY.as_ptr(), c"emoji".as_ptr().cast());
            fc::FcPatternAddBool(pattern, FC_COLOR.as_ptr(), 1);
            fc::FcConfigSubstitute(self.0, pattern, fc::FcMatchPattern);
            fc::FcDefaultSubstitute(pattern);
            let mut result = fc::FcResultNoMatch;
            let matched = fc::FcFontMatch(self.0, pattern, &mut result);
            fc::FcPatternDestroy(pattern);
            if matched.is_null() {
                return None;
            }
            // The match is the closest font, which may not be a color one.
            let mut color = 0;
            let mut file: *mut fc::FcChar8 = std::ptr::null_mut();
            let mut index = 0;
            let found = fc::FcPatternGetBool(matched, FC_COLOR.as_ptr(), 0, &mut color)
                == fc::FcResultMatch
                && color != 0
                && fc::FcPatternGetString(matched, FC_FILE.as_ptr(), 0, &mut file)
                    == fc::FcResultMatch;
            fc::FcPatternGetInteger(matched, FC_INDEX.as_ptr(), 0, &mut index);
            let font = found.then(|| FontFile {
                path: PathBuf::from(CStr::from_ptr(file.cast()).to_string_lossy().into_owned()),
                index: index.max(0) as u32,
            });
            fc::FcPatternDestroy(matched);
            font
        }
    }
}

/// Finds and loads the system's color emoji font.
pub fn load_emoji_font() -> Option<EmojiFont> {
    let file = Fontconfig::new()?.find_color_emoji()?;
    let font = EmojiFont::new(map_file(&file.path).ok()?, file.index & 0xffff);
    match &font {
        Some(_) => log::info!("loading emoji font {}", file.path.display()),
        None => log::info!(
            "{}: no color bitmaps, emoji stay monochrome",
            file.path.display()
        ),
    }
    font
}

impl Drop for Fontconfig {
    fn drop(&mut self) {
        // SAFETY: we own the config.
        unsafe { fc::FcConfigDestroy(self.0) }
    }
}

pub struct Fonts {
    fontconfig: Fontconfig,
    faces: Vec<FontRef<'static>>,
    /// Font files mapped so far, shared by the faces and styles in them.
    files: HashMap<PathBuf, &'static [u8]>,
    /// Loaded faces by file and style (variable fonts differ per style).
    loaded: HashMap<(FontFile, Style), usize>,
    /// Face index for each style.
    styles: [usize; 4],
    /// Families searched, in order, for characters the primary faces lack:
    /// the configured fallbacks, then the primary family, so fontconfig
    /// picks fonts like it (DejaVu Sans Mono rather than DejaVu Sans for
    /// a monospace font), then any font.
    fallback_families: Vec<String>,
    /// Face covering a character none of the primary faces have.
    fallback: HashMap<char, Option<usize>>,
    /// The color emoji font, loaded on first use (`None` inside: there is
    /// none).
    emoji: Option<Option<EmojiFont>>,
    px: f32,
    pub metrics: CellMetrics,
}

impl Fonts {
    /// Loads `family` in all four styles at `px` pixels per em.
    pub fn new(family: &str, fallback: &[String], px: f32) -> Result<Fonts, FontError> {
        let fontconfig = Fontconfig::new().ok_or_else(|| FontError::NotFound(family.into()))?;
        let mut fonts = Fonts {
            fontconfig,
            faces: Vec::new(),
            files: HashMap::new(),
            loaded: HashMap::new(),
            styles: [0; 4],
            fallback_families: fallback.iter().cloned().chain([family.into()]).collect(),
            fallback: HashMap::new(),
            emoji: None,
            px,
            metrics: CellMetrics::new(1.0, 1.0, 0.0, 0.0),
        };
        let regular = fonts.find_and_load(family, Style::Regular, None)?;
        fonts.styles = [regular; 4];
        for (slot, style) in [Style::Bold, Style::Italic, Style::BoldItalic]
            .into_iter()
            .enumerate()
        {
            match fonts.find_and_load(family, style, None) {
                Ok(face) => fonts.styles[slot + 1] = face,
                Err(e) => log::warn!("{e}; using the regular face for {style:?}"),
            }
        }
        fonts.set_size(px);
        Ok(fonts)
    }

    fn find_and_load(
        &mut self,
        family: &str,
        style: Style,
        c: Option<char>,
    ) -> Result<usize, FontError> {
        let file = self
            .fontconfig
            .find(family, style, c)
            .ok_or_else(|| FontError::NotFound(family.into()))?;
        self.load(file, style)
    }

    fn load(&mut self, file: FontFile, style: Style) -> Result<usize, FontError> {
        if let Some(&face) = self.loaded.get(&(file.clone(), style)) {
            return Ok(face);
        }
        log::info!(
            "loading font {} (index {})",
            file.path.display(),
            file.index
        );
        let data = match self.files.get(&file.path) {
            Some(&data) => data,
            None => {
                let data = map_file(&file.path)
                    .map_err(|_| FontError::Load(file.path.clone(), "cannot read file"))?;
                self.files.insert(file.path.clone(), data);
                data
            }
        };
        // fontconfig puts the named instance of a variable font in the high
        // 16 bits; only the face index matters here (the default instance
        // is used).
        let mut face = FontRef::try_from_slice_and_index(data, file.index & 0xffff)
            .map_err(|_| FontError::Load(file.path.clone(), "invalid font"))?;
        // Variable fonts default to an arbitrary instance (often the lightest).
        let weight = if matches!(style, Style::Bold | Style::BoldItalic) {
            700.0
        } else {
            400.0
        };
        face.set_variation(b"wght", weight);
        self.faces.push(face);
        let index = self.faces.len() - 1;
        self.loaded.insert((file, style), index);
        Ok(index)
    }

    pub fn set_size(&mut self, px: f32) {
        self.px = px;
        let face = &self.faces[self.styles[0]];
        let scaled = face.as_scaled(px_scale(face, px));
        let advance = ['M', '0', 'x']
            .into_iter()
            .find(|&c| has_glyph(face, c))
            .map(|c| scaled.h_advance(face.glyph_id(c)))
            .unwrap_or(px * 0.6);
        self.metrics = CellMetrics::new(
            advance,
            scaled.ascent(),
            scaled.descent(),
            scaled.line_gap(),
        );
    }

    /// The face used to draw `c` in `style`.
    fn face_for(&mut self, c: char, style: Style) -> Option<usize> {
        let styled = self.styles[style as usize];
        if has_glyph(&self.faces[styled], c) {
            return Some(styled);
        }
        let regular = self.styles[0];
        if has_glyph(&self.faces[regular], c) {
            return Some(regular);
        }
        if let Some(&face) = self.fallback.get(&c) {
            return face;
        }
        let families: Vec<String> = self
            .fallback_families
            .iter()
            .cloned()
            .chain([String::new()])
            .collect();
        let face = families.iter().find_map(|family| {
            let face = self.find_and_load(family, Style::Regular, Some(c)).ok()?;
            has_glyph(&self.faces[face], c).then_some(face)
        });
        if face.is_none() {
            log::debug!("no font covers {c:?} (U+{:04X})", c as u32);
        }
        self.fallback.insert(c, face);
        face
    }

    /// Draws `text` (a character or an emoji sequence) as a color emoji, if
    /// it is shown as one and the emoji font has it.
    fn rasterize_color(&mut self, text: &str, wide: bool) -> Option<Glyph> {
        if !emoji::wants_color(text) {
            return None;
        }
        let font = self.emoji.get_or_insert_with(load_emoji_font).as_ref()?;
        let glyph = font.glyph_for(text)?;
        let m = self.metrics;
        let slot = m.width * if wide { 2 } else { 1 };
        let (width, height, data) = font.rasterize(glyph, slot, m.height)?;
        Some(Glyph {
            width,
            height,
            left: (slot as i32 - width as i32) / 2,
            top: (m.height as i32 - height as i32) / 2,
            data,
            color: true,
        })
    }

    /// Rasterizes `c` for a one-cell slot, or a two-cell slot when `wide`.
    /// Returns `None` for characters with no visible pixels.
    pub fn rasterize(&mut self, c: char, style: Style, wide: bool) -> Option<Glyph> {
        if c == ' ' {
            return None;
        }
        if let Some(glyph) = self.rasterize_color(c.encode_utf8(&mut [0; 4]), wide) {
            return Some(glyph);
        }
        if let Some(glyph) = box_glyph(c, self.metrics) {
            return Some(glyph);
        }
        let index = self.face_for(c, style).unwrap_or(self.styles[0]);
        let face = &self.faces[index];
        let m = self.metrics;
        let id = face.glyph_id(c);
        let scale = px_scale(face, self.px);
        // Place the baseline where the cell's is, so bounds are cell-relative.
        let origin = point(0.0, m.baseline as f32);
        let mut outline = face.outline_glyph(id.with_scale_and_position(scale, origin))?;

        // A narrow glyph in a two-cell slot, as for emoji presentation
        // sequences (`❄` + VS16): enlarge it to fill the slot like an emoji
        // would, and center it.
        let slot_width = 2.0 * m.width as f32;
        let fill_slot = wide && c.width() == Some(1);
        if fill_slot {
            let b = outline.px_bounds();
            let factor = (slot_width * 0.9 / b.width())
                .min(m.height as f32 * 0.9 / b.height())
                .clamp(1.0, 2.0);
            let scale = PxScale::from(scale.y * factor);
            outline = face.outline_glyph(id.with_scale_and_position(scale, origin))?;
        }

        // Symbols can stick out of the cell (☺ in DejaVu Sans Mono, ♥ in
        // JetBrains Mono) and would overlap their neighbors: shrink them to
        // fit, centered across the cell and on the baseline. Letters keep
        // their shape, italic overhangs included, and so do Nerd Font
        // glyphs: icons are sized by their font, and powerline separators
        // must reach the cell edges.
        let b = outline.px_bounds();
        let fit = !wide
            && !is_private_use(c)
            && !c.is_alphanumeric()
            && (b.min.x < 0.0 || b.max.x > m.width as f32);
        if fit {
            let (w, h) = (m.width as f32, m.height as f32);
            // Rounding the bounds outward can add a pixel: shrink a pixel
            // more if the first try doesn't fit.
            for width in [w, w - 1.0] {
                let factor = (width.max(1.0) / b.width()).min(h / b.height());
                if factor >= 1.0 {
                    break;
                }
                let scale = PxScale::from(scale.y * factor);
                outline = face.outline_glyph(id.with_scale_and_position(scale, origin))?;
                if outline.px_bounds().width() <= w {
                    break;
                }
            }
        }

        let bounds = outline.px_bounds();
        let (width, height) = (bounds.width() as u32, bounds.height() as u32);
        if width == 0 || height == 0 {
            return None;
        }
        let mut data = vec![0; (width * height) as usize];
        outline.draw(|x, y, coverage| {
            if let Some(px) = data.get_mut((y * width + x) as usize) {
                *px = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        // Center the visible ink: bitmaps can have faint antialiasing rows
        // on one side only.
        let ink = ink_rows(&data, width, 64).unwrap_or((0, height - 1));
        let ink_height = (ink.1 - ink.0 + 1) as i32;
        let centered_top = (m.height as i32 - ink_height) / 2 - ink.0 as i32;
        let (left, top) = if fill_slot {
            ((slot_width as i32 - width as i32) / 2, centered_top)
        } else if fit {
            // Keep it on the baseline unless that leaves it out of the cell.
            let top = (bounds.min.y as i32).clamp(0, (m.height as i32 - height as i32).max(0));
            ((m.width as i32 - width as i32) / 2, top)
        } else if is_icon(c) && ink_height <= m.height as i32 {
            // Icon fonts center glyphs on the font's math axis, which sits
            // above the cell's middle; center them in the cell instead.
            (bounds.min.x as i32, centered_top)
        } else {
            (bounds.min.x as i32, bounds.min.y as i32)
        };
        Some(Glyph {
            width,
            height,
            left,
            top,
            data,
            color: false,
        })
    }
}

impl Fonts {
    /// Rasterizes a grapheme cluster: a character with combining marks,
    /// and possibly characters that join it but take up cells of their own
    /// (Thai ำ after a consonant). When Unicode composes it into one
    /// character (`e` + U+0301 is `é`), that character is drawn; otherwise
    /// the marks are drawn centered over the character they follow, and
    /// spacing characters in their own cells.
    pub fn rasterize_cluster(&mut self, text: &str, style: Style, wide: bool) -> Option<Glyph> {
        if let Some(glyph) = self.rasterize_color(text, wide) {
            return Some(glyph);
        }
        let composed: String = text.nfc().collect();
        let mut chars = composed.chars();
        let base = chars.next()?;
        // A narrow character fills a two-cell slot only when nothing else
        // is drawn in it, as for emoji presentation sequences (`❄` + VS16).
        let spacing = chars.clone().any(|c| c.width().unwrap_or(0) > 0);
        let mut glyph = self.rasterize(base, style, wide && !spacing);
        let cell = self.metrics.width as i32;
        // The cell the last character drawn starts in, and its width.
        let (mut col, mut cols) = (0, if wide && !spacing { 2 } else { 1 });
        for c in chars {
            let width = c.width().unwrap_or(0) as i32;
            // Joiners and variation selectors have no ink of their own.
            if width == 0 && is_joiner_or_selector(c) {
                continue;
            }
            let Some(mut part) = self.rasterize(c, style, false) else {
                continue;
            };
            if width > 0 {
                col += cols;
                cols = width;
                part.left += col * cell;
            } else {
                part.left = col * cell + (cols * cell - part.width as i32) / 2;
            }
            glyph = Some(match glyph {
                Some(base) => overlay(&base, &part),
                None => part,
            });
        }
        glyph
    }
}

/// ZWJ, ZWNJ and variation selectors.
fn is_joiner_or_selector(c: char) -> bool {
    matches!(c, '\u{200c}' | '\u{200d}' | '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
}

/// Combines two glyph bitmaps, positioned by their offsets, into one.
fn overlay(a: &Glyph, b: &Glyph) -> Glyph {
    let left = a.left.min(b.left);
    let top = a.top.min(b.top);
    let right = (a.left + a.width as i32).max(b.left + b.width as i32);
    let bottom = (a.top + a.height as i32).max(b.top + b.height as i32);
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    let mut data = vec![0u8; (width * height) as usize];
    for g in [a, b] {
        let (dx, dy) = ((g.left - left) as u32, (g.top - top) as u32);
        for y in 0..g.height {
            for x in 0..g.width {
                let src = g.data[(y * g.width + x) as usize];
                let dst = &mut data[((y + dy) * width + x + dx) as usize];
                *dst = (*dst).max(src);
            }
        }
    }
    Glyph {
        width,
        height,
        left,
        top,
        data,
        color: false,
    }
}

/// Draws an underline of `style` spanning `cells` cells. The bitmap is as
/// wide as the cells and positioned relative to the cell top; patterns
/// repeat per cell so neighboring cells join seamlessly.
pub fn underline_glyph(style: Underline, m: CellMetrics, cells: u32) -> Option<Glyph> {
    let (cw, ch) = (m.width, m.height);
    let width = cw * cells;
    let t = m.stroke.max(1);
    let (top, height) = match style {
        Underline::None => return None,
        Underline::Double => (m.underline.min(ch.saturating_sub(3 * t)), 3 * t),
        Underline::Curly => {
            let band = curly_band(m);
            (
                m.underline.saturating_sub(1).min(ch.saturating_sub(band)),
                band,
            )
        }
        _ => (m.underline.min(ch.saturating_sub(t)), t),
    };
    let mut data = vec![0u8; (width * height) as usize];
    let mut fill = |x0: u32, x1: u32, y0: u32, y1: u32| {
        for y in y0..y1.min(height) {
            for x in x0..x1.min(width) {
                data[(y * width + x) as usize] = 255;
            }
        }
    };
    match style {
        Underline::None => unreachable!(),
        Underline::Single => fill(0, width, 0, t),
        Underline::Double => {
            fill(0, width, 0, t);
            fill(0, width, 2 * t, 3 * t);
        }
        Underline::Dotted => {
            // Square dots, an even number per cell so the rhythm is regular.
            let dot = t.max(2);
            let per_cell = (cw / (2 * dot)).max(1);
            for cell in 0..cells {
                for i in 0..per_cell {
                    let x = cell * cw + (2 * i + 1) * cw / (2 * per_cell) - dot / 2;
                    fill(x, x + dot, 0, t);
                }
            }
        }
        Underline::Dashed => {
            // One dash per cell, with a gap split over both ends.
            for cell in 0..cells {
                let gap = cw / 6;
                fill(cell * cw + gap, (cell + 1) * cw - gap, 0, t);
            }
        }
        Underline::Curly => draw_wave(&mut data, width, height, cw, t),
    }
    Some(Glyph {
        width,
        height,
        left: 0,
        top: top as i32,
        data,
        color: false,
    })
}

/// Amplitude of the curly underline, in pixels.
fn curly_amplitude(m: CellMetrics) -> f32 {
    (m.height as f32 / 14.0).max(1.2)
}

/// Height of the bitmap holding the curly underline.
fn curly_band(m: CellMetrics) -> u32 {
    (2.0 * curly_amplitude(m) + m.stroke.max(1) as f32).ceil() as u32 + 1
}

/// A sine wave with one period per cell, antialiased by the distance of
/// each pixel center to the curve.
fn draw_wave(data: &mut [u8], width: u32, height: u32, period: u32, thickness: u32) {
    let amplitude = (height as f32 - 1.0 - thickness as f32) / 2.0;
    let mid = height as f32 / 2.0;
    let k = std::f32::consts::TAU / period as f32;
    for x in 0..width {
        let px = x as f32 + 0.5;
        let y_curve = mid - amplitude * (k * px).cos();
        let slope = amplitude * k * (k * px).sin();
        let scale = (1.0 + slope * slope).sqrt();
        for y in 0..height {
            let distance = ((y as f32 + 0.5) - y_curve).abs() / scale;
            let coverage = (thickness as f32 / 2.0 + 0.5 - distance).clamp(0.0, 1.0);
            data[(y * width + x) as usize] = (coverage * 255.0).round() as u8;
        }
    }
}

/// Arm weights (up, down, left, right) of U+2500–U+257F, sixteen
/// characters per row: 0 none, 1 light, 2 heavy, 3 double. `----` marks
/// the dashed, rounded and diagonal lines, which are drawn separately.
const BOX_ARMS: [&str; 8] = [
    "0011 0022 1100 2200 ---- ---- ---- ---- ---- ---- ---- ---- 0101 0102 0201 0202",
    "0110 0120 0210 0220 1001 1002 2001 2002 1010 1020 2010 2020 1101 1102 2101 1201",
    "2201 2102 1202 2202 1110 1120 2110 1210 2210 2120 1220 2220 0111 0121 0112 0122",
    "0211 0221 0212 0222 1011 1021 1012 1022 2011 2021 2012 2022 1111 1121 1112 1122",
    "2111 1211 2211 2121 2112 1221 1212 2122 1222 2221 2212 2222 ---- ---- ---- ----",
    "0033 3300 0103 0301 0303 0130 0310 0330 1003 3001 3003 1030 3010 3030 1103 3301",
    "3303 1130 3310 3330 0133 0311 0333 1033 3011 3033 1133 3311 3333 ---- ---- ----",
    "---- ---- ---- ---- 0010 1000 0001 0100 0020 2000 0002 0200 0012 1200 0021 2100",
];

/// Line weights of the four arms (up, down, left, right) of a box-drawing
/// character made of straight lines meeting at the center.
fn box_arms(c: char) -> Option<[u8; 4]> {
    let index = (c as u32).checked_sub(0x2500).filter(|&i| i < 0x80)? as usize;
    let entry = BOX_ARMS[index / 16].split(' ').nth(index % 16)?;
    let mut arms = [0; 4];
    for (arm, digit) in arms.iter_mut().zip(entry.bytes()) {
        *arm = digit.checked_sub(b'0').filter(|&d| d <= 3)?;
    }
    Some(arms)
}

/// Start of a band `thickness` wide centered in `size`.
fn band_start(thickness: u32, size: u32) -> u32 {
    (size - thickness.min(size)) / 2
}

/// A coverage bitmap the size of a cell.
struct Canvas {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            data: vec![0; (width * height) as usize],
        }
    }

    /// Sets the pixels in `x0..x1` × `y0..y1`, clipped to the canvas.
    fn fill(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, value: u8) {
        for y in y0.min(self.height)..y1.min(self.height) {
            for x in x0.min(self.width)..x1.min(self.width) {
                self.data[(y * self.width + x) as usize] = value;
            }
        }
    }

    /// Draws an antialiased line `thickness` wide, given the distance from
    /// a point to its center line.
    fn stroke(&mut self, thickness: f32, distance: impl Fn(f32, f32) -> f32) {
        for y in 0..self.height {
            for x in 0..self.width {
                let d = distance(x as f32 + 0.5, y as f32 + 0.5);
                let coverage = (thickness / 2.0 + 0.5 - d).clamp(0.0, 1.0);
                let px = &mut self.data[(y * self.width + x) as usize];
                *px = (*px).max((coverage * 255.0).round() as u8);
            }
        }
    }
}

impl Canvas {
    /// Fills the region where `inside` holds, given coordinates relative
    /// to the cell (0 to 1 across and down), at `value`. Each pixel is
    /// sampled 4×4 times for antialiased edges.
    fn shape(&mut self, value: u8, inside: impl Fn(f32, f32) -> bool) {
        let (w, h) = (self.width as f32, self.height as f32);
        for y in 0..self.height {
            for x in 0..self.width {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let u = (x as f32 + (sx as f32 + 0.5) / 4.0) / w;
                        let v = (y as f32 + (sy as f32 + 0.5) / 4.0) / h;
                        hits += u32::from(inside(u, v));
                    }
                }
                let px = &mut self.data[(y * self.width + x) as usize];
                *px = (*px).max((hits * u32::from(value) / 16) as u8);
            }
        }
    }
}

/// Draws straight box-drawing lines meeting at the center. A double line
/// is drawn as one wide band whose middle is then cleared.
fn draw_arms(canvas: &mut Canvas, stroke: u32, arms: [u8; 4]) {
    let (w, h) = (canvas.width, canvas.height);
    let thickness = |weight: u8| stroke * u32::from(weight);
    let band = |t: u32, size: u32| (band_start(t, size), band_start(t, size) + t);
    let [up, down, left, right] = arms;
    let (vertical, horizontal) = (thickness(up.max(down)), thickness(left.max(right)));
    // Arms run past the center to the far side of the band crossing them,
    // or of their own band when nothing crosses.
    let (x0, x1) = band(if vertical > 0 { vertical } else { horizontal }, w);
    let (y0, y1) = band(if horizontal > 0 { horizontal } else { vertical }, h);
    if up > 0 {
        let (a, b) = band(thickness(up), w);
        canvas.fill(a, 0, b, y1, 255);
    }
    if down > 0 {
        let (a, b) = band(thickness(down), w);
        canvas.fill(a, y0, b, h, 255);
    }
    if left > 0 {
        let (a, b) = band(thickness(left), h);
        canvas.fill(0, a, x1, b, 255);
    }
    if right > 0 {
        let (a, b) = band(thickness(right), h);
        canvas.fill(x0, a, w, b, 255);
    }

    // The gap between the two lines of a double band.
    let gap = |size: u32| {
        let a = band_start(3 * stroke, size) + stroke;
        (a, a + stroke)
    };
    // Spans along an axis where the gaps of its double arms (`before`,
    // `after`) are cleared, given the arms crossing them.
    let spans = |size: u32, [before, after]: [u8; 2], [p, q]: [u8; 2]| {
        let (lo, hi) = if p == 3 || q == 3 {
            // Double lines crossing: each pair of lines turns a corner.
            let (g0, g1) = gap(size);
            (g1, g0)
        } else if p.max(q) > 0 && !(before == 3 && after == 3 && (p == 0 || q == 0)) {
            // A single line crossing or turning the corner spans the gap.
            band(thickness(p.max(q)), size)
        } else {
            // Nothing crosses, or a single line meets the double line in a
            // tee and stops at its near side.
            (size, 0)
        };
        [
            (before == 3).then_some((0, lo)),
            (after == 3).then_some((hi, size)),
        ]
    };
    let (g0, g1) = gap(h);
    for (a, b) in spans(w, [left, right], [up, down]).into_iter().flatten() {
        canvas.fill(a, g0, b, g1, 0);
    }
    let (g0, g1) = gap(w);
    for (a, b) in spans(h, [up, down], [left, right]).into_iter().flatten() {
        canvas.fill(g0, a, g1, b, 0);
    }
}

/// Draws `count` dashes per cell, each centered in its share of the cell.
fn draw_dashes(canvas: &mut Canvas, thickness: u32, count: u32, vertical: bool) {
    let (length, across) = if vertical {
        (canvas.height, canvas.width)
    } else {
        (canvas.width, canvas.height)
    };
    let a = band_start(thickness, across);
    for i in 0..count {
        let (from, to) = (i * length / count, (i + 1) * length / count);
        let gap = ((to - from) / 3).max(1);
        let (from, to) = (from + gap / 2, to - (gap - gap / 2));
        if vertical {
            canvas.fill(a, from, a + thickness, to, 255);
        } else {
            canvas.fill(from, a, to, a + thickness, 255);
        }
    }
}

/// Draws a rounded corner: a quarter circle joining the middles of the
/// edges toward `dx` (1 right, -1 left) and `dy` (1 down, -1 up), meeting
/// the straight lines of the neighboring cells.
fn draw_arc(canvas: &mut Canvas, stroke: u32, dx: f32, dy: f32) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let s = stroke as f32;
    let mx = band_start(stroke, canvas.width) as f32 + s / 2.0;
    let my = band_start(stroke, canvas.height) as f32 + s / 2.0;
    let r = (if dx > 0.0 { w - mx } else { mx }).min(if dy > 0.0 { h - my } else { my });
    let (cx, cy) = (mx + dx * r, my + dy * r);
    canvas.stroke(s, |x, y| {
        let (vx, vy) = (x - cx, y - cy);
        let arc = if vx * dx <= 0.0 && vy * dy <= 0.0 {
            (vx.hypot(vy) - r).abs()
        } else {
            f32::INFINITY
        };
        // Straight runs from the arc's ends to the edges.
        let horizontal = if vx * dx >= 0.0 {
            (y - my).abs()
        } else {
            vx.hypot(y - my)
        };
        let vertical = if vy * dy >= 0.0 {
            (x - mx).abs()
        } else {
            (x - mx).hypot(vy)
        };
        arc.min(horizontal).min(vertical)
    });
}

/// Draws box-drawing and block characters to fill the cell exactly, so
/// lines join across cells regardless of the font's metrics.
fn box_glyph(c: char, m: CellMetrics) -> Option<Glyph> {
    let (w, h) = (m.width, m.height);
    let s = m.stroke;
    let mut canvas = Canvas::new(w, h);
    let eighth_h = |n: u32| (h * n + 4) / 8;
    let eighth_w = |n: u32| (w * n + 4) / 8;
    if let Some(arms) = box_arms(c) {
        draw_arms(&mut canvas, s, arms);
    } else {
        match c {
            // Triple and quadruple dashes: light and heavy, horizontal and
            // vertical.
            '┄'..='┋' => {
                let i = c as u32 - '┄' as u32;
                draw_dashes(&mut canvas, s * (1 + i % 2), 3 + i / 4, i / 2 % 2 == 1);
            }
            '╌'..='╏' => {
                let i = c as u32 - '╌' as u32;
                draw_dashes(&mut canvas, s * (1 + i % 2), 2, i / 2 == 1);
            }
            '╭' => draw_arc(&mut canvas, s, 1.0, 1.0),
            '╮' => draw_arc(&mut canvas, s, -1.0, 1.0),
            '╯' => draw_arc(&mut canvas, s, -1.0, -1.0),
            '╰' => draw_arc(&mut canvas, s, 1.0, -1.0),
            // Diagonals from corner to corner, so they join across cells.
            '╱'..='╳' => {
                let (wf, hf) = (w as f32, h as f32);
                let length = wf.hypot(hf);
                if c != '╲' {
                    canvas.stroke(s as f32, |x, y| (hf * x + wf * y - wf * hf).abs() / length);
                }
                if c != '╱' {
                    canvas.stroke(s as f32, |x, y| (hf * x - wf * y).abs() / length);
                }
            }
            '█' => canvas.fill(0, 0, w, h, 255),
            '▀' => canvas.fill(0, 0, w, eighth_h(4), 255),
            '▔' => canvas.fill(0, 0, w, eighth_h(1), 255),
            '▕' => canvas.fill(w - eighth_w(1), 0, w, h, 255),
            '▐' => canvas.fill(w - eighth_w(4), 0, w, h, 255),
            '░' => canvas.fill(0, 0, w, h, 64),
            '▒' => canvas.fill(0, 0, w, h, 128),
            '▓' => canvas.fill(0, 0, w, h, 192),
            // Lower one eighth to seven eighths (including the lower half).
            '▁'..='▇' => {
                let n = c as u32 - '▁' as u32 + 1;
                canvas.fill(0, h - eighth_h(n), w, h, 255);
            }
            // Left seven eighths down to one eighth (including the left half).
            '▉'..='▏' => {
                let n = 7 - (c as u32 - '▉' as u32);
                canvas.fill(0, 0, eighth_w(n), h, 255);
            }
            // Quadrants, as bits: upper left 1, upper right 2, lower left
            // 4, lower right 8.
            '▖'..='▟' => {
                let bits = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14][(c as u32 - '▖' as u32) as usize];
                let (mx, my) = (eighth_w(4), eighth_h(4));
                for (bit, x0, y0, x1, y1) in [
                    (1, 0, 0, mx, my),
                    (2, mx, 0, w, my),
                    (4, 0, my, mx, h),
                    (8, mx, my, w, h),
                ] {
                    if bits & bit != 0 {
                        canvas.fill(x0, y0, x1, y1, 255);
                    }
                }
            }
            // Braille: square dots on a 2×4 grid spaced evenly across
            // cells, so braille graphs line up like pixels.
            '\u{2801}'..='\u{28FF}' => {
                let bits = c as u32 - 0x2800;
                let size = (w / 4).min(h / 8).max(1);
                let start = |i: u32, cells: u32, length: u32| {
                    ((2 * i + 1) * length / (2 * cells)).saturating_sub(size / 2)
                };
                // Dots 1–3 and 7 down the left column, 4–6 and 8 down the
                // right, as bits 0–7.
                for (bit, column, row) in [
                    (0, 0, 0),
                    (1, 0, 1),
                    (2, 0, 2),
                    (3, 1, 0),
                    (4, 1, 1),
                    (5, 1, 2),
                    (6, 0, 3),
                    (7, 1, 3),
                ] {
                    if bits & (1 << bit) != 0 {
                        let (x, y) = (start(column, 2, w), start(row, 4, h));
                        canvas.fill(x, y, x + size, y + size, 255);
                    }
                }
            }
            _ if draw_legacy(&mut canvas, c, s) => {}
            _ => return None,
        }
    }
    Some(Glyph {
        width: w,
        height: h,
        left: 0,
        top: 0,
        data: canvas.data,
        color: false,
    })
}

/// Smooth mosaics (U+1FB3C–U+1FB67): the part of the cell on one side of
/// a line, as the line's two ends and the corner on the filled side, in
/// (x, y) pairs with x in halves and y in thirds of the cell.
const MOSAICS: [[u8; 6]; 44] = [
    [0, 2, 1, 3, 0, 3],
    [0, 2, 2, 3, 0, 3],
    [0, 1, 1, 3, 0, 3],
    [0, 1, 2, 3, 0, 3],
    [0, 0, 1, 3, 0, 3],
    [0, 1, 1, 0, 2, 3],
    [0, 1, 2, 0, 2, 3],
    [0, 2, 1, 0, 2, 3],
    [0, 2, 2, 0, 2, 3],
    [0, 3, 1, 0, 2, 3],
    [0, 2, 2, 1, 2, 3],
    [1, 3, 2, 2, 2, 3],
    [0, 3, 2, 2, 2, 3],
    [1, 3, 2, 1, 2, 3],
    [0, 3, 2, 1, 2, 3],
    [1, 3, 2, 0, 2, 3],
    [1, 0, 2, 1, 0, 3],
    [0, 0, 2, 1, 0, 3],
    [1, 0, 2, 2, 0, 3],
    [0, 0, 2, 2, 0, 3],
    [1, 0, 2, 3, 0, 3],
    [0, 1, 2, 2, 0, 3],
    [0, 2, 1, 3, 2, 0],
    [0, 2, 2, 3, 2, 0],
    [0, 1, 1, 3, 2, 0],
    [0, 1, 2, 3, 2, 0],
    [0, 0, 1, 3, 2, 0],
    [0, 1, 1, 0, 0, 0],
    [0, 1, 2, 0, 0, 0],
    [0, 2, 1, 0, 0, 0],
    [0, 2, 2, 0, 0, 0],
    [0, 3, 1, 0, 0, 0],
    [0, 2, 2, 1, 0, 0],
    [1, 3, 2, 2, 0, 0],
    [0, 3, 2, 2, 0, 0],
    [1, 3, 2, 1, 0, 0],
    [0, 3, 2, 1, 0, 0],
    [1, 3, 2, 0, 0, 0],
    [1, 0, 2, 1, 2, 0],
    [0, 0, 2, 1, 2, 0],
    [1, 0, 2, 2, 2, 0],
    [0, 0, 2, 2, 2, 0],
    [1, 0, 2, 3, 2, 0],
    [0, 1, 2, 2, 2, 0],
];

/// The triangle between the cell's center and its left, upper, right or
/// lower edge (`edge` 0 to 3), in cell coordinates.
fn in_quarter(edge: u32, u: f32, v: f32) -> bool {
    match edge {
        0 => u < v && u < 1.0 - v,
        1 => v < u && v < 1.0 - u,
        2 => u > v && u > 1.0 - v,
        _ => v > u && v > 1.0 - u,
    }
}

/// Distance from (`x`, `y`) to the segment from `a` to `b`.
fn segment_distance(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a.0 - t * dx).hypot(y - a.1 - t * dy)
}

/// Draws the Symbols for Legacy Computing (U+1FB00–U+1FBAF) that are
/// made of blocks, wedges, shades and lines, so that they tile like the
/// block elements. Returns `false` for other characters.
fn draw_legacy(canvas: &mut Canvas, c: char, stroke: u32) -> bool {
    const SHADE: u8 = 128;
    let (w, h) = (canvas.width, canvas.height);
    let eighth_h = |n: u32| (h * n + 4) / 8;
    let eighth_w = |n: u32| (w * n + 4) / 8;
    let i = (c as u32).wrapping_sub(0x1FB00);
    match i {
        // Sextants: a 2×3 grid, numbered across then down, without the
        // left and right halves, which are ▌ and ▐.
        0x00..=0x3B => {
            let mut bits = i + 1;
            if bits >= 21 {
                bits += 1;
            }
            if bits >= 42 {
                bits += 1;
            }
            let third = |n: u32| (h * n + 1) / 3;
            for cell in 0..6 {
                if bits & (1 << cell) != 0 {
                    let (x0, x1) = if cell % 2 == 0 {
                        (0, eighth_w(4))
                    } else {
                        (eighth_w(4), w)
                    };
                    canvas.fill(x0, third(cell / 2), x1, third(cell / 2 + 1), 255);
                }
            }
        }
        0x3C..=0x67 => {
            let [x1, y1, x2, y2, cx, cy] = MOSAICS[(i - 0x3C) as usize].map(f32::from);
            let side = |x: f32, y: f32| (x2 - x1) * (y - y1) - (y2 - y1) * (x - x1);
            let corner = side(cx, cy) > 0.0;
            canvas.shape(255, |u, v| (side(u * 2.0, v * 3.0) > 0.0) == corner);
        }
        // Triangles from the center to an edge: all but one, then one.
        0x68..=0x6F => canvas.shape(255, |u, v| in_quarter(i % 4, u, v) != (i < 0x6C)),
        // One-eighth columns and rows 2 to 7.
        0x70..=0x75 => canvas.fill(eighth_w(i - 0x70 + 1), 0, eighth_w(i - 0x70 + 2), h, 255),
        0x76..=0x7B => canvas.fill(0, eighth_h(i - 0x76 + 1), w, eighth_h(i - 0x76 + 2), 255),
        // Pairs of one-eighth edges: left and lower, left and upper, right
        // and upper, right and lower, upper and lower.
        0x7C..=0x80 => {
            let (left, right, upper, lower) = [
                (true, false, false, true),
                (true, false, true, false),
                (false, true, true, false),
                (false, true, false, true),
                (false, false, true, true),
            ][(i - 0x7C) as usize];
            if left {
                canvas.fill(0, 0, eighth_w(1), h, 255);
            }
            if right {
                canvas.fill(w - eighth_w(1), 0, w, h, 255);
            }
            if upper {
                canvas.fill(0, 0, w, eighth_h(1), 255);
            }
            if lower {
                canvas.fill(0, h - eighth_h(1), w, h, 255);
            }
        }
        // Rows 1, 3, 5 and 8.
        0x81 => {
            for row in [0, 2, 4, 7] {
                canvas.fill(0, eighth_h(row), w, eighth_h(row + 1), 255);
            }
        }
        // Upper and right blocks of 2, 3, 5, 6 and 7 eighths.
        0x82..=0x8B => {
            let n = [2, 3, 5, 6, 7][((i - 0x82) % 5) as usize];
            if i < 0x87 {
                canvas.fill(0, 0, w, eighth_h(n), 255);
            } else {
                canvas.fill(w - eighth_w(n), 0, w, h, 255);
            }
        }
        // Half shades, and halves of a shade and a block.
        0x8C => canvas.fill(0, 0, eighth_w(4), h, SHADE),
        0x8D => canvas.fill(w - eighth_w(4), 0, w, h, SHADE),
        0x8E => canvas.fill(0, 0, w, eighth_h(4), SHADE),
        0x8F => canvas.fill(0, h - eighth_h(4), w, h, SHADE),
        0x90 => canvas.fill(0, 0, w, h, SHADE),
        0x91 => {
            canvas.fill(0, 0, w, eighth_h(4), 255);
            canvas.fill(0, eighth_h(4), w, h, SHADE);
        }
        0x92 => {
            canvas.fill(0, 0, w, eighth_h(4), SHADE);
            canvas.fill(0, eighth_h(4), w, h, 255);
        }
        0x94 => {
            canvas.fill(0, 0, eighth_w(4), h, SHADE);
            canvas.fill(eighth_w(4), 0, w, h, 255);
        }
        // Checker boards of 4×8 squares, and 4 horizontal bars.
        0x95..=0x97 => {
            for row in 0..8 {
                for col in 0..4 {
                    let on = match i {
                        0x95 => (row + col) % 2 == 0,
                        0x96 => (row + col) % 2 == 1,
                        _ => row % 2 == 1,
                    };
                    if on {
                        canvas.fill(
                            w * col / 4,
                            eighth_h(row),
                            w * (col + 1) / 4,
                            eighth_h(row + 1),
                            255,
                        );
                    }
                }
            }
        }
        // Four diagonal stripes, parallel to the cell's diagonals so they
        // continue across cells.
        0x98 => canvas.shape(255, |u, v| (u - v).rem_euclid(0.25) < 0.125),
        0x99 => canvas.shape(255, |u, v| (u + v).rem_euclid(0.25) < 0.125),
        0x9A => canvas.shape(255, |u, v| in_quarter(1, u, v) || in_quarter(3, u, v)),
        0x9B => canvas.shape(255, |u, v| in_quarter(0, u, v) || in_quarter(2, u, v)),
        // Shaded triangles: upper left, upper right, lower right, lower left.
        0x9C => canvas.shape(SHADE, |u, v| u + v < 1.0),
        0x9D => canvas.shape(SHADE, |u, v| v < u),
        0x9E => canvas.shape(SHADE, |u, v| u + v > 1.0),
        0x9F => canvas.shape(SHADE, |u, v| v > u),
        // Lines between the middles of the edges, meeting the lines of
        // box-drawing characters next to them. Bits: upper to left 1, upper
        // to right 2, left to lower 4, right to lower 8.
        0xA0..=0xAE => {
            let bits = [1, 2, 4, 8, 5, 10, 12, 3, 9, 6, 14, 13, 11, 7, 15][(i - 0xA0) as usize];
            let s = stroke as f32;
            let mx = band_start(stroke, w) as f32 + s / 2.0;
            let my = band_start(stroke, h) as f32 + s / 2.0;
            let (up, down) = ((mx, 0.0), (mx, h as f32));
            let (left, right) = ((0.0, my), (w as f32, my));
            let segments = [(up, left), (up, right), (left, down), (right, down)];
            canvas.stroke(s, |x, y| {
                segments
                    .iter()
                    .enumerate()
                    .filter(|(bit, _)| bits & (1 << bit) != 0)
                    .map(|(_, &(a, b))| segment_distance(x, y, a, b))
                    .fold(f32::INFINITY, f32::min)
            });
        }
        // A horizontal line crossed by a short vertical stroke.
        0xAF => {
            draw_arms(canvas, stroke, [0, 0, 1, 1]);
            let x = band_start(stroke, w);
            canvas.fill(x, h / 3, x + stroke, h - h / 3, 255);
        }
        _ => return false,
    }
    true
}

/// The first and last bitmap rows with a pixel of at least `threshold`
/// coverage.
fn ink_rows(data: &[u8], width: u32, threshold: u8) -> Option<(u32, u32)> {
    let mut rows = data
        .chunks(width as usize)
        .enumerate()
        .filter(|(_, row)| row.iter().any(|&c| c >= threshold))
        .map(|(y, _)| y as u32);
    let first = rows.next()?;
    Some((first, rows.next_back().unwrap_or(first)))
}

/// Characters in the Private Use Areas, where Nerd Fonts put their icons
/// and powerline symbols.
fn is_private_use(c: char) -> bool {
    matches!(c,
        '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}')
}

/// Icons in the Private Use Areas, as used by Nerd Fonts, except powerline
/// separators (U+E0A0–U+E0D7), which must keep their position to line up
/// with the cell backgrounds next to them.
fn is_icon(c: char) -> bool {
    matches!(c,
        '\u{E000}'..='\u{E09F}'
        | '\u{E0D8}'..='\u{F8FF}'
        | '\u{F0000}'..='\u{FFFFD}'
        | '\u{100000}'..='\u{10FFFD}')
}

/// Maps a font file into memory for the rest of the process. Unlike
/// reading it, only the parts in use take memory, and the kernel can share
/// and reclaim them; that matters for fallback fonts such as Noto CJK,
/// which are tens of megabytes. Faces are never unloaded, so neither are
/// the mappings.
fn map_file(path: &Path) -> std::io::Result<&'static [u8]> {
    let file = std::fs::File::open(path)?;
    let len = usize::try_from(file.metadata()?.len()).map_err(std::io::Error::other)?;
    if len == 0 {
        return Err(std::io::Error::other("empty file"));
    }
    // SAFETY: a private read-only mapping of an open file, which stays valid
    // after the descriptor is closed. Font files are not modified in place
    // (package updates replace them), as FreeType also assumes when it maps
    // them.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            std::os::fd::AsRawFd::as_raw_fd(&file),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the mapping is `len` readable bytes and is never unmapped.
    Ok(unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) })
}

fn has_glyph(face: &FontRef, c: char) -> bool {
    face.glyph_id(c).0 != 0
}

/// The ab_glyph scale for a size of `px` pixels per em. ab_glyph scales by
/// the ascent-to-descent height instead of the em size.
fn px_scale(face: &FontRef, px: f32) -> PxScale {
    match face.units_per_em() {
        Some(units) if units > 0.0 => PxScale::from(px * face.height_unscaled() / units),
        _ => PxScale::from(px),
    }
}

/// Converts a size in points to pixels at the given scale (96 DPI).
pub fn points_to_px(points: f32, scale: f32) -> f32 {
    points * 96.0 / 72.0 * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_selection() {
        assert_eq!(Style::new(false, false), Style::Regular);
        assert_eq!(Style::new(true, true), Style::BoldItalic);
        assert_eq!(Style::BoldItalic as usize, 3);
    }

    #[test]
    fn cell_metrics_from_line_metrics() {
        let m = CellMetrics::new(8.4, 12.0, -3.0, 1.0);
        assert_eq!(m.width, 8);
        assert_eq!(m.height, 16);
        assert_eq!(m.baseline, 13);
        assert_eq!(m.stroke, 1);
        assert!(m.underline < m.height);
    }

    #[test]
    fn degenerate_metrics_stay_positive() {
        let m = CellMetrics::new(0.0, 0.0, 0.0, 0.0);
        assert_eq!((m.width, m.height), (1, 1));
        assert!(m.underline < m.height);
    }

    #[test]
    fn points_conversion() {
        assert_eq!(points_to_px(12.0, 1.0), 16.0);
        assert_eq!(points_to_px(12.0, 2.0), 32.0);
    }

    fn coverage(glyph: &Glyph, x: u32, y: u32) -> u8 {
        glyph.data[(y * glyph.width + x) as usize]
    }

    #[test]
    fn box_lines_reach_cell_edges() {
        let m = CellMetrics::new(8.0, 13.0, -3.0, 1.0);
        let (w, h) = (m.width, m.height);
        let horizontal = box_glyph('─', m).unwrap();
        assert_eq!((horizontal.width, horizontal.height), (w, h));
        let mid = (0..h).find(|&y| coverage(&horizontal, 0, y) > 0).unwrap();
        assert!((0..w).all(|x| coverage(&horizontal, x, mid) == 255));
        assert_eq!(coverage(&horizontal, 0, 0), 0);

        let vertical = box_glyph('│', m).unwrap();
        let mid = (0..w).find(|&x| coverage(&vertical, x, 0) > 0).unwrap();
        assert!((0..h).all(|y| coverage(&vertical, mid, y) == 255));

        // A corner covers the center and reaches only its two edges.
        let corner = box_glyph('┌', m).unwrap();
        assert!((0..h).any(|y| coverage(&corner, w - 1, y) > 0));
        assert!((0..w).any(|x| coverage(&corner, x, h - 1) > 0));
        assert!((0..h).all(|y| coverage(&corner, 0, y) == 0));
        assert!((0..w).all(|x| coverage(&corner, x, 0) == 0));
    }

    #[test]
    fn whole_box_drawing_range_is_drawn() {
        let m = CellMetrics::new(10.0, 16.0, -5.0, 0.0);
        for c in '\u{2500}'..='\u{259F}' {
            let glyph = box_glyph(c, m).unwrap_or_else(|| panic!("{c} not drawn"));
            assert!(glyph.data.iter().any(|&p| p > 0), "{c} is blank");
        }
    }

    /// Ink in a column (or row, when `row`) of a glyph, as indices.
    fn ink(glyph: &Glyph, index: u32, row: bool) -> Vec<u32> {
        let (len, cov): (u32, Box<dyn Fn(u32) -> u8>) = if row {
            (glyph.width, Box::new(|x| coverage(glyph, x, index)))
        } else {
            (glyph.height, Box::new(|y| coverage(glyph, index, y)))
        };
        (0..len).filter(|&i| cov(i) > 127).collect()
    }

    #[test]
    fn double_lines_join() {
        let m = CellMetrics::new(10.0, 16.0, -5.0, 0.0);
        let (w, h) = (m.width, m.height);
        // ═ is two lines across the cell; ║ two lines down it.
        let horizontal = ink(&box_glyph('═', m).unwrap(), 0, false);
        assert_eq!(horizontal.len(), 2);
        assert_eq!(horizontal, ink(&box_glyph('═', m).unwrap(), w - 1, false));
        let vertical = ink(&box_glyph('║', m).unwrap(), 0, true);
        assert_eq!(vertical.len(), 2);
        // Each corner and tee continues them at its edges.
        for c in ['╔', '╦', '╬', '╠', '╚'] {
            let glyph = box_glyph(c, m).unwrap();
            assert_eq!(ink(&glyph, w - 1, false), horizontal, "{c}");
        }
        for c in ['╔', '╦', '╬', '╣', '╗'] {
            let glyph = box_glyph(c, m).unwrap();
            assert_eq!(ink(&glyph, h - 1, true), vertical, "{c}");
        }
        // The outer lines of ╔ meet; the inner ones turn a corner inside.
        let corner = box_glyph('╔', m).unwrap();
        assert_eq!(coverage(&corner, vertical[0], horizontal[0]), 255);
        assert_eq!(coverage(&corner, vertical[1], horizontal[0] + 1), 0);
        assert_eq!(coverage(&corner, vertical[1], horizontal[1]), 255);
    }

    #[test]
    fn single_line_stops_at_a_double_tee() {
        let m = CellMetrics::new(10.0, 16.0, -5.0, 0.0);
        let lines = ink(&box_glyph('═', m).unwrap(), 0, false);
        let middle = ink(&box_glyph('│', m).unwrap(), 0, true)[0];
        // ╤ reaches only the lower line; ╪ crosses both, and so does ╒,
        // which turns into the upper one.
        let tee = box_glyph('╤', m).unwrap();
        assert_eq!(coverage(&tee, middle, lines[0] + 1), 0);
        for c in ['╪', '╒'] {
            let glyph = box_glyph(c, m).unwrap();
            assert_eq!(coverage(&glyph, middle, lines[0] + 1), 255, "{c}");
        }
    }

    #[test]
    fn arcs_meet_straight_lines() {
        for m in [
            CellMetrics::new(10.0, 16.0, -5.0, 0.0),
            CellMetrics::new(11.0, 24.0, -8.0, 0.0),
        ] {
            let (w, h) = (m.width, m.height);
            let row = ink(&box_glyph('─', m).unwrap(), 0, false);
            let column = ink(&box_glyph('│', m).unwrap(), 0, true);
            for (c, x, y) in [
                ('╭', w - 1, h - 1),
                ('╮', 0, h - 1),
                ('╯', 0, 0),
                ('╰', w - 1, 0),
            ] {
                let arc = box_glyph(c, m).unwrap();
                assert_eq!(ink(&arc, x, false), row, "{c} at its side edge");
                assert_eq!(ink(&arc, y, true), column, "{c} at its end edge");
            }
        }
    }

    #[test]
    fn braille_dots_are_evenly_spaced() {
        let m = CellMetrics::new(10.0, 16.0, -5.0, 0.0);
        let (w, h) = (m.width, m.height);
        assert!(box_glyph('\u{2800}', m).is_none());
        // Dot 1 alone is in the top left quarter.
        let one = box_glyph('⠁', m).unwrap();
        let first_ink = one.data.iter().position(|&p| p > 0).unwrap() as u32;
        assert!(first_ink % w < w / 2 && first_ink / w < h / 4);
        // All eight dots: the gaps between them, including those to the
        // next cell over, are equal (within a pixel of rounding).
        let full = box_glyph('⣿', m).unwrap();
        let starts = |ink: Vec<u32>, size: u32| {
            let mut starts: Vec<u32> = ink
                .iter()
                .copied()
                .filter(|&i| i == 0 || !ink.contains(&(i - 1)))
                .collect();
            starts.push(starts[0] + size);
            starts.windows(2).map(|p| p[1] - p[0]).collect::<Vec<_>>()
        };
        let corner = full.data.iter().position(|&p| p > 0).unwrap() as u32;
        let row = ink(&full, corner / w, true);
        let column = ink(&full, corner % w, false);
        for (pitches, cells) in [(starts(row, w), 2), (starts(column, h), 4)] {
            assert_eq!(pitches.len(), cells, "{pitches:?}");
            let (lo, hi) = (pitches.iter().min().unwrap(), pitches.iter().max().unwrap());
            assert!(hi - lo <= 1, "{pitches:?}");
        }
    }

    #[test]
    fn legacy_computing_symbols() {
        let m = CellMetrics::new(10.0, 16.0, -5.0, 0.0);
        let (w, h) = (m.width, m.height);
        for c in '\u{1FB00}'..='\u{1FBAF}' {
            if c == '\u{1FB93}' {
                continue; // unassigned
            }
            let glyph = box_glyph(c, m).unwrap_or_else(|| panic!("{c} not drawn"));
            assert!(glyph.data.iter().any(|&p| p > 0), "{c} is blank");
        }

        // Sextant 1 is the upper left cell of the 2×3 grid; sextant
        // 2-3-4-5-6 everything else.
        let one = box_glyph('🬀', m).unwrap();
        assert_eq!(coverage(&one, 0, 0), 255);
        assert_eq!(coverage(&one, w - 1, 0), 0);
        assert_eq!(coverage(&one, 0, h / 2), 0);
        let most = box_glyph('🬻', m).unwrap();
        assert_eq!(coverage(&most, 0, 0), 0);
        assert!(
            [(w - 1, 0), (0, h / 2), (w - 1, h - 1), (0, h - 1)]
                .iter()
                .all(|&(x, y)| coverage(&most, x, y) == 255)
        );

        // Each smooth mosaic fills its corner and leaves the opposite one.
        for (i, &[.., cx, cy]) in MOSAICS.iter().enumerate() {
            let c = char::from_u32(0x1FB3C + i as u32).unwrap();
            let glyph = box_glyph(c, m).unwrap();
            let (x, y) = (
                if cx == 0 { 0 } else { w - 1 },
                if cy == 0 { 0 } else { h - 1 },
            );
            assert_eq!(coverage(&glyph, x, y), 255, "{c} corner");
            assert_eq!(coverage(&glyph, w - 1 - x, h - 1 - y), 0, "{c} opposite");
        }

        // The checker boards are each other's inverse.
        let (a, b) = (box_glyph('🮕', m).unwrap(), box_glyph('🮖', m).unwrap());
        assert!(a.data.iter().zip(&b.data).all(|(p, q)| p ^ q == 255));
    }

    #[test]
    fn dashes_and_quadrants() {
        let m = CellMetrics::new(12.0, 16.0, -5.0, 0.0);
        let row = ink(&box_glyph('─', m).unwrap(), 0, false)[0];
        let dashes = |c| {
            let glyph = box_glyph(c, m).unwrap();
            let ink = ink(&glyph, row, true);
            ink.windows(2).filter(|p| p[1] != p[0] + 1).count() + 1
        };
        assert_eq!((dashes('╌'), dashes('┄'), dashes('┈')), (2, 3, 4));

        let quadrants = box_glyph('▚', m).unwrap();
        let (w, h) = (m.width, m.height);
        assert_eq!(coverage(&quadrants, 0, 0), 255);
        assert_eq!(coverage(&quadrants, w - 1, 0), 0);
        assert_eq!(coverage(&quadrants, 0, h - 1), 0);
        assert_eq!(coverage(&quadrants, w - 1, h - 1), 255);
    }

    fn columns_with_ink(glyph: &Glyph) -> Vec<u32> {
        (0..glyph.width)
            .filter(|&x| (0..glyph.height).any(|y| coverage(glyph, x, y) > 0))
            .collect()
    }

    #[test]
    fn underlines_fit_in_the_cell() {
        let m = CellMetrics::new(9.0, 17.0, -5.0, 0.0);
        use Underline::*;
        for style in [Single, Double, Curly, Dotted, Dashed] {
            for cells in [1, 2] {
                let g = underline_glyph(style, m, cells).unwrap();
                assert_eq!(g.width, m.width * cells, "{style:?}");
                assert!(
                    g.top >= 0 && g.top as u32 + g.height <= m.height,
                    "{style:?} {g:?}"
                );
                assert!(g.data.iter().any(|&c| c > 0), "{style:?}");
            }
        }
        assert!(underline_glyph(None, m, 1).is_none());
    }

    #[test]
    fn solid_underlines_cover_every_column() {
        let m = CellMetrics::new(9.0, 17.0, -5.0, 0.0);
        for style in [Underline::Single, Underline::Double, Underline::Curly] {
            let g = underline_glyph(style, m, 2).unwrap();
            assert_eq!(columns_with_ink(&g).len() as u32, g.width, "{style:?}");
        }
        let double = underline_glyph(Underline::Double, m, 1).unwrap();
        let rows: Vec<_> = (0..double.height)
            .map(|y| coverage(&double, 0, y))
            .collect();
        assert_eq!(rows, [255, 0, 255]);
    }

    #[test]
    fn broken_underlines_have_gaps_and_repeat_per_cell() {
        let m = CellMetrics::new(12.0, 17.0, -5.0, 0.0);
        for style in [Underline::Dotted, Underline::Dashed] {
            let one = underline_glyph(style, m, 1).unwrap();
            let two = underline_glyph(style, m, 2).unwrap();
            let inked = columns_with_ink(&one);
            assert!(
                !inked.is_empty() && (inked.len() as u32) < one.width,
                "{style:?}"
            );
            // The second cell repeats the first.
            let shifted: Vec<_> = columns_with_ink(&two)
                .into_iter()
                .filter(|&x| x >= m.width)
                .map(|x| x - m.width)
                .collect();
            assert_eq!(shifted, inked, "{style:?}");
        }
    }

    #[test]
    fn curly_wave_joins_across_cells() {
        let m = CellMetrics::new(10.0, 18.0, -4.0, 0.0);
        let g = underline_glyph(Underline::Curly, m, 2).unwrap();
        let column = |x: u32| {
            (0..g.height)
                .map(|y| coverage(&g, x, y))
                .collect::<Vec<_>>()
        };
        // Same phase at the start of each cell, and it actually waves.
        assert_eq!(column(0), column(m.width));
        assert_ne!(column(0), column(m.width / 2));
    }

    #[test]
    fn ink_rows_ignore_faint_pixels() {
        let data = [0, 0, 10, 0, 200, 0, 90, 90, 20, 0, 0, 0];
        assert_eq!(ink_rows(&data, 3, 64), Some((1, 2)));
        assert_eq!(ink_rows(&[0; 6], 3, 64), None);
    }

    #[test]
    fn icon_ranges() {
        assert!(is_icon('\u{F43A}')); // nf-oct-clock
        assert!(is_icon('\u{E7A8}')); // nf-dev-rust
        assert!(is_icon('\u{F08C7}')); // nf-md-arch
        assert!(!is_icon('\u{E0B0}')); // powerline arrow
        assert!(!is_icon('\u{E0B6}')); // powerline round cap
        // Powerline separators and icons are never shrunk to fit the cell.
        assert!(is_private_use('\u{E0B0}'));
        assert!(is_private_use('\u{F43A}'));
        assert!(!is_private_use('☺'));
        assert!(!is_icon('A'));
        assert!(!is_icon('中'));
    }

    #[test]
    fn block_elements() {
        let m = CellMetrics::new(8.0, 12.0, -4.0, 0.0);
        let full = box_glyph('█', m).unwrap();
        assert!(full.data.iter().all(|&c| c == 255));
        let upper = box_glyph('▀', m).unwrap();
        assert_eq!(coverage(&upper, 0, 0), 255);
        assert_eq!(coverage(&upper, 0, m.height - 1), 0);
        let lower = box_glyph('▄', m).unwrap();
        assert_eq!(coverage(&lower, 0, 0), 0);
        assert_eq!(coverage(&lower, 0, m.height - 1), 255);
        let left = box_glyph('▌', m).unwrap();
        assert_eq!(coverage(&left, 0, 0), 255);
        assert_eq!(coverage(&left, m.width - 1, 0), 0);
        assert_eq!(box_glyph('▒', m).unwrap().data[0], 128);
        assert!(box_glyph('a', m).is_none());
    }

    #[test]
    fn overlay_unites_bitmaps() {
        let a = Glyph {
            width: 2,
            height: 1,
            left: 0,
            top: 5,
            data: vec![10, 20],
            color: false,
        };
        let b = Glyph {
            width: 1,
            height: 2,
            left: 1,
            top: 3,
            data: vec![30, 40],
            color: false,
        };
        let c = overlay(&a, &b);
        assert_eq!((c.left, c.top, c.width, c.height), (0, 3, 2, 3));
        assert_eq!(c.data, [0, 30, 0, 40, 10, 20]);
    }

    /// Needs fonts installed; skipped (passes) where none are available,
    /// such as the Nix build sandbox.
    #[test]
    fn loads_system_monospace() {
        let Ok(mut fonts) = Fonts::new("monospace", &[], 16.0) else {
            eprintln!("no system fonts, skipping");
            return;
        };
        let m = fonts.metrics;
        assert!(m.width > 0 && m.height > m.width);
        let glyph = fonts
            .rasterize('A', Style::Regular, false)
            .expect("glyph for A");
        assert_eq!(glyph.data.len(), (glyph.width * glyph.height) as usize);
        assert!(glyph.top >= 0);
        assert!(fonts.rasterize(' ', Style::Regular, false).is_none());

        // In a two-cell slot, a narrow glyph is enlarged and centered.
        let wide = fonts
            .rasterize('A', Style::Regular, true)
            .expect("wide glyph for A");
        assert!(wide.height > glyph.height);
        assert!(wide.height <= m.height);
        let right_gap = 2 * m.width as i32 - (wide.left + wide.width as i32);
        assert!((wide.left - right_gap).abs() <= 1);

        // Icons, when some installed font has them, are centered vertically.
        if let Some(icon) = fonts.rasterize('\u{F43A}', Style::Regular, false) {
            let (first, last) = ink_rows(&icon.data, icon.width, 64).unwrap();
            let top_gap = icon.top + first as i32;
            let bottom_gap = m.height as i32 - (icon.top + last as i32 + 1);
            assert!(
                (top_gap - bottom_gap).abs() <= 1,
                "{top_gap} vs {bottom_gap}"
            );
        }

        // Thai ส + ำ is one cluster over two cells: ส keeps its size and
        // ำ, a spacing vowel, is drawn in the second cell.
        let base = fonts.rasterize('ส', Style::Regular, false);
        let vowel = fonts.rasterize('ำ', Style::Regular, false);
        let cluster = fonts.rasterize_cluster("สำ", Style::Regular, true);
        if let (Some(base), Some(vowel), Some(cluster)) = (base, vowel, cluster) {
            let top = base.top.min(vowel.top);
            let bottom = (base.top + base.height as i32).max(vowel.top + vowel.height as i32);
            assert_eq!(
                (cluster.top, cluster.top + cluster.height as i32),
                (top, bottom)
            );
            assert!(cluster.left + cluster.width as i32 > m.width as i32);
        }

        // Symbols, often from a fallback font, stay inside their cell.
        for c in ['☺', '♀', '♥'] {
            if let Some(glyph) = fonts.rasterize(c, Style::Regular, false) {
                assert!(glyph.left >= 0, "{c}");
                assert!(glyph.left + glyph.width as i32 <= m.width as i32, "{c}");
                assert!(glyph.top >= 0, "{c}");
                assert!(glyph.top + glyph.height as i32 <= m.height as i32, "{c}");
            }
        }

        fonts.set_size(32.0);
        assert!(fonts.metrics.height > m.height);
    }
}
