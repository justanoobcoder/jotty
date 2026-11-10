//! The glyph atlas: glyph bitmaps packed into one single-channel image,
//! rasterized on demand while a frame is built. Where the pixels live (a GL
//! texture or plain memory) is up to the renderer.

use std::collections::HashMap;

use super::{GlyphCache, GlyphInfo};
use crate::font::{self, Fonts, Glyph, Style};
use crate::terminal::Underline;

/// Packs rectangles left to right into rows ("shelves") as tall as their
/// tallest member. Good enough for glyphs, which are of similar height.
#[derive(Debug)]
pub struct Packer {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    shelf_height: u32,
}

/// Gap between glyphs so linear filtering never samples a neighbor.
const PADDING: u32 = 1;

impl Packer {
    pub fn new(width: u32, height: u32) -> Self {
        Packer {
            width,
            height,
            x: 0,
            y: 0,
            shelf_height: 0,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reserves a `w`×`h` area and returns its top-left corner, or `None`
    /// when the atlas is full.
    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.width || h > self.height {
            return None;
        }
        if self.x + w > self.width {
            self.y += self.shelf_height + PADDING;
            self.x = 0;
            self.shelf_height = 0;
        }
        if self.y + h > self.height {
            return None;
        }
        let pos = (self.x, self.y);
        self.x += w + PADDING;
        self.shelf_height = self.shelf_height.max(h);
        Some(pos)
    }

    /// Frees the whole area, for repacking from scratch.
    pub fn clear(&mut self) {
        *self = Packer::new(self.width, self.height);
    }
}

/// What a cached atlas entry shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GlyphKey {
    /// A character in a style, for a one- or two-cell slot.
    Char(char, Style, bool),
    /// A grapheme cluster, by a hash of its text, in a style and slot.
    Cluster(u64, Style, bool),
    /// An underline spanning a number of cells.
    Underline(Underline, u32),
}

/// Placement of glyphs in the atlas images, and which glyphs they hold.
/// There are two images: coverage for text, and RGBA for color emoji.
#[derive(Debug)]
pub struct Atlas {
    packer: Packer,
    color_packer: Packer,
    glyphs: HashMap<GlyphKey, Option<GlyphInfo>>,
    /// The atlas filled up and was cleared since the last `take_overflowed`.
    overflowed: bool,
    /// This frame is the redraw after an overflow. If it fills the atlas too,
    /// the screen needs more than it holds, and clearing again would only
    /// repeat forever: the glyphs that don't fit go undrawn instead.
    retrying: bool,
    /// Changes whenever bitmaps at known positions may have changed.
    generation: u64,
}

impl Atlas {
    /// Square atlas images of `size` (coverage) and `color_size` (RGBA)
    /// pixels.
    pub fn new(size: u32, color_size: u32) -> Self {
        Atlas {
            packer: Packer::new(size, size),
            color_packer: Packer::new(color_size, color_size),
            glyphs: HashMap::new(),
            overflowed: false,
            retrying: false,
            generation: 0,
        }
    }

    fn packer(&mut self, color: bool) -> &mut Packer {
        if color {
            &mut self.color_packer
        } else {
            &mut self.packer
        }
    }

    /// Forgets all glyphs, after a font size change.
    pub fn clear(&mut self) {
        self.packer.clear();
        self.color_packer.clear();
        self.glyphs.clear();
        self.generation += 1;
    }

    /// Changes when the atlas is cleared, so that drawings that look the
    /// same by glyph position can be told apart.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the atlas overflowed since the last call. Glyphs placed
    /// before that are stale, so the frame must be drawn again, once.
    pub fn take_overflowed(&mut self) -> bool {
        self.retrying = std::mem::take(&mut self.overflowed);
        self.retrying
    }

    /// Finds room for a glyph, clearing the atlas if it is full, unless that
    /// just happened.
    fn allocate(&mut self, w: u32, h: u32, color: bool) -> Option<(u32, u32)> {
        if let Some(pos) = self.packer(color).alloc(w, h) {
            return Some(pos);
        }
        if self.retrying {
            log::debug!("the glyph atlas can't hold this screen; dropping a glyph");
            return None;
        }
        // Start over; glyphs placed earlier this frame are stale, so the
        // caller redraws once more.
        self.clear();
        self.overflowed = true;
        self.packer(color).alloc(w, h)
    }

    /// A [`GlyphCache`] that rasterizes missing glyphs with `fonts` and
    /// hands each new bitmap to `store` with its position in its atlas
    /// image (the color one when `glyph.color`).
    pub fn cache<'cache, S: FnMut(u32, u32, &Glyph)>(
        &'cache mut self,
        fonts: &'cache mut Fonts,
        store: S,
    ) -> Cache<'cache, S> {
        Cache {
            atlas: self,
            fonts,
            store,
        }
    }
}

/// A [`GlyphCache`] over an atlas and the fonts, which calls `store` to
/// upload each newly placed glyph.
pub struct Cache<'cache, S> {
    atlas: &'cache mut Atlas,
    fonts: &'cache mut Fonts,
    store: S,
}

impl<S: FnMut(u32, u32, &Glyph)> Cache<'_, S> {
    /// Returns the cached entry for `key`, or creates it with `make`.
    fn cached(
        &mut self,
        key: GlyphKey,
        make: impl FnOnce(&mut Fonts) -> Option<Glyph>,
    ) -> Option<GlyphInfo> {
        if let Some(info) = self.atlas.glyphs.get(&key) {
            return *info;
        }
        let info = make(self.fonts).and_then(|glyph| self.place(&glyph));
        // A glyph that only lacks room now may fit in a later frame.
        if info.is_some() || !self.atlas.retrying {
            self.atlas.glyphs.insert(key, info);
        }
        info
    }

    fn place(&mut self, glyph: &Glyph) -> Option<GlyphInfo> {
        let atlas = &mut *self.atlas;
        let (x, y) = atlas.allocate(glyph.width, glyph.height, glyph.color)?;
        (self.store)(x, y, glyph);
        let size = atlas.packer(glyph.color).size().0 as f32;
        Some(GlyphInfo {
            uv: [
                x as f32 / size,
                y as f32 / size,
                (x + glyph.width) as f32 / size,
                (y + glyph.height) as f32 / size,
            ],
            width: glyph.width,
            height: glyph.height,
            left: glyph.left,
            top: glyph.top,
            color: glyph.color,
        })
    }
}

impl<S: FnMut(u32, u32, &Glyph)> GlyphCache for Cache<'_, S> {
    fn glyph(&mut self, c: char, style: Style, wide: bool) -> Option<GlyphInfo> {
        self.cached(GlyphKey::Char(c, style, wide), |fonts| {
            fonts.rasterize(c, style, wide)
        })
    }

    fn cluster(&mut self, text: &str, style: Style, wide: bool) -> Option<GlyphInfo> {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        let hash = BuildHasherDefault::<DefaultHasher>::default().hash_one(text);
        self.cached(GlyphKey::Cluster(hash, style, wide), |fonts| {
            fonts.rasterize_cluster(text, style, wide)
        })
    }

    fn underline(&mut self, style: Underline, cells: u32) -> Option<GlyphInfo> {
        self.cached(GlyphKey::Underline(style, cells), |fonts| {
            font::underline_glyph(style, fonts.metrics, cells)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    #[test]
    fn a_screen_too_big_for_the_atlas_redraws_only_once() {
        let mut atlas = Atlas::new(10, 10);
        assert!(atlas.allocate(10, 10, /* color= */ false).is_some());
        let generation = atlas.generation();
        // The first frame overflows and asks for one more.
        assert!(atlas.allocate(10, 10, /* color= */ false).is_some());
        assert_ne!(atlas.generation(), generation);
        assert!(atlas.take_overflowed());
        // That one doesn't fit either: it drops the glyph and is the last.
        assert_eq!(atlas.allocate(10, 10, /* color= */ false), None);
        assert!(!atlas.take_overflowed());
        // The next frame can start over.
        assert!(atlas.allocate(10, 10, /* color= */ false).is_some());
        assert!(atlas.take_overflowed());
    }

    #[test]
    fn packs_without_overlap() {
        let mut packer = Packer::new(64, 64);
        let mut placed = Vec::new();
        for i in 0..40 {
            let (w, h) = (5 + i % 7, 8 + i % 5);
            let Some((x, y)) = packer.alloc(w, h) else {
                break;
            };
            assert!(x + w <= 64 && y + h <= 64);
            let rect = (x, y, w, h);
            assert!(placed.iter().all(|&r| !overlaps(r, rect)));
            placed.push(rect);
        }
        assert!(placed.len() > 20);
    }

    #[test]
    fn starts_new_shelf_when_row_is_full() {
        let mut packer = Packer::new(10, 100);
        assert_eq!(packer.alloc(6, 5), Some((0, 0)));
        assert_eq!(packer.alloc(6, 7), Some((0, 6)));
        assert_eq!(packer.alloc(3, 2), Some((7, 6)));
    }

    #[test]
    fn reports_full_and_clears() {
        let mut packer = Packer::new(10, 10);
        assert_eq!(packer.alloc(11, 1), None);
        assert!(packer.alloc(10, 10).is_some());
        assert_eq!(packer.alloc(1, 1), None);
        packer.clear();
        assert_eq!(packer.alloc(1, 1), Some((0, 0)));
    }
}
