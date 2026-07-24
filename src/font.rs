//! Font stack: the embedded monospace font plus optional fallbacks.
//!
//! The binary stays standalone — every font here is `include_bytes!`'d. A
//! stack exists because no single free monospace font covers both Latin and
//! CJK: DejaVu Sans Mono draws the ASCII ramps, a subset of Noto Sans Mono
//! CJK draws the `chinese`/`korean`/`japanese` presets, and users can prepend
//! their own font with `--font` for anything else.

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// DejaVu Sans Mono Bold — the primary font.
/// License: `assets/fonts/DejaVu-Fonts-License.txt`.
pub const DEJAVU_BYTES: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono-Bold.ttf");

/// Noto Sans Mono CJK SC Bold, subset to the glyphs the CJK presets use
/// (36 KB instead of 19 MB). License: `assets/fonts/Noto-CJK-License.txt`.
#[cfg(feature = "cjk")]
pub const CJK_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansMonoCJK-Subset-Bold.otf");

/// Where a character's glyph came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    /// Index into [`FontStack::fonts`].
    pub font: usize,
    pub glyph: GlyphId,
}

/// Pixel geometry of one character cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    /// Baseline offset from the top of the cell.
    pub ascent: f32,
}

impl CellMetrics {
    /// Cell height ÷ width — the value [`crate::engine::Options::aspect`]
    /// needs so a painted image keeps the source's proportions.
    pub fn aspect(&self) -> f32 {
        self.height as f32 / self.width as f32
    }
}

/// An ordered list of fonts; the first one that has a glyph wins.
pub struct FontStack {
    fonts: Vec<FontArc>,
}

impl FontStack {
    /// The built-in stack: DejaVu Sans Mono Bold, then the CJK subset.
    pub fn embedded() -> Self {
        let mut fonts =
            vec![FontArc::try_from_slice(DEJAVU_BYTES).expect("embedded font is valid")];
        #[cfg(feature = "cjk")]
        fonts.push(FontArc::try_from_slice(CJK_BYTES).expect("embedded CJK font is valid"));
        Self { fonts }
    }

    /// The built-in stack with a user font in front, so it wins every glyph
    /// it has (and supplies the cell metrics).
    pub fn with_font_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read font '{}'", path.display()))?;
        let font = FontArc::try_from_vec(bytes)
            .with_context(|| format!("'{}' is not a usable TTF/OTF font", path.display()))?;
        let mut stack = Self::embedded();
        stack.fonts.insert(0, font);
        Ok(stack)
    }

    /// The font that defines the character grid.
    pub fn primary(&self) -> &FontArc {
        &self.fonts[0]
    }

    pub fn get(&self, index: usize) -> &FontArc {
        &self.fonts[index]
    }

    /// First font in the stack that can draw `c`, if any.
    pub fn locate(&self, c: char) -> Option<Located> {
        self.fonts.iter().enumerate().find_map(|(font, f)| {
            let glyph = f.glyph_id(c);
            (glyph != GlyphId(0)).then_some(Located { font, glyph })
        })
    }

    /// Resolve every character once — paint loops call this instead of
    /// walking the stack per cell.
    ///
    /// A grid holds millions of cells but only a charset's worth of distinct
    /// characters, so deduplicate before touching a font's cmap.
    pub fn locate_all<'a>(
        &self,
        chars: impl IntoIterator<Item = &'a char>,
    ) -> HashMap<char, Located> {
        let distinct: std::collections::BTreeSet<char> =
            chars.into_iter().copied().filter(|&c| c != ' ').collect();
        distinct
            .into_iter()
            .filter_map(|c| self.locate(c).map(|l| (c, l)))
            .collect()
    }

    /// Cell geometry for a charset at `font_px`.
    ///
    /// Width is the widest advance any of `chars` needs, so full-width CJK
    /// glyphs get a full-width cell instead of being clipped in half; height
    /// and baseline span every font actually used. For a pure-ASCII charset
    /// this is exactly the primary font's monospace advance.
    pub fn cell_metrics<'a>(
        &self,
        chars: impl IntoIterator<Item = &'a char>,
        font_px: f32,
    ) -> CellMetrics {
        let scale = PxScale::from(font_px);
        let primary = self.primary().as_scaled(scale);
        let mut width = primary.h_advance(self.primary().glyph_id('M'));
        let mut height = primary.height();
        let mut ascent = primary.ascent();

        for c in chars {
            let Some(loc) = self.locate(*c) else { continue };
            let scaled = self.fonts[loc.font].as_scaled(scale);
            width = width.max(scaled.h_advance(loc.glyph));
            height = height.max(scaled.height());
            ascent = ascent.max(scaled.ascent());
        }
        CellMetrics {
            width: width.ceil().max(1.0) as u32,
            height: height.ceil().max(1.0) as u32,
            ascent,
        }
    }

    /// Fraction of a cell that `c`'s glyph inks, in the first font that has
    /// it. Used to sort charsets dark → light.
    pub fn coverage(&self, c: char, font_px: f32) -> f32 {
        let Some(loc) = self.locate(c) else {
            return 0.0;
        };
        let scale = PxScale::from(font_px);
        let font = &self.fonts[loc.font];
        let scaled = font.as_scaled(scale);
        let cell_area = scaled.h_advance(loc.glyph) * scaled.height();
        if cell_area <= 0.0 {
            return 0.0;
        }
        let glyph = loc
            .glyph
            .with_scale_and_position(scale, ab_glyph::point(0.0, scaled.ascent()));
        match font.outline_glyph(glyph) {
            Some(outline) => {
                let mut sum = 0.0f32;
                outline.draw(|_, _, c| sum += c);
                sum / cell_area
            }
            None => 0.0,
        }
    }
}

impl Default for FontStack {
    fn default() -> Self {
        Self::embedded()
    }
}

/// The built-in stack, parsed once per process.
pub fn embedded() -> &'static FontStack {
    static STACK: std::sync::OnceLock<FontStack> = std::sync::OnceLock::new();
    STACK.get_or_init(FontStack::embedded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_resolves_to_the_primary_font() {
        let stack = FontStack::embedded();
        assert_eq!(stack.locate('A').unwrap().font, 0);
        assert!(stack.locate('@').is_some());
    }

    #[test]
    fn braille_is_absent_from_every_bundled_font() {
        // Neither DejaVu Sans Mono nor the CJK subset ships U+2800–U+28FF, so
        // `paint` plots braille dots itself. If a future font *does* cover the
        // block this test flips and the procedural path becomes dead code.
        let stack = FontStack::embedded();
        for c in ['\u{2801}', '\u{28FF}', '\u{2847}'] {
            assert!(stack.locate(c).is_none(), "{c:?} unexpectedly has a glyph");
        }
    }

    #[cfg(feature = "cjk")]
    #[test]
    fn cjk_falls_back_to_the_noto_subset() {
        let stack = FontStack::embedded();
        for c in ['龘', '一', 'あ', 'ア', 'ㄱ'] {
            let loc = stack.locate(c).unwrap_or_else(|| panic!("missing {c:?}"));
            assert_eq!(loc.font, 1, "{c:?} should come from the CJK fallback");
        }
    }

    #[test]
    fn unknown_glyphs_are_reported_missing() {
        // U+E000 is a private-use codepoint no bundled font maps.
        assert!(FontStack::embedded().locate('\u{E000}').is_none());
    }

    #[test]
    fn ascii_metrics_match_the_monospace_advance() {
        let stack = FontStack::embedded();
        let m = stack.cell_metrics(&['@', '.', ' '], 16.0);
        assert_eq!(m.width, 9); // DejaVu Sans Mono's advance at 16 px
                                // Monospace: every ASCII glyph must agree on the cell width.
        assert_eq!(stack.cell_metrics(&['W', 'i', '|'], 16.0).width, m.width);
        assert!(m.height > m.width, "cells are taller than wide");
        assert!((m.aspect() - 1.9).abs() < 0.2, "aspect {}", m.aspect());
    }

    #[cfg(feature = "cjk")]
    #[test]
    fn cjk_metrics_widen_the_cell() {
        let stack = FontStack::embedded();
        let latin = stack.cell_metrics(&['@'], 16.0);
        let cjk = stack.cell_metrics(&['龘'], 16.0);
        assert!(
            cjk.width > latin.width,
            "full-width glyphs need a wider cell ({} vs {})",
            cjk.width,
            latin.width
        );
    }

    #[test]
    fn coverage_orders_dense_before_sparse() {
        let stack = FontStack::embedded();
        assert!(stack.coverage('@', 32.0) > stack.coverage('.', 32.0));
        assert_eq!(stack.coverage(' ', 32.0), 0.0);
    }

    #[test]
    fn missing_font_file_errors() {
        assert!(FontStack::with_font_file("/nonexistent/font.ttf").is_err());
    }
}
