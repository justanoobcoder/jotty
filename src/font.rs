//! Font discovery (fontconfig) and glyph rasterization (ab_glyph).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fmt;
use std::path::PathBuf;

use unicode_width::UnicodeWidthChar;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, VariableFont, point};
use fontconfig_sys as fc;
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

/// A rasterized glyph: an 8-bit coverage bitmap and its placement.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub width: u32,
    pub height: u32,
    /// Offset of the bitmap's left edge from the cell's left edge.
    pub left: i32,
    /// Offset of the bitmap's top edge from the cell's top edge.
    pub top: i32,
    pub data: Vec<u8>,
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

impl Drop for Fontconfig {
    fn drop(&mut self) {
        // SAFETY: we own the config.
        unsafe { fc::FcConfigDestroy(self.0) }
    }
}

pub struct Fonts {
    fontconfig: Fontconfig,
    faces: Vec<FontVec>,
    /// Loaded faces by file and style (variable fonts differ per style).
    loaded: HashMap<(FontFile, Style), usize>,
    /// Face index for each style.
    styles: [usize; 4],
    fallback_families: Vec<String>,
    /// Face covering a character none of the primary faces have.
    fallback: HashMap<char, Option<usize>>,
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
            loaded: HashMap::new(),
            styles: [0; 4],
            fallback_families: fallback.to_vec(),
            fallback: HashMap::new(),
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
        let data = std::fs::read(&file.path)
            .map_err(|_| FontError::Load(file.path.clone(), "cannot read file"))?;
        // fontconfig puts the named instance of a variable font in the high
        // 16 bits; only the face index matters here (the default instance
        // is used).
        let mut face = FontVec::try_from_vec_and_index(data, file.index & 0xffff)
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

    /// Rasterizes `c` for a one-cell slot, or a two-cell slot when `wide`.
    /// Returns `None` for characters with no visible pixels.
    pub fn rasterize(&mut self, c: char, style: Style, wide: bool) -> Option<Glyph> {
        if c == ' ' {
            return None;
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
        let (left, top) = if fill_slot {
            (
                (slot_width as i32 - width as i32) / 2,
                (m.height as i32 - height as i32) / 2,
            )
        } else {
            (bounds.min.x as i32, bounds.min.y as i32)
        };
        Some(Glyph {
            width,
            height,
            left,
            top,
            data,
        })
    }
}

/// Line weights of the four arms (up, down, left, right) of a box-drawing
/// character: 0 none, 1 light, 2 heavy.
fn box_arms(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' => [0, 0, 1, 1],
        '━' => [0, 0, 2, 2],
        '│' => [1, 1, 0, 0],
        '┃' => [2, 2, 0, 0],
        '┌' | '╭' => [0, 1, 0, 1],
        '┏' => [0, 2, 0, 2],
        '┐' | '╮' => [0, 1, 1, 0],
        '┓' => [0, 2, 2, 0],
        '└' | '╰' => [1, 0, 0, 1],
        '┗' => [2, 0, 0, 2],
        '┘' | '╯' => [1, 0, 1, 0],
        '┛' => [2, 0, 2, 0],
        '├' => [1, 1, 0, 1],
        '┣' => [2, 2, 0, 2],
        '┤' => [1, 1, 1, 0],
        '┫' => [2, 2, 2, 0],
        '┬' => [0, 1, 1, 1],
        '┳' => [0, 2, 2, 2],
        '┴' => [1, 0, 1, 1],
        '┻' => [2, 0, 2, 2],
        '┼' => [1, 1, 1, 1],
        '╋' => [2, 2, 2, 2],
        '╴' => [0, 0, 1, 0],
        '╵' => [1, 0, 0, 0],
        '╶' => [0, 0, 0, 1],
        '╷' => [0, 1, 0, 0],
        _ => return None,
    })
}

/// Draws box-drawing and block characters to fill the cell exactly, so
/// lines join across cells regardless of the font's metrics.
fn box_glyph(c: char, m: CellMetrics) -> Option<Glyph> {
    let (w, h) = (m.width, m.height);
    let mut data = vec![0u8; (w * h) as usize];
    let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32, value: u8| {
        for y in y0.min(h)..y1.min(h) {
            for x in x0.min(w)..x1.min(w) {
                data[(y * w + x) as usize] = value;
            }
        }
    };
    if let Some(arms) = box_arms(c) {
        let thickness = |weight: u8| m.stroke * u32::from(weight);
        let heaviest = thickness(*arms.iter().max().unwrap());
        let (cx, cy) = ((w - heaviest.min(w)) / 2, (h - heaviest.min(h)) / 2);
        let [up, down, left, right] = arms.map(thickness);
        let center = |t: u32, size: u32| (size - t.min(size)) / 2;
        if up > 0 {
            fill(center(up, w), 0, center(up, w) + up, cy + heaviest, 255);
        }
        if down > 0 {
            fill(center(down, w), cy, center(down, w) + down, h, 255);
        }
        if left > 0 {
            fill(
                0,
                center(left, h),
                cx + heaviest,
                center(left, h) + left,
                255,
            );
        }
        if right > 0 {
            fill(cx, center(right, h), w, center(right, h) + right, 255);
        }
    } else {
        let eighth_h = |n: u32| (h * n + 4) / 8;
        let eighth_w = |n: u32| (w * n + 4) / 8;
        match c {
            '█' => fill(0, 0, w, h, 255),
            '▀' => fill(0, 0, w, eighth_h(4), 255),
            '▔' => fill(0, 0, w, eighth_h(1), 255),
            '▕' => fill(w - eighth_w(1), 0, w, h, 255),
            '▐' => fill(w - eighth_w(4), 0, w, h, 255),
            '░' => fill(0, 0, w, h, 64),
            '▒' => fill(0, 0, w, h, 128),
            '▓' => fill(0, 0, w, h, 192),
            // Lower one eighth to seven eighths (including the lower half).
            '▁'..='▇' => {
                let n = c as u32 - '▁' as u32 + 1;
                fill(0, h - eighth_h(n), w, h, 255);
            }
            // Left seven eighths down to one eighth (including the left half).
            '▉'..='▏' => {
                let n = 7 - (c as u32 - '▉' as u32);
                fill(0, 0, eighth_w(n), h, 255);
            }
            _ => return None,
        }
    }
    Some(Glyph {
        width: w,
        height: h,
        left: 0,
        top: 0,
        data,
    })
}

fn has_glyph(face: &FontVec, c: char) -> bool {
    face.glyph_id(c).0 != 0
}

/// The ab_glyph scale for a size of `px` pixels per em. ab_glyph scales by
/// the ascent-to-descent height instead of the em size.
fn px_scale(face: &FontVec, px: f32) -> PxScale {
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

        fonts.set_size(32.0);
        assert!(fonts.metrics.height > m.height);
    }
}
