//! Sixel graphics: images that programs send in `DCS P1 ; P2 ; P3 q … ST`.
//!
//! A [`Decoder`] takes the bytes of the DCS string one at a time, as vte
//! passes them, and paints an RGBA canvas that grows as the image does.
//! Each sixel is a column of six pixels; `$` returns to the left edge and
//! `-` moves down six pixels. Color registers are private to each image.

/// The largest image accepted, in pixels per side; pixels past it are
/// dropped. It bounds the canvas to 64 MiB.
pub const MAX_SIDE: usize = 4096;
/// The pixels an image may allocate and paint per byte received, so that a
/// few bytes can't cost a full canvas. A solid 4096×4096 image, the most
/// compressed real one, needs about 7000.
const PIXELS_PER_BYTE: usize = 16_384;
/// How many color registers an image has.
pub const REGISTERS: usize = 256;

/// The VT340's default colors for the first 16 registers, in percent.
const VT340: [[u32; 3]; 16] = [
    [0, 0, 0],
    [20, 20, 80],
    [80, 13, 13],
    [20, 80, 20],
    [80, 20, 80],
    [20, 80, 80],
    [80, 80, 20],
    [53, 53, 53],
    [26, 26, 26],
    [33, 33, 60],
    [60, 26, 26],
    [33, 60, 33],
    [60, 33, 60],
    [33, 60, 60],
    [60, 60, 33],
    [80, 80, 80],
];

/// The control function whose numeric parameters are being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// `" Pan ; Pad ; Ph ; Pv`.
    Raster,
    /// `# Pc` or `# Pc ; Pu ; Px ; Py ; Pz`.
    Color,
    /// `! Pn`.
    Repeat,
}

/// A decoded image: its size and RGBA pixels, not premultiplied.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decodes one sixel image as its bytes arrive.
pub struct Decoder {
    /// RGBA, `stride` pixels wide and `pixels.len() / 4 / stride` high.
    pixels: Vec<u8>,
    stride: usize,
    palette: [[u8; 4]; REGISTERS],
    color: [u8; 4],
    /// Where the next sixel goes: x, and the top row of the current band.
    x: usize,
    y: usize,
    /// How many times the next sixel is drawn (`!`).
    repeat: usize,
    /// The size the raster attributes declared.
    declared: (usize, usize),
    /// The extent drawn so far: past the rightmost sixel and the lowest
    /// pixel set.
    extent: (usize, usize),
    pending: Pending,
    params: [u32; 5],
    count: usize,
    /// The pixels left to allocate and paint (see `PIXELS_PER_BYTE`).
    budget: usize,
    /// Pixels left unset stay transparent (`P2 = 1`) instead of taking this
    /// background color.
    background: Option<[u8; 4]>,
}

impl Decoder {
    /// Starts an image. `transparent` is `P2 = 1`; otherwise pixels left
    /// unset are filled with `background`.
    #[must_use]
    pub fn new(transparent: bool, background: [u8; 3]) -> Self {
        let mut palette = [[0, 0, 0, 255]; REGISTERS];
        for (entry, rgb) in palette.iter_mut().zip(VT340) {
            *entry = rgba_from_percent(rgb);
        }
        let [r, g, b] = background;
        Decoder {
            pixels: Vec::new(),
            stride: 0,
            palette,
            color: palette[0],
            x: 0,
            y: 0,
            repeat: 1,
            declared: (0, 0),
            extent: (0, 0),
            pending: Pending::None,
            params: [0; 5],
            count: 0,
            budget: 0,
            background: (!transparent).then_some([r, g, b, 255]),
        }
    }

    /// Takes the next byte of the DCS string.
    pub fn put(&mut self, byte: u8) {
        self.budget = self.budget.saturating_add(PIXELS_PER_BYTE);
        if self.pending != Pending::None {
            match byte {
                b'0'..=b'9' => {
                    // Parameters past the last one are ignored.
                    if let Some(param) = self.params.get_mut(self.count) {
                        // Capped well past any meaningful value, so it can't overflow.
                        *param = (*param * 10 + u32::from(byte - b'0')).min(1 << 20);
                    }
                    return;
                }
                b';' => {
                    self.count = (self.count + 1).min(self.params.len());
                    return;
                }
                _ => self.finish_pending(),
            }
        }
        match byte {
            b'?'..=b'~' => {
                let repeat = std::mem::replace(&mut self.repeat, 1);
                self.sixel(byte - b'?', repeat);
            }
            b'$' => {
                self.x = 0;
                self.repeat = 1;
            }
            b'-' => {
                self.x = 0;
                self.y += 6;
                self.repeat = 1;
            }
            b'"' => self.begin(Pending::Raster),
            b'#' => self.begin(Pending::Color),
            b'!' => self.begin(Pending::Repeat),
            // Line breaks and anything else between commands mean nothing.
            _ => {}
        }
    }

    /// Ends the image, or `None` when nothing was drawn.
    #[must_use]
    pub fn finish(mut self) -> Option<Image> {
        self.finish_pending();
        // The declared size counts only if the budget pays for it.
        let (mut width, mut height) = (
            self.extent.0.max(self.declared.0),
            self.extent.1.max(self.declared.1),
        );
        if !self.reserve(width, height) {
            (width, height) = self.extent;
        }
        if width == 0 || height == 0 {
            return None;
        }
        let mut pixels = if self.stride == width {
            self.pixels.truncate(width * height * 4);
            self.pixels
        } else {
            let mut pixels = Vec::with_capacity(width * height * 4);
            for row in self.pixels.chunks_exact(self.stride * 4).take(height) {
                pixels.extend_from_slice(&row[..width * 4]);
            }
            pixels
        };
        if let Some(background) = self.background {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                if pixel[3] == 0 {
                    *pixel = background;
                }
            }
        }
        Some(Image {
            width: width as u32,
            height: height as u32,
            pixels,
        })
    }

    fn begin(&mut self, pending: Pending) {
        self.pending = pending;
        self.params = [0; 5];
        self.count = 0;
        self.repeat = 1;
    }

    fn finish_pending(&mut self) {
        let [a, b, c, d, e] = self.params;
        match std::mem::replace(&mut self.pending, Pending::None) {
            Pending::None => {}
            Pending::Raster => {
                // The pixel aspect ratio (`a`, `b`) is taken as 1:1, which
                // is what encoders send.
                self.declared = ((c as usize).min(MAX_SIDE), (d as usize).min(MAX_SIDE));
            }
            Pending::Color => {
                let register = a as usize % REGISTERS;
                if self.count >= 4 {
                    let rgba = match b {
                        1 => rgba_from_hls(c, d, e),
                        2 => rgba_from_percent([c, d, e]),
                        _ => self.palette[register],
                    };
                    self.palette[register] = rgba;
                }
                self.color = self.palette[register];
            }
            Pending::Repeat => self.repeat = (a as usize).max(1),
        }
    }

    /// Draws a sixel with `bits` (bit 0 at the top) `repeat` times, and
    /// moves past it. Past the budget, it only moves.
    fn sixel(&mut self, bits: u8, repeat: usize) {
        let start = self.x;
        let end = start.saturating_add(repeat).min(MAX_SIDE);
        self.x = start.saturating_add(repeat);
        if start >= end || self.y >= MAX_SIDE {
            return;
        }
        // Without a bit set, it still widens the image but paints nothing.
        if bits == 0 {
            self.extent.0 = self.extent.0.max(end);
            return;
        }
        // Just past the lowest bit set.
        let bottom = (self.y + 8 - bits.leading_zeros() as usize).min(MAX_SIDE);
        if !self.reserve(end, bottom) || !self.spend((end - start) * bits.count_ones() as usize) {
            return;
        }
        self.extent = (self.extent.0.max(end), self.extent.1.max(bottom));
        let color = self.color;
        for bit in 0..6 {
            let y = self.y + bit;
            if bits & (1 << bit) == 0 || y >= MAX_SIDE {
                continue;
            }
            let row = y * self.stride * 4;
            for pixel in self.pixels[row + start * 4..row + end * 4]
                .as_chunks_mut::<4>()
                .0
            {
                *pixel = color;
            }
        }
    }

    /// Takes `pixels` from the budget, if it has them.
    fn spend(&mut self, pixels: usize) -> bool {
        let Some(left) = self.budget.checked_sub(pixels) else {
            return false;
        };
        self.budget = left;
        true
    }

    /// Grows the canvas to at least `width` × `height` (at most
    /// `MAX_SIDE`), at least doubling a side that grows so that drawing left
    /// to right stays linear. Returns false, leaving it, when the budget
    /// can't pay for the new pixels.
    fn reserve(&mut self, width: usize, height: usize) -> bool {
        let (width, height) = (width.min(MAX_SIDE), height.min(MAX_SIDE));
        let rows = self.pixels.len().checked_div(self.stride * 4).unwrap_or(0);
        if width <= self.stride && height <= rows {
            return true;
        }
        let new_stride = if width > self.stride {
            width.max(self.stride * 2).min(MAX_SIDE)
        } else {
            self.stride
        };
        let new_rows = if height > rows {
            height.max(rows * 2).min(MAX_SIDE)
        } else {
            rows
        };
        if !self.spend(new_stride * new_rows - self.stride * rows) {
            return false;
        }
        if width > self.stride {
            let stride = new_stride;
            let mut pixels = vec![0; stride * rows * 4];
            if self.stride > 0 {
                for (new, old) in pixels
                    .chunks_exact_mut(stride * 4)
                    .zip(self.pixels.chunks_exact(self.stride * 4))
                {
                    new[..old.len()].copy_from_slice(old);
                }
            }
            self.pixels = pixels;
            self.stride = stride;
        }
        if height > rows {
            self.pixels.resize(self.stride * new_rows * 4, 0);
        }
        true
    }
}

/// An opaque color from red, green and blue in percent.
fn rgba_from_percent(rgb: [u32; 3]) -> [u8; 4] {
    let [r, g, b] = rgb.map(|v| (v.min(100) * 255 + 50) / 100);
    [r as u8, g as u8, b as u8, 255]
}

/// An opaque color from hue (0–360°, 0° being blue), lightness and
/// saturation (in percent), as DEC defines HLS.
fn rgba_from_hls(hue: u32, lightness: u32, saturation: u32) -> [u8; 4] {
    // DEC puts blue at 0° and red at 120°; the usual formula has red at 0°.
    let h = ((hue % 360 + 240) % 360) as f32 / 60.0;
    let l = lightness.min(100) as f32 / 100.0;
    let s = saturation.min(100) as f32 / 100.0;
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = l - chroma / 2.0;
    let channel = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [channel(r), channel(g), channel(b), 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(transparent: bool, data: &[u8]) -> Option<Image> {
        let mut decoder = Decoder::new(transparent, [1, 2, 3]);
        for &byte in data {
            decoder.put(byte);
        }
        decoder.finish()
    }

    fn pixel(image: &Image, x: usize, y: usize) -> [u8; 4] {
        let i = (y * image.width as usize + x) * 4;
        image.pixels[i..i + 4]
            .try_into()
            .expect("a pixel is 4 bytes")
    }

    #[test]
    fn sixel_bits_paint_a_column_from_the_top() {
        // '@' is bit 0: only the top pixel; 'B' is bit 1.
        let image = decode(true, b"#1;2;100;0;0@B").expect("an image");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 0, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&image, 1, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn repeat_draws_the_next_sixel_n_times() {
        let image = decode(true, b"#2;2;0;100;0!5~").expect("an image");
        assert_eq!((image.width, image.height), (5, 6));
        assert!(
            image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 255, 0, 255])
        );
    }

    #[test]
    fn dollar_returns_and_dash_moves_down_a_band() {
        let image = decode(true, b"#1;2;100;0;0@$#2;2;0;0;100?@-@").expect("an image");
        assert_eq!((image.width, image.height), (2, 7));
        assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 1, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&image, 0, 6), [0, 0, 255, 255]);
    }

    #[test]
    fn raster_attributes_size_the_image_and_background_fills_it() {
        let image = decode(false, b"\"1;1;3;4#1;2;100;100;100@").expect("an image");
        assert_eq!((image.width, image.height), (3, 4));
        assert_eq!(pixel(&image, 0, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&image, 2, 3), [1, 2, 3, 255]);
    }

    #[test]
    fn hls_colors_put_blue_at_zero_degrees() {
        assert_eq!(rgba_from_hls(0, 50, 100), [0, 0, 255, 255]);
        assert_eq!(rgba_from_hls(120, 50, 100), [255, 0, 0, 255]);
        assert_eq!(rgba_from_hls(240, 50, 100), [0, 255, 0, 255]);
    }

    #[test]
    fn selecting_a_register_uses_its_color() {
        // Register 2 is VT340 red until redefined.
        let image = decode(true, b"#2@").expect("an image");
        assert_eq!(pixel(&image, 0, 0), rgba_from_percent([80, 13, 13]));
    }

    #[test]
    fn an_empty_image_is_none() {
        assert_eq!(decode(true, b""), None);
        assert_eq!(decode(true, b"#1;2;0;0;0"), None);
    }

    #[test]
    fn a_few_bytes_cant_buy_a_large_canvas() {
        // Raster attributes alone declare an image they don't pay for.
        assert_eq!(decode(false, b"\"1;1;4096;4096"), None);
        // One pixel far from the origin is dropped likewise.
        let mut data = b"@".to_vec();
        data.extend([b'-'; 600]);
        data.extend(b"!4000@");
        let image = decode(true, &data).expect("an image");
        assert_eq!((image.width, image.height), (1, 1));
    }

    #[test]
    fn the_budget_pays_for_a_solid_full_size_image() {
        let mut data = b"\"1;1;4096;4096#1;2;100;0;0".to_vec();
        for _ in 0..4096 / 6 {
            data.extend(b"!4096~-");
        }
        data.extend(b"!4096N");
        let image = decode(true, &data).expect("an image");
        assert_eq!((image.width, image.height), (4096, 4096));
        assert!(image.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn repeat_applies_only_to_the_next_sixel() {
        let image = decode(true, b"#1;2;100;0;0!5$@").expect("an image");
        assert_eq!((image.width, image.height), (1, 1));
    }

    #[test]
    fn huge_repeats_and_sizes_are_capped() {
        let image = decode(true, b"\"1;1;99999999;1!99999999@").expect("an image");
        assert_eq!((image.width, image.height), (MAX_SIDE as u32, 1));
    }
}
