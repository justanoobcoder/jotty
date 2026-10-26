//! CPU rendering into shared-memory buffers (`wl_shm`). Draws the same
//! [`Batch`] as the GL backend, without loading a GPU driver.

use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_surface::WlSurface;

use super::atlas::Atlas;
use super::{Batch, Frame, Theme, VERTEX_FLOATS};
use crate::font::{CellMetrics, Fonts, Glyph};

/// Smaller than the GL atlas: saving memory is the point of this renderer.
const ATLAS_SIZE: u32 = 1024;

pub struct Renderer {
    surface: WlSurface,
    pool: SlotPool,
    /// Buffers of the current size; one is free unless the compositor
    /// still holds all of them.
    buffers: Vec<Buffer>,
    atlas: Atlas,
    /// The atlas image, one coverage byte per pixel.
    atlas_pixels: Vec<u8>,
    pub fonts: Fonts,
    pub theme: Theme,
    size: (u32, u32),
}

impl Renderer {
    pub fn new(
        shm: &Shm,
        surface: WlSurface,
        size: (u32, u32),
        fonts: Fonts,
        theme: Theme,
    ) -> Result<Renderer, String> {
        let len = (size.0.max(1) * size.1.max(1) * 4) as usize;
        let pool = SlotPool::new(len, shm).map_err(|e| format!("wl_shm pool: {e}"))?;
        log::info!("rendering on the CPU");
        Ok(Renderer {
            surface,
            pool,
            buffers: Vec::new(),
            atlas: Atlas::new(ATLAS_SIZE),
            atlas_pixels: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            fonts,
            theme,
            size,
        })
    }

    pub fn cell(&self) -> CellMetrics {
        self.fonts.metrics
    }

    /// Resizes the drawable, in physical pixels.
    pub fn resize(&mut self, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            self.buffers.clear();
        }
    }

    pub fn set_font_size(&mut self, px: f32) {
        self.fonts.set_size(px);
        self.atlas.clear();
    }

    /// Draws and presents a frame (this commits the surface). Returns
    /// whether another frame is needed because the glyph atlas overflowed.
    pub fn draw(&mut self, frame: &Frame) -> Result<bool, String> {
        let metrics = self.fonts.metrics;
        let theme = self.theme.effective(frame.screen);
        let pixels = &mut self.atlas_pixels;
        let store = |x: u32, y: u32, glyph: &Glyph| {
            let w = glyph.width as usize;
            for (row, src) in glyph.data.chunks_exact(w).enumerate() {
                let start = (y as usize + row) * ATLAS_SIZE as usize + x as usize;
                pixels[start..start + w].copy_from_slice(src);
            }
        };
        let batch = super::build(
            frame,
            &theme,
            metrics,
            &mut self.atlas.cache(&mut self.fonts, store),
        );

        let (w, h) = (self.size.0.max(1), self.size.1.max(1));
        let free = self
            .buffers
            .iter()
            .position(|b| b.canvas(&mut self.pool).is_some());
        let index = match free {
            Some(i) => i,
            None => {
                let (buffer, _) = self
                    .pool
                    .create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
                    .map_err(|e| format!("wl_shm buffer: {e}"))?;
                self.buffers.push(buffer);
                self.buffers.len() - 1
            }
        };
        let buffer = &self.buffers[index];
        let canvas = buffer
            .canvas(&mut self.pool)
            .ok_or("wl_shm buffer is busy")?;
        let background = premultiply(theme.background.to_f32(), theme.opacity);
        paint(&batch, &self.atlas_pixels, canvas, (w, h), background);
        buffer
            .attach_to(&self.surface)
            .map_err(|e| format!("wl_shm attach: {e:?}"))?;
        self.surface.damage_buffer(0, 0, w as i32, h as i32);
        self.surface.commit();
        Ok(self.atlas.take_overflowed())
    }
}

/// A color with `alpha` as ARGB8888 bytes in memory order (little-endian:
/// blue, green, red, alpha), premultiplied.
fn premultiply([r, g, b]: [f32; 3], alpha: f32) -> [u8; 4] {
    let byte = |v: f32| (v * alpha * 255.0).round() as u8;
    [byte(b), byte(g), byte(r), (alpha * 255.0).round() as u8]
}

/// Clears `canvas` (ARGB8888, `size` pixels, no row padding) to `clear` and
/// draws the quads of `batch` over it, sampling glyphs from `atlas`.
fn paint(batch: &Batch, atlas: &[u8], canvas: &mut [u8], size: (u32, u32), clear: [u8; 4]) {
    for pixel in canvas.as_chunks_mut::<4>().0 {
        *pixel = clear;
    }
    let (width, height) = (size.0 as i64, size.1 as i64);
    for quad in batch.vertices.as_chunks::<{ 6 * VERTEX_FLOATS }>().0 {
        // Quads are axis-aligned: the first vertex is the top-left corner,
        // the fifth the bottom-right one.
        let first = &quad[..VERTEX_FLOATS];
        let last = &quad[4 * VERTEX_FLOATS..5 * VERTEX_FLOATS];
        let [x0, y0] = [first[0], first[1]].map(|v| v.round() as i64);
        let [x1, y1] = [last[0], last[1]].map(|v| v.round() as i64);
        let color = premultiply([first[4], first[5], first[6]], 1.0);
        let alpha = (first[7] * 255.0).round() as u32;
        let glyph = first[8] > 0.5;
        // Glyphs are drawn 1:1, so a pixel maps to one atlas texel.
        let [u0, v0] = [first[2], first[3]].map(|t| (t * ATLAS_SIZE as f32).round() as i64);

        for y in y0.max(0)..y1.min(height) {
            let row = (y * width) as usize * 4;
            for x in x0.max(0)..x1.min(width) {
                let a = if glyph {
                    let texel = (v0 + y - y0) * ATLAS_SIZE as i64 + u0 + x - x0;
                    alpha * u32::from(atlas[texel as usize]) / 255
                } else {
                    alpha
                };
                blend(&mut canvas[row + x as usize * 4..][..4], color, a);
            }
        }
    }
}

/// Draws opaque `color` with coverage `a` (0–255) over a premultiplied pixel.
fn blend(pixel: &mut [u8], color: [u8; 4], a: u32) {
    match a {
        0 => {}
        255 => pixel.copy_from_slice(&color),
        _ => {
            let keep = 255 - a;
            for (dst, src) in pixel.iter_mut().zip(color) {
                *dst = ((u32::from(src) * a + u32::from(*dst) * keep + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Rgb;
    use crate::renderer::GlyphInfo;

    const CLEAR: [u8; 4] = [0x18, 0x18, 0x18, 0xff];

    fn pixel(canvas: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        canvas[i..i + 4].try_into().unwrap()
    }

    fn atlas_with(texels: &[(u32, u32, u8)]) -> Vec<u8> {
        let mut atlas = vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize];
        for &(x, y, v) in texels {
            atlas[(y * ATLAS_SIZE + x) as usize] = v;
        }
        atlas
    }

    #[test]
    fn premultiplies_in_memory_order() {
        assert_eq!(premultiply([1.0, 0.5, 0.0], 1.0), [0, 128, 255, 255]);
        assert_eq!(premultiply([1.0, 1.0, 1.0], 0.5), [128, 128, 128, 128]);
    }

    #[test]
    fn fills_clipped_rectangles() {
        let mut batch = Batch::default();
        batch.rect(2.0, 1.0, 10.0, 1.0, Rgb(255, 0, 0));
        let mut canvas = vec![0; 4 * 4 * 3];
        paint(&batch, &atlas_with(&[]), &mut canvas, (4, 3), CLEAR);
        assert_eq!(pixel(&canvas, 4, 1, 1), CLEAR);
        assert_eq!(pixel(&canvas, 4, 2, 1), [0, 0, 255, 255]);
        assert_eq!(pixel(&canvas, 4, 3, 1), [0, 0, 255, 255]);
        assert_eq!(pixel(&canvas, 4, 2, 2), CLEAR);
    }

    #[test]
    fn translucent_fill_blends() {
        let mut batch = Batch::default();
        batch.fill(0.0, 0.0, 1.0, 1.0, Rgb(255, 255, 255), 0.5);
        let mut canvas = vec![0; 4];
        paint(&batch, &atlas_with(&[]), &mut canvas, (1, 1), [0; 4]);
        assert_eq!(pixel(&canvas, 1, 0, 0), [128, 128, 128, 128]);
    }

    #[test]
    fn glyphs_use_atlas_coverage() {
        // A 2×1 glyph at atlas position (4, 2): full, then half coverage.
        let atlas = atlas_with(&[(4, 2, 255), (5, 2, 128)]);
        let size = ATLAS_SIZE as f32;
        let glyph = GlyphInfo {
            uv: [4.0 / size, 2.0 / size, 6.0 / size, 3.0 / size],
            width: 2,
            height: 1,
            left: 1,
            top: 0,
        };
        let mut batch = Batch::default();
        batch.glyph(0.0, 0.0, &glyph, Rgb(255, 255, 255));
        let mut canvas = vec![0; 4 * 3];
        paint(&batch, &atlas, &mut canvas, (3, 1), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 3, 0, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 3, 1, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 2, 0), [128, 128, 128, 255]);
    }

    #[test]
    fn glyphs_partly_off_canvas_are_clipped() {
        let atlas = atlas_with(&[(0, 0, 255), (1, 0, 255)]);
        let size = ATLAS_SIZE as f32;
        let glyph = GlyphInfo {
            uv: [0.0, 0.0, 2.0 / size, 1.0 / size],
            width: 2,
            height: 1,
            left: -1,
            top: 0,
        };
        let mut batch = Batch::default();
        batch.glyph(0.0, 0.0, &glyph, Rgb(255, 255, 255));
        let mut canvas = vec![0; 4];
        paint(&batch, &atlas, &mut canvas, (1, 1), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 0, 0), [255, 255, 255, 255]);
    }
}
