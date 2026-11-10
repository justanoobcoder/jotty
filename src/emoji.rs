//! Color emoji from bitmap fonts (CBDT or sbix, as in Noto Color Emoji),
//! with the font's ligatures for sequences: ZWJ sequences, flags, skin
//! tones and keycaps become one glyph.

use ttf_parser::gsub::SubstitutionSubtable;
use ttf_parser::{Face, GlyphId, RasterImageFormat};

/// A font with color bitmaps for emoji.
pub struct EmojiFont {
    face: Face<'static>,
}

impl EmojiFont {
    /// Parses face `index` of `data`. Fails unless it has color bitmaps.
    pub fn try_new(data: &'static [u8], index: u32) -> Result<EmojiFont, &'static str> {
        let face = Face::parse(data, index).map_err(|_| "not a font file")?;
        let tables = face.tables();
        if tables.cbdt.is_some() || tables.sbix.is_some() {
            Ok(EmojiFont { face })
        } else {
            Err("no color bitmaps")
        }
    }

    /// The glyph showing `text` (a character or emoji sequence), after the
    /// font's ligatures. `None` if the font lacks its first character.
    pub fn glyph_for(&self, text: &str) -> Option<GlyphId> {
        let first = self.face.glyph_index(text.chars().next()?)?;
        // Characters the font has no glyph for, typically VS16, are left
        // out; the ligatures don't include them either.
        let mut glyphs: Vec<GlyphId> = std::iter::once(first)
            .chain(
                text.chars()
                    .skip(1)
                    .filter_map(|c| self.face.glyph_index(c)),
            )
            .collect();
        apply_ligatures(&self.face, &mut glyphs);
        glyphs.first().copied()
    }

    /// Draws `glyph` scaled to fit a `width`×`height` box, keeping its
    /// aspect ratio. Returns its size and premultiplied RGBA pixels.
    pub fn rasterize(
        &self,
        glyph: GlyphId,
        width: u32,
        height: u32,
    ) -> Option<(u32, u32, Vec<u8>)> {
        let image = self.face.glyph_raster_image(glyph, height.max(1) as u16)?;
        let (src_w, src_h, rgba) = match image.format {
            RasterImageFormat::PNG => decode_png(image.data).ok()?,
            RasterImageFormat::BitmapPremulBgra32 => {
                let (w, h) = (u32::from(image.width), u32::from(image.height));
                // Not in u32: the sides come from the font file, up to 65535.
                let size = w as usize * h as usize * 4;
                let mut rgba = image.data.get(..size)?.to_vec();
                for px in rgba.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                (w, h, rgba)
            }
            _ => return None,
        };
        let scale = (width as f32 / src_w as f32).min(height as f32 / src_h as f32);
        let w = ((src_w as f32 * scale).round() as u32).clamp(1, width.max(1));
        let h = ((src_h as f32 * scale).round() as u32).clamp(1, height.max(1));
        Some((w, h, resize(&rgba, (src_w, src_h), (w, h))))
    }
}

/// Applies the font's ligature substitutions (GSUB lookup type 4) to
/// `glyphs`, in lookup order. This is all emoji fonts need for sequences;
/// other substitution types are not applied.
fn apply_ligatures(face: &Face, glyphs: &mut Vec<GlyphId>) {
    let Some(gsub) = face.tables().gsub else {
        return;
    };
    for lookup in gsub.lookups {
        for subtable in lookup.subtables.into_iter::<SubstitutionSubtable>() {
            let SubstitutionSubtable::Ligature(table) = subtable else {
                continue;
            };
            let mut i = 0;
            while i < glyphs.len() {
                let set = table
                    .coverage
                    .get(glyphs[i])
                    .and_then(|index| table.ligature_sets.get(index));
                if let Some(set) = set {
                    for ligature in set {
                        let rest = &glyphs[i + 1..];
                        let n = ligature.components.len() as usize;
                        if rest.len() >= n
                            && ligature
                                .components
                                .into_iter()
                                .eq(rest[..n].iter().copied())
                        {
                            glyphs.splice(i..=i + n, [ligature.glyph]);
                            break;
                        }
                    }
                }
                i += 1;
            }
        }
    }
}

/// Decodes a PNG into premultiplied RGBA.
fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), &'static str> {
    const BAD_PNG: &str = "not a valid PNG";
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(|_| BAD_PNG)?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("the PNG is too large")?];
    let info = reader.next_frame(&mut buf).map_err(|_| BAD_PNG)?;
    let pixels = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        _ => return Err("unsupported PNG color type"),
    };
    let mut rgba = rgba;
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
    Ok((info.width, info.height, rgba))
}

/// Resizes premultiplied RGBA pixels by averaging the source area under
/// each destination pixel, which keeps downscaled emoji smooth.
fn resize(src: &[u8], (sw, sh): (u32, u32), (dw, dh): (u32, u32)) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    for y in 0..dh {
        let (y0, y1) = (y as f32 * fy, (y + 1) as f32 * fy);
        for x in 0..dw {
            let (x0, x1) = (x as f32 * fx, (x + 1) as f32 * fx);
            let mut sum = [0f32; 4];
            let mut total = 0f32;
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(sh) {
                let wy = (y1.min(sy as f32 + 1.0) - y0.max(sy as f32)).max(0.0);
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(sw) {
                    let wx = (x1.min(sx as f32 + 1.0) - x0.max(sx as f32)).max(0.0);
                    let weight = wx * wy;
                    let i = ((sy * sw + sx) * 4) as usize;
                    for (s, &v) in sum.iter_mut().zip(&src[i..i + 4]) {
                        *s += f32::from(v) * weight;
                    }
                    total += weight;
                }
            }
            let i = ((y * dw + x) * 4) as usize;
            for (o, s) in out[i..i + 4].iter_mut().zip(sum) {
                *o = (s / total.max(f32::EPSILON)).round() as u8;
            }
        }
    }
    out
}

/// Whether `text` (a character or a cluster) should be drawn as a color
/// emoji: an emoji sequence, a character with VS16, or a character shown
/// as emoji by default. VS15 asks for the text form.
pub fn wants_color(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if text.contains('\u{fe0e}') {
        return false;
    }
    let sequence = text.chars().any(|c| {
        matches!(c,
            '\u{fe0f}' | '\u{200d}' | '\u{20e3}'
            | '\u{1f1e6}'..='\u{1f1ff}'
            | '\u{1f3fb}'..='\u{1f3ff}'
            | '\u{e0020}'..='\u{e007f}')
    });
    sequence || is_default_emoji(first)
}

/// Characters shown as emoji without VS16 (Emoji_Presentation),
/// approximated: the wide characters in the emoji and symbol blocks.
fn is_default_emoji(c: char) -> bool {
    use unicode_width::UnicodeWidthChar;
    let code = c as u32;
    c.width() == Some(2)
        && ((0x2300..=0x2bff).contains(&code) || (0x1f000..=0x1faff).contains(&code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_text_wants_color() {
        assert!(wants_color("😀"));
        assert!(wants_color("⌚"));
        assert!(wants_color("❤\u{fe0f}"));
        assert!(wants_color("👍\u{1f3fd}"));
        assert!(wants_color("🇻🇳"));
        assert!(wants_color("1\u{fe0f}\u{20e3}"));
        assert!(!wants_color("😀\u{fe0e}"));
        assert!(!wants_color("❤"));
        assert!(!wants_color("中"));
        assert!(!wants_color("a"));
    }

    #[test]
    fn resize_averages_areas() {
        // 2×1 → 1×1: the average of a red and a transparent pixel.
        let src = [255, 0, 0, 255, 0, 0, 0, 0];
        assert_eq!(resize(&src, (2, 1), (1, 1)), [128, 0, 0, 128]);
        // 1×1 → 2×2: the same pixel everywhere.
        assert_eq!(
            resize(&[1, 2, 3, 4], (1, 1), (2, 2)),
            [1, 2, 3, 4].repeat(4)
        );
    }

    /// Needs Noto Color Emoji (or another bitmap emoji font) installed;
    /// skipped where it isn't.
    #[test]
    fn draws_sequences_from_the_system_font() {
        let Some(font) = crate::font::load_emoji_font() else {
            return;
        };
        let flag = font.glyph_for("🇻🇳").unwrap();
        let letter = font.glyph_for("🇻").unwrap();
        assert_ne!(flag, letter, "the flag ligature applies");
        let (w, h, rgba) = font.rasterize(flag, 40, 20).unwrap();
        assert!(w <= 40 && h <= 20);
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert!(rgba.as_chunks::<4>().0.iter().any(|px| px[3] > 0));
    }
}
