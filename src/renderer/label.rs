//! A line of text drawn on its own, outside the terminal grid: the icon
//! that follows the pointer while a file is dragged out.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::color::Rgb;
use crate::font::{Fonts, Glyph, Style};

/// An image of a line of text, in premultiplied BGRA (`wl_shm`'s
/// ARGB8888 on little-endian), `width` × `height` pixels.
pub struct Label {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Draws `text` in `fonts`, `foreground` on an opaque `background` with a
/// border of the foreground color and `padding` pixels around the text.
/// Each side comes out a multiple of `multiple` pixels (for an integer
/// buffer scale).
pub fn draw(
    fonts: &mut Fonts,
    text: &str,
    foreground: Rgb,
    background: Rgb,
    padding: u32,
    multiple: u32,
) -> Label {
    let m = fonts.metrics;
    let clusters: Vec<(&str, u32)> = text
        .graphemes(true)
        .map(|g| (g, g.width().clamp(1, 2) as u32))
        .collect();
    let cells: u32 = clusters.iter().map(|&(_, w)| w).sum();
    let round_up = |v: u32| v.div_ceil(multiple.max(1)) * multiple.max(1);
    let width = round_up(cells * m.width + 2 * padding);
    let height = round_up(m.height + 2 * padding);
    let mut label = Label {
        width,
        height,
        pixels: [background.b, background.g, background.r, 0xff].repeat((width * height) as usize),
    };
    let border = m.stroke;
    for y in 0..height {
        for x in 0..width {
            if x < border || y < border || x >= width - border || y >= height - border {
                label.blend(x as i32, y as i32, foreground, 0xff);
            }
        }
    }
    // Centered: rounding up may have added room.
    let mut x = ((width - cells * m.width) / 2) as i32;
    let top = ((height - m.height) / 2) as i32;
    for (cluster, cells) in clusters {
        let wide = cells == 2;
        let mut chars = cluster.chars();
        let glyph = match (chars.next(), chars.next()) {
            (Some(c), None) => fonts.rasterize(c, Style::Regular, wide),
            _ => fonts.rasterize_cluster(cluster, Style::Regular, wide),
        };
        if let Some(glyph) = glyph {
            label.glyph(x + glyph.left, top + glyph.top, &glyph, foreground);
        }
        x += (cells * m.width) as i32;
    }
    label
}

impl Label {
    /// Draws `glyph` with its top-left corner at `(x, y)`, clipped.
    fn glyph(&mut self, x: i32, y: i32, glyph: &Glyph, color: Rgb) {
        for gy in 0..glyph.height as i32 {
            for gx in 0..glyph.width as i32 {
                let i = (gy * glyph.width as i32 + gx) as usize;
                if glyph.color {
                    let p = &glyph.data[i * 4..i * 4 + 4];
                    self.over(x + gx, y + gy, [p[2], p[1], p[0], p[3]]);
                } else {
                    self.blend(x + gx, y + gy, color, glyph.data[i]);
                }
            }
        }
    }

    /// Mixes `color` into the pixel at `(x, y)` by `coverage`.
    fn blend(&mut self, x: i32, y: i32, color: Rgb, coverage: u8) {
        let a = u32::from(coverage);
        let premultiplied = |c: u8| (u32::from(c) * a / 255) as u8;
        self.over(
            x,
            y,
            [
                premultiplied(color.b),
                premultiplied(color.g),
                premultiplied(color.r),
                coverage,
            ],
        );
    }

    /// Composites the premultiplied BGRA pixel `src` over the pixel at
    /// `(x, y)`, if it is inside the label.
    fn over(&mut self, x: i32, y: i32, src: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        let keep = 255 - u32::from(src[3]);
        for (dst, src) in self.pixels[i..i + 4].iter_mut().zip(src) {
            *dst = (u32::from(src) + u32::from(*dst) * keep / 255).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(width: u32, height: u32) -> Label {
        Label {
            width,
            height,
            pixels: [10, 20, 30, 0xff].repeat((width * height) as usize),
        }
    }

    #[test]
    fn blending_mixes_by_coverage_and_clips() {
        let mut label = blank(2, 1);
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        label.blend(0, 0, white, 0xff);
        label.blend(1, 0, white, 0);
        label.blend(-1, 0, white, 0xff);
        label.blend(2, 0, white, 0xff);
        assert_eq!(label.pixels, [255, 255, 255, 255, 10, 20, 30, 255]);
    }
}
