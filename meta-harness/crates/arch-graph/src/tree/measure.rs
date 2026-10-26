//! Card text widths, measured with the cockpit theme's proportional fonts
//! (egui's defaults: Ubuntu Light, then the two emoji fonts, each at the
//! theme's scale) the way egui lays a line out: each glyph's advance plus
//! the font's pair kerning. Advances and kerning pairs are kept once
//! measured, so a width is a sum of kept numbers.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use ttf_parser::{Face, GlyphId};

/// The proportional fonts in the order the theme falls back through them,
/// each with the scale the theme draws it at. (The theme's icon font sits
/// second; it holds only private-use characters, which card text doesn't
/// carry.)
const FONTS: [(&[u8], f32); 3] =
    [(epaint_default_fonts::UBUNTU_LIGHT, 1.0), (epaint_default_fonts::NOTO_EMOJI_REGULAR, 0.81), (epaint_default_fonts::EMOJI_ICON, 0.90)];

/// Drawn for a character no font has, as egui does (the first one found).
const REPLACEMENTS: [char; 2] = ['◻', '?'];

struct Font {
    face: Face<'static>,
    /// Font units to a share of the font size.
    per_unit: f32,
}

/// A character's glyph: which font has it, its id there, and its advance as
/// a share of the font size.
#[derive(Clone, Copy)]
struct Glyph {
    font: usize,
    id: GlyphId,
    advance: f32,
}

#[derive(Default)]
struct Kept {
    glyphs: HashMap<char, Glyph>,
    kerning: HashMap<(char, char), f32>,
}

struct Measure {
    fonts: Vec<Font>,
    kept: Mutex<Kept>,
}

static MEASURE: LazyLock<Measure> = LazyLock::new(|| Measure {
    fonts: FONTS
        .iter()
        .map(|(bytes, scale)| {
            let face = Face::parse(bytes, 0).expect("the theme's fonts parse");
            Font { per_unit: scale / f32::from(face.units_per_em()), face }
        })
        .collect(),
    kept: Mutex::default(),
});

impl Measure {
    fn find(&self, c: char) -> Option<Glyph> {
        self.fonts.iter().enumerate().find_map(|(font, f)| {
            let id = f.face.glyph_index(c)?;
            Some(Glyph { font, id, advance: f32::from(f.face.glyph_hor_advance(id).unwrap_or_default()) * f.per_unit })
        })
    }

    fn glyph(&self, kept: &mut Kept, c: char) -> Glyph {
        if let Some(g) = kept.glyphs.get(&c) {
            return *g;
        }
        let found = self.find(c).or_else(|| REPLACEMENTS.iter().find_map(|r| self.find(*r))).unwrap_or(Glyph {
            font: 0,
            id: GlyphId(0),
            advance: 0.0,
        });
        kept.glyphs.insert(c, found);
        found
    }

    /// The pair's kerning in the second glyph's font, as egui does (even when
    /// the first came from another font).
    fn kerning(&self, kept: &mut Kept, (a, b): (char, char), (ga, gb): (Glyph, Glyph)) -> f32 {
        *kept.kerning.entry((a, b)).or_insert_with(|| {
            let font = &self.fonts[gb.font];
            let units = font.face.tables().kern.and_then(|kern| {
                kern.subtables.into_iter().filter(|s| s.horizontal && !s.variable).find_map(|s| s.glyphs_kerning(ga.id, gb.id))
            });
            f32::from(units.unwrap_or_default()) * font.per_unit
        })
    }

    fn width(&self, text: &str, size: f32) -> f32 {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        let mut ems = 0.0;
        let mut last: Option<(char, Glyph)> = None;
        for c in text.chars() {
            let g = self.glyph(&mut kept, c);
            if let Some((lc, lg)) = last {
                ems += self.kerning(&mut kept, (lc, c), (lg, g));
            }
            ems += g.advance;
            last = Some((c, g));
        }
        ems * size
    }
}

/// How wide `text` is drawn at `size` in the theme's proportional font.
pub fn text_width(text: &str, size: f32) -> f32 {
    MEASURE.width(text, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_follow_the_glyphs_not_the_character_count() {
        // Same length, different glyphs: a character-width guess makes them equal.
        let narrow = text_width("iiiiiiii", 14.0);
        let wide = text_width("MMMMMMMM", 14.0);
        assert!(wide > narrow * 2.0, "{narrow} vs {wide}");
        // Widths scale with the size.
        assert!((text_width("step", 28.0) - 2.0 * text_width("step", 14.0)).abs() < 1e-3);
        assert_eq!(text_width("", 14.0), 0.0);
    }

    #[test]
    fn measured_glyphs_and_pairs_are_kept() {
        let width = text_width("Kept€", 14.0);
        {
            let kept = MEASURE.kept.lock().unwrap();
            assert!("Kept€".chars().all(|c| kept.glyphs.contains_key(&c)));
            assert!(kept.kerning.contains_key(&('K', 'e')) && kept.kerning.contains_key(&('t', '€')));
        }
        // Measured again from what's kept: the same width.
        assert_eq!(text_width("Kept€", 14.0), width);
    }
}
