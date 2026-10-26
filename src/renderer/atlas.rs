//! Shelf packing of glyph bitmaps into a single texture.

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

    pub fn clear(&mut self) {
        *self = Packer::new(self.width, self.height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    #[test]
    fn packs_without_overlap() {
        let mut packer = Packer::new(64, 64);
        let mut placed = Vec::new();
        for i in 0..40 {
            let (w, h) = (5 + i % 7, 8 + i % 5);
            let Some((x, y)) = packer.alloc(w, h) else { break };
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
