//! CPU rendering into shared-memory buffers (`wl_shm`). Draws the same
//! [`Batch`] as the GL backend, without loading a GPU driver.
//!
//! Only what changed is repainted. The window is split into horizontal
//! bands, one per text row plus the padding above and below, and each band
//! gets a fingerprint of everything drawn in it. A buffer remembers the
//! fingerprints of what it holds, so only differing bands are repainted,
//! and only bands that differ from the previous frame are damaged.

use std::hash::{DefaultHasher, Hash, Hasher};

use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_surface::WlSurface;

use super::atlas::Atlas;
use super::{Batch, Frame, Theme, VERTEX_FLOATS};
use crate::font::{CellMetrics, Fonts, Glyph};
use crate::terminal::graphics::Image;

/// Smaller than the GL atlas: saving memory is the point of this renderer.
const ATLAS_SIZE: u32 = 1024;
/// The color emoji atlas: room for a few hundred emoji at common sizes.
const COLOR_ATLAS_SIZE: u32 = 512;

pub struct Renderer {
    surface: WlSurface,
    pool: SlotPool,
    /// Buffers of the current size, with the band fingerprints of what each
    /// holds (empty: unknown). One is free unless the compositor still holds
    /// all of them.
    buffers: Vec<(Buffer, Vec<u64>)>,
    /// Band fingerprints of the last frame presented.
    presented: Vec<u64>,
    atlas: Atlas,
    /// The atlas image, one coverage byte per pixel.
    atlas_pixels: Vec<u8>,
    /// The color atlas, premultiplied BGRA like the buffers; allocated when
    /// the first emoji is drawn.
    color_pixels: Vec<u8>,
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
            presented: Vec::new(),
            atlas: Atlas::new(ATLAS_SIZE, COLOR_ATLAS_SIZE),
            atlas_pixels: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            color_pixels: Vec::new(),
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
            self.presented.clear();
        }
    }

    /// Replaces the fonts, after a config reload.
    pub fn set_fonts(&mut self, fonts: Fonts) {
        self.fonts = fonts;
        self.atlas.clear();
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
        let color_pixels = &mut self.color_pixels;
        let store = |x: u32, y: u32, glyph: &Glyph| {
            let w = glyph.width as usize;
            if glyph.color {
                if color_pixels.is_empty() {
                    color_pixels.resize((COLOR_ATLAS_SIZE * COLOR_ATLAS_SIZE * 4) as usize, 0);
                }
                for (row, src) in glyph.data.chunks_exact(w * 4).enumerate() {
                    let start = ((y as usize + row) * COLOR_ATLAS_SIZE as usize + x as usize) * 4;
                    let dst = &mut color_pixels[start..start + w * 4];
                    for (d, &[r, g, b, a]) in dst
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(src.as_chunks::<4>().0)
                    {
                        *d = [b, g, r, a];
                    }
                }
                return;
            }
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
        let background = premultiply(theme.background.to_f32(), theme.opacity);
        let bands = bands(frame.origin.1, metrics.height, frame.screen.rows(), h);
        let fingerprints = fingerprint(&batch, &bands, background, self.atlas.generation());
        let damage = changed(&bands, &fingerprints, &self.presented);
        if damage.is_empty() {
            // Nothing changed; commit anyway so the frame callback fires.
            self.surface.damage_buffer(0, 0, 1, 1);
            self.surface.commit();
            return Ok(self.atlas.take_overflowed());
        }

        let free = self
            .buffers
            .iter()
            .position(|(b, _)| b.canvas(&mut self.pool).is_some());
        let index = match free {
            Some(i) => i,
            None => {
                let (buffer, _) = self
                    .pool
                    .create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
                    .map_err(|e| format!("wl_shm buffer: {e}"))?;
                self.buffers.push((buffer, Vec::new()));
                self.buffers.len() - 1
            }
        };
        let (buffer, held) = &mut self.buffers[index];
        let canvas = buffer
            .canvas(&mut self.pool)
            .ok_or("wl_shm buffer is busy")?;
        let mut rows = vec![false; h as usize];
        for (y0, y1) in changed(&bands, &fingerprints, held) {
            rows[y0 as usize..y1 as usize].fill(true);
        }
        let images: Vec<Option<&Image>> = batch
            .images
            .iter()
            .map(|&(_, uid)| frame.screen.image(uid))
            .collect();
        let atlases = Atlases {
            coverage: &self.atlas_pixels,
            color: &self.color_pixels,
            images: &images,
        };
        paint(&batch, &atlases, canvas, (w, h), background, &rows);
        buffer
            .attach_to(&self.surface)
            .map_err(|e| format!("wl_shm attach: {e:?}"))?;
        for (y0, y1) in damage {
            self.surface
                .damage_buffer(0, y0 as i32, w as i32, (y1 - y0) as i32);
        }
        self.surface.commit();
        *held = fingerprints.clone();
        self.presented = fingerprints;
        Ok(self.atlas.take_overflowed())
    }
}

/// A color with `alpha` as ARGB8888 bytes in memory order (little-endian:
/// blue, green, red, alpha), premultiplied.
fn premultiply([r, g, b]: [f32; 3], alpha: f32) -> [u8; 4] {
    let byte = |v: f32| (v * alpha * 255.0).round() as u8;
    [byte(b), byte(g), byte(r), (alpha * 255.0).round() as u8]
}

/// The window's horizontal bands, as pixel rows `[start, end)`: the padding
/// above the grid, one band per text row, and the rest below.
fn bands(top: u32, cell_height: u32, rows: usize, height: u32) -> Vec<(u32, u32)> {
    let mut bands = vec![(0, top.min(height))];
    for row in 0..rows as u32 {
        let y = (top + row * cell_height).min(height);
        bands.push((y, (y + cell_height).min(height)));
    }
    let bottom = bands.last().expect("bands starts with the top margin").1;
    bands.push((bottom, height));
    bands
}

/// A fingerprint per band of everything drawn in it.
fn fingerprint(batch: &Batch, bands: &[(u32, u32)], clear: [u8; 4], atlas: u64) -> Vec<u64> {
    let mut hashers: Vec<DefaultHasher> = bands
        .iter()
        .map(|_| {
            let mut hasher = DefaultHasher::new();
            clear.hash(&mut hasher);
            // Quads only say where in the atlas their glyph is; after the
            // atlas is cleared, another glyph may be there.
            atlas.hash(&mut hasher);
            hasher
        })
        .collect();
    let mut images = batch.images.iter().peekable();
    for (i, quad) in batch
        .vertices
        .as_chunks::<{ 6 * VERTEX_FLOATS }>()
        .0
        .iter()
        .enumerate()
    {
        // The first vertex has the top-left corner, texture coordinate,
        // color and mode; the fifth the bottom-right corner and texture
        // coordinate.
        let first = &quad[..VERTEX_FLOATS];
        let last = &quad[4 * VERTEX_FLOATS..4 * VERTEX_FLOATS + 4];
        let image = images.next_if(|(v, _)| *v == i * 6).map(|&(_, uid)| uid);
        let (y0, y1) = (first[1].round() as i64, last[1].round() as i64);
        // The bands go down the window, so the first one this quad can touch
        // is found directly; there are thousands of quads per frame.
        let start = bands.partition_point(|band| i64::from(band.1) <= y0);
        for (band, hasher) in bands.iter().zip(&mut hashers).skip(start) {
            if i64::from(band.0) >= y1 {
                break;
            }
            for v in first.iter().chain(last) {
                v.to_bits().hash(hasher);
            }
            image.hash(hasher);
        }
    }
    hashers.iter().map(Hasher::finish).collect()
}

/// The bands whose fingerprint differs from `old` (all when `old` is
/// unknown), merged into contiguous pixel row ranges.
fn changed(bands: &[(u32, u32)], new: &[u64], old: &[u64]) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for (i, &(y0, y1)) in bands.iter().enumerate() {
        if y0 == y1 || (old.len() == new.len() && old[i] == new[i]) {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.1 == y0 => last.1 = y1,
            _ => ranges.push((y0, y1)),
        }
    }
    ranges
}

/// The atlas images: coverage (`ATLAS_SIZE`²) and color emoji
/// (`COLOR_ATLAS_SIZE`², BGRA; empty until needed), and the image of each
/// image quad of the batch.
struct Atlases<'pixels> {
    coverage: &'pixels [u8],
    color: &'pixels [u8],
    images: &'pixels [Option<&'pixels Image>],
}

/// Repaints the pixel rows of `canvas` (ARGB8888, `size` pixels, no row
/// padding) marked in `rows`: clears them to `clear` and draws the parts of
/// the quads of `batch` on them, sampling glyphs from `atlases`.
fn paint(
    batch: &Batch,
    atlases: &Atlases,
    canvas: &mut [u8],
    size: (u32, u32),
    clear: [u8; 4],
    rows: &[bool],
) {
    let width = size.0 as usize;
    for (pixels, _) in canvas
        .chunks_exact_mut(width * 4)
        .zip(rows)
        .filter(|(_, dirty)| **dirty)
    {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            *pixel = clear;
        }
    }
    let (width, height) = (size.0 as i64, size.1 as i64);
    let mut images = batch.images.iter().zip(atlases.images).peekable();
    for (i, quad) in batch
        .vertices
        .as_chunks::<{ 6 * VERTEX_FLOATS }>()
        .0
        .iter()
        .enumerate()
    {
        // Quads are axis-aligned: the first vertex is the top-left corner,
        // the fifth the bottom-right one.
        let first = &quad[..VERTEX_FLOATS];
        let last = &quad[4 * VERTEX_FLOATS..5 * VERTEX_FLOATS];
        let [x0, y0] = [first[0], first[1]].map(|v| v.round() as i64);
        let [x1, y1] = [last[0], last[1]].map(|v| v.round() as i64);
        if let Some((_, image)) = images.next_if(|((v, _), _)| *v == i * 6) {
            if let Some(image) = image {
                let uv = [first[2], first[3], last[2], last[3]];
                paint_image(image, uv, [x0, y0, x1, y1], canvas, size, rows);
            }
            continue;
        }
        let color = premultiply([first[4], first[5], first[6]], 1.0);
        let alpha = (first[7] * 255.0).round() as u32;
        let mode = first[8].round() as u8;
        if mode == 2 && atlases.color.is_empty() {
            continue;
        }
        // Glyphs are drawn 1:1, so a pixel maps to one atlas texel.
        let size = if mode == 2 {
            COLOR_ATLAS_SIZE
        } else {
            ATLAS_SIZE
        } as i64;
        let [u0, v0] = [first[2], first[3]].map(|t| (t * size as f32).round() as i64);

        for y in y0.max(0)..y1.min(height) {
            if !rows[y as usize] {
                continue;
            }
            let row = (y * width) as usize * 4;
            for x in x0.max(0)..x1.min(width) {
                let pixel = &mut canvas[row + x as usize * 4..][..4];
                let texel = ((v0 + y - y0) * size + u0 + x - x0) as usize;
                match mode {
                    0 => blend(pixel, color, alpha),
                    1 => blend(
                        pixel,
                        color,
                        alpha * u32::from(atlases.coverage[texel]) / 255,
                    ),
                    _ => blend_premultiplied(pixel, &atlases.color[texel * 4..][..4]),
                }
            }
        }
    }
}

/// Draws the part `uv` (normalized) of an image into the pixel rectangle
/// `[x0, y0, x1, y1]` of the rows marked in `rows`, sampling the nearest
/// pixel.
fn paint_image(
    image: &Image,
    uv: [f32; 4],
    [x0, y0, x1, y1]: [i64; 4],
    canvas: &mut [u8],
    size: (u32, u32),
    rows: &[bool],
) {
    let (width, height) = (size.0 as i64, size.1 as i64);
    let (iw, ih) = (image.width as f32, image.height as f32);
    let [u0, v0, u1, v1] = uv;
    let (dw, dh) = ((x1 - x0).max(1) as f32, (y1 - y0).max(1) as f32);
    for y in y0.max(0)..y1.min(height) {
        if !rows[y as usize] {
            continue;
        }
        let v = v0 + ((y - y0) as f32 + 0.5) / dh * (v1 - v0);
        let ty = ((v * ih) as i64).clamp(0, ih as i64 - 1) as usize;
        let row = (y * width) as usize * 4;
        for x in x0.max(0)..x1.min(width) {
            let u = u0 + ((x - x0) as f32 + 0.5) / dw * (u1 - u0);
            let tx = ((u * iw) as i64).clamp(0, iw as i64 - 1) as usize;
            let i = (ty * image.width as usize + tx) * 4;
            let Some([r, g, b, a]) = image
                .pixels
                .get(i..i + 4)
                .and_then(|p| <[u8; 4]>::try_from(p).ok())
            else {
                continue;
            };
            let a32 = u32::from(a);
            let pre = |c: u8| ((u32::from(c) * a32 + 127) / 255) as u8;
            let pixel = &mut canvas[row + x as usize * 4..][..4];
            blend_premultiplied(pixel, &[pre(b), pre(g), pre(r), a]);
        }
    }
}

/// Draws a premultiplied color over a premultiplied pixel.
fn blend_premultiplied(pixel: &mut [u8], src: &[u8]) {
    let keep = 255 - u32::from(src[3]);
    for (dst, &s) in pixel.iter_mut().zip(src) {
        *dst = (u32::from(s) + (u32::from(*dst) * keep + 127) / 255).min(255) as u8;
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

    fn atlases(coverage: &[u8]) -> Atlases<'_> {
        Atlases {
            coverage,
            color: &[],
            images: &[],
        }
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
        paint(
            &batch,
            &atlases(&atlas_with(&[])),
            &mut canvas,
            (4, 3),
            CLEAR,
            &[true; 3],
        );
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
        paint(
            &batch,
            &atlases(&atlas_with(&[])),
            &mut canvas,
            (1, 1),
            [0; 4],
            &[true],
        );
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
            color: false,
        };
        let mut batch = Batch::default();
        batch.glyph(0.0, 0.0, &glyph, Rgb(255, 255, 255));
        let mut canvas = vec![0; 4 * 3];
        paint(
            &batch,
            &atlases(&atlas),
            &mut canvas,
            (3, 1),
            [0, 0, 0, 255],
            &[true],
        );
        assert_eq!(pixel(&canvas, 3, 0, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 3, 1, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 2, 0), [128, 128, 128, 255]);
    }

    #[test]
    fn repaints_only_marked_rows() {
        let mut batch = Batch::default();
        batch.rect(0.0, 0.0, 1.0, 3.0, Rgb(255, 0, 0));
        let mut canvas = vec![7; 4 * 3];
        paint(
            &batch,
            &atlases(&atlas_with(&[])),
            &mut canvas,
            (1, 3),
            CLEAR,
            &[false, true, false],
        );
        assert_eq!(pixel(&canvas, 1, 0, 0), [7; 4]);
        assert_eq!(pixel(&canvas, 1, 0, 1), [0, 0, 255, 255]);
        assert_eq!(pixel(&canvas, 1, 0, 2), [7; 4]);
    }

    #[test]
    fn bands_cover_padding_and_rows() {
        assert_eq!(bands(4, 10, 2, 30), [(0, 4), (4, 14), (14, 24), (24, 30)]);
        // Clipped to the window.
        assert_eq!(
            bands(4, 10, 3, 30),
            [(0, 4), (4, 14), (14, 24), (24, 30), (30, 30)]
        );
    }

    #[test]
    fn only_changed_bands_are_reported() {
        let bands = bands(0, 10, 3, 30);
        let mut batch = Batch::default();
        batch.rect(0.0, 10.0, 5.0, 10.0, Rgb(1, 2, 3));
        let before = fingerprint(&batch, &bands, CLEAR, 0);
        // Unknown contents: everything.
        assert_eq!(changed(&bands, &before, &[]), [(0, 30)]);
        assert!(changed(&bands, &before, &before).is_empty());
        // A glyph overhanging from the second row into the third.
        batch.rect(0.0, 18.0, 1.0, 4.0, Rgb(9, 9, 9));
        let after = fingerprint(&batch, &bands, CLEAR, 0);
        assert_eq!(changed(&bands, &after, &before), [(10, 30)]);
        // A new background color changes every band.
        let other = fingerprint(&batch, &bands, [0; 4], 0);
        assert_eq!(changed(&bands, &other, &after), [(0, 30)]);
        // So does clearing the atlas: another glyph may be at the same place.
        let cleared = fingerprint(&batch, &bands, CLEAR, 1);
        assert_eq!(changed(&bands, &cleared, &after), [(0, 30)]);
    }

    #[test]
    fn color_glyphs_blend_premultiplied() {
        let mut color = vec![0; (COLOR_ATLAS_SIZE * COLOR_ATLAS_SIZE * 4) as usize];
        // Half-transparent blue (premultiplied BGRA), then opaque red.
        color[..8].copy_from_slice(&[128, 0, 0, 128, 0, 0, 255, 255]);
        let size = COLOR_ATLAS_SIZE as f32;
        let glyph = GlyphInfo {
            uv: [0.0, 0.0, 2.0 / size, 1.0 / size],
            width: 2,
            height: 1,
            left: 0,
            top: 0,
            color: true,
        };
        let mut batch = Batch::default();
        batch.glyph(0.0, 0.0, &glyph, Rgb(0, 255, 0));
        let mut canvas = vec![0; 8];
        let atlases = Atlases {
            coverage: &[],
            color: &color,
            images: &[],
        };
        paint(
            &batch,
            &atlases,
            &mut canvas,
            (2, 1),
            [0, 0, 100, 255],
            &[true],
        );
        assert_eq!(pixel(&canvas, 2, 0, 0), [128, 0, 50, 255]);
        assert_eq!(pixel(&canvas, 2, 1, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn images_are_scaled_and_blended() {
        // Opaque red and half-transparent white, 2×1, drawn 4 pixels wide.
        let image = Image {
            uid: 1,
            id: 0,
            number: 0,
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 255, 255, 255, 128],
        };
        let mut batch = Batch::default();
        batch.image([0.0, 0.0, 4.0, 1.0], [0.0, 0.0, 1.0, 1.0], 1);
        let images = [Some(&image)];
        let atlases = Atlases {
            coverage: &[],
            color: &[],
            images: &images,
        };
        let mut canvas = vec![0; 4 * 4];
        paint(
            &batch,
            &atlases,
            &mut canvas,
            (4, 1),
            [0, 0, 0, 255],
            &[true],
        );
        assert_eq!(pixel(&canvas, 4, 1, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&canvas, 4, 2, 0), [128, 128, 128, 255]);
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
            color: false,
        };
        let mut batch = Batch::default();
        batch.glyph(0.0, 0.0, &glyph, Rgb(255, 255, 255));
        let mut canvas = vec![0; 4];
        paint(
            &batch,
            &atlases(&atlas),
            &mut canvas,
            (1, 1),
            [0, 0, 0, 255],
            &[true],
        );
        assert_eq!(pixel(&canvas, 1, 0, 0), [255, 255, 255, 255]);
    }
}
