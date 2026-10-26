//! Font discovery (fontconfig) and glyph rasterization (fontdue).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fmt;
use std::path::PathBuf;

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
        let baseline = ((line_gap / 2.0) + ascent).round().clamp(0.0, height as f32 - 1.0) as u32;
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
                // Color (bitmap) fonts cannot be rendered by fontdue.
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
    faces: Vec<fontdue::Font>,
    loaded: HashMap<FontFile, usize>,
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
        for (slot, style) in [Style::Bold, Style::Italic, Style::BoldItalic].into_iter().enumerate() {
            match fonts.find_and_load(family, style, None) {
                Ok(face) => fonts.styles[slot + 1] = face,
                Err(e) => log::warn!("{e}; using the regular face for {style:?}"),
            }
        }
        fonts.set_size(px);
        Ok(fonts)
    }

    fn find_and_load(&mut self, family: &str, style: Style, c: Option<char>) -> Result<usize, FontError> {
        let file = self
            .fontconfig
            .find(family, style, c)
            .ok_or_else(|| FontError::NotFound(family.into()))?;
        self.load(file)
    }

    fn load(&mut self, file: FontFile) -> Result<usize, FontError> {
        if let Some(&face) = self.loaded.get(&file) {
            return Ok(face);
        }
        log::info!("loading font {} (index {})", file.path.display(), file.index);
        let data = std::fs::read(&file.path).map_err(|_| FontError::Load(file.path.clone(), "cannot read file"))?;
        let settings = fontdue::FontSettings {
            collection_index: file.index,
            ..fontdue::FontSettings::default()
        };
        let face = fontdue::Font::from_bytes(data, settings).map_err(|e| FontError::Load(file.path.clone(), e))?;
        self.faces.push(face);
        let index = self.faces.len() - 1;
        self.loaded.insert(file, index);
        Ok(index)
    }

    pub fn size(&self) -> f32 {
        self.px
    }

    pub fn set_size(&mut self, px: f32) {
        self.px = px;
        let face = &self.faces[self.styles[0]];
        let advance = ['M', '0', 'x']
            .into_iter()
            .find(|&c| face.lookup_glyph_index(c) != 0)
            .map(|c| face.metrics(c, px).advance_width)
            .unwrap_or(px * 0.6);
        let (ascent, descent, gap) = match face.horizontal_line_metrics(px) {
            Some(m) => (m.ascent, m.descent, m.line_gap),
            None => (px * 0.8, -px * 0.2, 0.0),
        };
        self.metrics = CellMetrics::new(advance, ascent, descent, gap);
    }

    /// The face used to draw `c` in `style`.
    fn face_for(&mut self, c: char, style: Style) -> Option<usize> {
        let styled = self.styles[style as usize];
        if self.faces[styled].lookup_glyph_index(c) != 0 {
            return Some(styled);
        }
        let regular = self.styles[0];
        if self.faces[regular].lookup_glyph_index(c) != 0 {
            return Some(regular);
        }
        if let Some(&face) = self.fallback.get(&c) {
            return face;
        }
        let families: Vec<String> = self.fallback_families.iter().cloned().chain([String::new()]).collect();
        let face = families.iter().find_map(|family| {
            let face = self.find_and_load(family, Style::Regular, Some(c)).ok()?;
            (self.faces[face].lookup_glyph_index(c) != 0).then_some(face)
        });
        if face.is_none() {
            log::debug!("no font covers {c:?} (U+{:04X})", c as u32);
        }
        self.fallback.insert(c, face);
        face
    }

    /// Rasterizes `c`. Returns `None` for characters with no visible pixels.
    pub fn rasterize(&mut self, c: char, style: Style) -> Option<Glyph> {
        if c == ' ' {
            return None;
        }
        let face = self.face_for(c, style).unwrap_or(self.styles[0]);
        let (m, data) = self.faces[face].rasterize(c, self.px);
        if m.width == 0 || m.height == 0 {
            return None;
        }
        Some(Glyph {
            width: m.width as u32,
            height: m.height as u32,
            left: m.xmin,
            top: self.metrics.baseline as i32 - (m.ymin + m.height as i32),
            data,
        })
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
        let glyph = fonts.rasterize('A', Style::Regular).expect("glyph for A");
        assert_eq!(glyph.data.len(), (glyph.width * glyph.height) as usize);
        assert!(glyph.top >= 0);
        assert!(fonts.rasterize(' ', Style::Regular).is_none());

        fonts.set_size(32.0);
        assert!(fonts.metrics.height > m.height);
    }
}
