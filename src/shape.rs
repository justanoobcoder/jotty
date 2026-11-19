//! Text shaping with harfrust: the glyphs a font draws for a run of text,
//! which can differ from its characters' own glyphs (ligatures, the joining
//! forms of Arabic letters, marks placed over the letter they follow).

use std::collections::HashMap;
use std::ops::Range;

use harfrust::{
    Direction, Feature, FontRef, GlyphBuffer, GlyphInfo, GlyphPosition, Script, ShapeOptions,
    ShapePlan, ShaperData, ShaperInstance, Tag, UnicodeBuffer, Variation,
};

/// A glyph that shaping draws in a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapedGlyph {
    /// The face, by its index in `Fonts`.
    pub face: usize,
    pub id: u16,
    /// Pixels from the cell's left edge to the glyph's origin.
    pub x: i32,
    /// Pixels the glyph moves down from the baseline.
    pub y: i32,
}

/// A word as shaping draws it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shaped {
    /// For each cell of the word: `None` when it shows its character's own
    /// glyph, as it would without shaping, or else the glyphs drawn from
    /// it, a range of `glyphs`. The range is empty when a ligature drawn
    /// from an earlier cell covers the cell.
    pub cells: Vec<Option<Range<usize>>>,
    pub glyphs: Vec<ShapedGlyph>,
}

/// The OpenType features that make ligatures, which `font.ligatures`
/// turns off (the same as kitty's `disable_ligatures`).
const LIGATURE_FEATURES: [&[u8; 4]; 3] = [b"liga", b"calt", b"dlig"];

/// A face's shaping tables.
pub struct ShapingFace {
    font: FontRef<'static>,
    data: ShaperData,
    instance: ShaperInstance,
    /// The plans made so far, by script and whether they make ligatures:
    /// making one takes most of the time of shaping a word.
    plans: HashMap<(Script, bool), ShapePlan>,
}

impl ShapingFace {
    /// Reads the face at `index` in `data`, with its variable font axes
    /// set to `variations`.
    pub fn try_new(
        data: &'static [u8],
        index: u32,
        variations: &[([u8; 4], f32)],
    ) -> Result<ShapingFace, &'static str> {
        let font = FontRef::from_index(data, index).map_err(|_| "invalid font")?;
        let variations = variations.iter().map(|(tag, value)| Variation {
            tag: Tag::new(tag),
            value: *value,
        });
        let instance = ShaperInstance::from_variations(&font, variations);
        let data = ShaperData::new(&font);
        Ok(ShapingFace {
            font,
            data,
            instance,
            plans: HashMap::new(),
        })
    }

    /// Shapes `text` left to right, with or without the features that make
    /// ligatures. A terminal keeps text in the order it was written, so
    /// right-to-left text is not reversed. Glyph clusters are byte offsets
    /// in `text`, and positions are in font units.
    #[must_use]
    pub fn shape(&mut self, text: &str, ligatures: bool) -> GlyphBuffer {
        let shaper = self
            .data
            .shaper(&self.font)
            .instance(Some(&self.instance))
            .build();
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        buffer.set_direction(Direction::LeftToRight);
        // Set, so that it is the plan's: a plan must match the buffer.
        let script = buffer.script();
        buffer.set_script(script);
        let off: [Feature; 3] = LIGATURE_FEATURES.map(|tag| Feature::new(Tag::new(tag), 0, ..));
        let features: &[Feature] = if ligatures { &[] } else { &off };
        let plan = self.plans.entry((script, ligatures)).or_insert_with(|| {
            ShapePlan::new(
                &shaper,
                Direction::LeftToRight,
                Some(script),
                None,
                features,
            )
        });
        shaper.shape(
            buffer,
            ShapeOptions::new().plan(Some(plan)).features(features),
        )
    }
}

/// Adds the glyphs shaped from a run of a word's cells, all in `face`, to
/// `shaped`, whose `cells` covers the whole word. The run starts at the
/// word's cell `first`. `starts` holds the byte offset at which each of
/// its cells starts in the run's text, the first one 0, in order. `scale`
/// converts font units to pixels, and `own_glyph(cell)` is the glyph a cell
/// of the run shows without shaping, if it holds a single character.
///
/// Cells whose glyph shaping leaves as it is stay `None`. So does the
/// whole run when the font is missing a glyph, or when its clusters go
/// backwards, which a left-to-right run never does.
pub fn place(
    shaped: &mut Shaped,
    face: usize,
    first: usize,
    starts: &[usize],
    glyphs: (&[GlyphInfo], &[GlyphPosition]),
    scale: f32,
    own_glyph: impl Fn(usize) -> Option<u16>,
) {
    let base = shaped.glyphs.len();
    let run = first..first + starts.len();
    let give_up = |shaped: &mut Shaped| {
        shaped.glyphs.truncate(base);
        shaped.cells[run.clone()].fill(None);
    };
    shaped.cells[run.clone()].fill(Some(base..base));
    let mut current = None;
    // The pen starts over at each cell, so that rounding never adds up and
    // the glyphs stay on the grid.
    let mut pen = 0;
    for (info, pos) in glyphs.0.iter().zip(glyphs.1) {
        let cell = starts
            .partition_point(|&start| start <= info.cluster as usize)
            .saturating_sub(1);
        match current {
            Some(c) if c == cell => {}
            Some(c) if cell < c => return give_up(shaped),
            _ => {
                current = Some(cell);
                pen = 0;
                let len = shaped.glyphs.len();
                shaped.cells[first + cell] = Some(len..len);
            }
        }
        let id = match u16::try_from(info.glyph_id) {
            Ok(0) | Err(_) => return give_up(shaped),
            Ok(id) => id,
        };
        let to_px = |units: i32| (units as f32 * scale).round() as i32;
        shaped.glyphs.push(ShapedGlyph {
            face,
            id,
            x: to_px(pen + pos.x_offset),
            // Font units go up.
            y: -to_px(pos.y_offset),
        });
        if let Some(range) = &mut shaped.cells[first + cell] {
            range.end += 1;
        }
        pen += pos.x_advance;
    }
    for cell in 0..starts.len() {
        let Some(range) = shaped.cells[first + cell].clone() else {
            continue;
        };
        let unchanged = match shaped.glyphs[range] {
            [glyph] => glyph.x == 0 && glyph.y == 0 && own_glyph(cell) == Some(glyph.id),
            _ => false,
        };
        if unchanged {
            shaped.cells[first + cell] = None;
        }
    }
}

/// Characters of the scripts whose letters change shape with their
/// neighbors (Arabic, Syriac, N'Ko, Mandaic, Mongolian). They need shaping
/// even without ligatures, which characters of other scripts only need in
/// grapheme clusters (marks, Indic conjuncts, Hangul jamo).
#[must_use]
pub fn joins_neighbors(c: char) -> bool {
    matches!(c,
        '\u{0600}'..='\u{08ff}'
        | '\u{1800}'..='\u{18af}'
        | '\u{fb50}'..='\u{fdff}'
        | '\u{fe70}'..='\u{feff}'
        | '\u{10d00}'..='\u{10d3f}'
        | '\u{1e900}'..='\u{1e95f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(glyph_id: u32, cluster: u32) -> GlyphInfo {
        let mut info = GlyphInfo::default();
        info.glyph_id = glyph_id;
        info.cluster = cluster;
        info
    }

    fn advance(x_advance: i32, x_offset: i32, y_offset: i32) -> GlyphPosition {
        let mut position = GlyphPosition::default();
        position.x_advance = x_advance;
        position.x_offset = x_offset;
        position.y_offset = y_offset;
        position
    }

    fn glyph(id: u16, x: i32, y: i32) -> ShapedGlyph {
        ShapedGlyph { face: 3, id, x, y }
    }

    /// Places glyphs for a whole word of `cells` cells starting at `starts`,
    /// whose own glyphs are `own`, at 0.5 pixel per unit.
    fn placed(
        cells: usize,
        starts: &[usize],
        infos: &[GlyphInfo],
        positions: &[GlyphPosition],
        own: &[u16],
    ) -> Shaped {
        let mut shaped = Shaped {
            cells: vec![None; cells],
            glyphs: Vec::new(),
        };
        place(&mut shaped, 3, 0, starts, (infos, positions), 0.5, |cell| {
            own.get(cell).copied()
        });
        shaped
    }

    #[test]
    fn unchanged_glyphs_are_left_to_the_cells() {
        let shaped = placed(
            2,
            &[0, 1],
            &[info(10, 0), info(11, 1)],
            &[advance(600, 0, 0), advance(600, 0, 0)],
            &[10, 11],
        );
        assert_eq!(shaped.cells, [None, None]);
    }

    #[test]
    fn contextual_alternates_replace_each_cell() {
        // `->` in a font that draws the arrow from the second cell.
        let shaped = placed(
            2,
            &[0, 1],
            &[info(50, 0), info(51, 1)],
            &[advance(600, 0, 0), advance(600, -100, 0)],
            &[10, 11],
        );
        assert_eq!(shaped.cells, [Some(0..1), Some(1..2)]);
        assert_eq!(shaped.glyphs, [glyph(50, 0, 0), glyph(51, -50, 0)]);
    }

    #[test]
    fn a_ligature_covers_the_cells_it_joins() {
        let shaped = placed(
            3,
            &[0, 1, 2],
            &[info(60, 0), info(12, 2)],
            &[advance(1200, 0, 0), advance(600, 0, 0)],
            &[10, 11, 12],
        );
        assert_eq!(shaped.cells[0], Some(0..1));
        assert!(shaped.cells[1].as_ref().is_some_and(Range::is_empty));
        assert_eq!(shaped.cells[2], None);
    }

    #[test]
    fn marks_are_placed_from_their_base() {
        // `e` + U+0301 in one cell, the accent moved back over the `e`.
        let shaped = placed(
            1,
            &[0],
            &[info(10, 0), info(70, 0)],
            &[advance(600, 0, 0), advance(0, -500, 200)],
            &[],
        );
        assert_eq!(shaped.cells, [Some(0..2)]);
        assert_eq!(shaped.glyphs, [glyph(10, 0, 0), glyph(70, 50, -100)]);
    }

    #[test]
    fn a_missing_glyph_leaves_the_run_unshaped() {
        let shaped = placed(
            2,
            &[0, 1],
            &[info(50, 0), info(0, 1)],
            &[advance(600, 0, 0), advance(600, 0, 0)],
            &[10, 11],
        );
        assert_eq!(shaped.cells, [None, None]);
        assert!(shaped.glyphs.is_empty());
    }

    #[test]
    fn clusters_going_backwards_leave_the_run_unshaped() {
        let shaped = placed(
            2,
            &[0, 1],
            &[info(50, 1), info(51, 0)],
            &[advance(600, 0, 0), advance(600, 0, 0)],
            &[10, 11],
        );
        assert_eq!(shaped.cells, [None, None]);
    }

    #[test]
    fn a_run_fills_only_its_own_cells() {
        let mut shaped = Shaped {
            cells: vec![None; 4],
            glyphs: vec![glyph(1, 0, 0)],
        };
        place(
            &mut shaped,
            3,
            1,
            &[0, 2],
            (
                &[info(50, 0), info(51, 2)],
                &[advance(600, 0, 0), advance(600, 0, 0)],
            ),
            0.5,
            |_| None,
        );
        assert_eq!(shaped.cells, [None, Some(1..2), Some(2..3), None]);
    }
}
