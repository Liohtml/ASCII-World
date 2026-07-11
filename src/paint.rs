//! PNG renderer: paints an [`AsciiGrid`] with the embedded monospace font.

use crate::engine::AsciiGrid;
use ab_glyph::{point, Font, GlyphId, PxScale, ScaleFont};
use anyhow::{bail, Result};
use image::{Rgb, RgbImage};

/// Largest accepted `font_px`; above this a single glyph is poster-sized and
/// canvas math risks overflowing.
pub const MAX_FONT_PX: f32 = 512.0;
/// Cap on output pixels (w × h) so absurd width/font combinations fail fast
/// instead of attempting a multi-gigabyte allocation.
const MAX_CANVAS_PIXELS: u64 = 500_000_000;

/// Background style for painted output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Black,
    White,
}

impl Background {
    fn fill(self) -> Rgb<u8> {
        match self {
            Background::Black => Rgb([0, 0, 0]),
            Background::White => Rgb([255, 255, 255]),
        }
    }

    /// Default glyph color when not painting per-cell colors.
    fn ink(self) -> [u8; 3] {
        match self {
            Background::Black => [255, 255, 255],
            Background::White => [0, 0, 0],
        }
    }

    /// The ramp inversion that keeps glyph density tracking contrast against
    /// this canvas: on black, bright cells need dense glyphs. Pass this as
    /// [`crate::engine::Options::invert`] when converting for [`paint_png`].
    pub fn default_invert(self) -> bool {
        matches!(self, Background::Black)
    }
}

/// Paint the grid into an RGB image using the embedded DejaVu Sans Mono Bold.
///
/// `colored` uses each cell's average source color as the glyph color
/// (value-boosted on black backgrounds, where glyph density already encodes
/// luminance); otherwise glyphs are white-on-black or black-on-white per
/// `background`. `font_px` controls the glyph size and therefore the output
/// resolution. Characters the embedded font cannot draw are left blank.
pub fn paint_png(
    grid: &AsciiGrid,
    background: Background,
    colored: bool,
    font_px: f32,
) -> Result<RgbImage> {
    if !(1.0..=MAX_FONT_PX).contains(&font_px) {
        bail!("font_px must be between 1 and {MAX_FONT_PX} (got {font_px})");
    }
    let font = crate::font();
    let scale = PxScale::from(font_px);
    let scaled = font.as_scaled(scale);

    let cell_w = scaled.h_advance(font.glyph_id('M')).ceil() as u32;
    let cell_h = scaled.height().ceil() as u32;
    let ascent = scaled.ascent();

    let (out_w, out_h) = (
        cell_w as u64 * grid.cols as u64,
        cell_h as u64 * grid.rows as u64,
    );
    if out_w * out_h > MAX_CANVAS_PIXELS {
        bail!(
            "output canvas {out_w}x{out_h} exceeds {MAX_CANVAS_PIXELS} pixels; \
             lower --width or --font-px"
        );
    }
    let (out_w, out_h) = (out_w as u32, out_h as u32);
    let mut img = RgbImage::from_pixel(out_w, out_h, background.fill());

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let ch = grid.char_at(row, col);
            if ch == ' ' {
                continue;
            }
            let glyph_id = font.glyph_id(ch);
            if glyph_id == GlyphId(0) {
                // Missing from the font: blank beats an identical tofu box
                // in every cell, which would destroy the density ramp.
                continue;
            }
            let ink = if !colored {
                background.ink()
            } else if background == Background::Black {
                brighten(grid.color_at(row, col))
            } else {
                grid.color_at(row, col)
            };
            // Clip to the cell so overshooting glyphs (e.g. █ block elements)
            // cannot bleed color into neighboring cells.
            let (cx0, cy0) = ((col * cell_w) as i64, (row * cell_h) as i64);
            let (cx1, cy1) = (cx0 + cell_w as i64, cy0 + cell_h as i64);
            let glyph =
                glyph_id.with_scale_and_position(scale, point(cx0 as f32, cy0 as f32 + ascent));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let x = bounds.min.x as i64 + gx as i64;
                    let y = bounds.min.y as i64 + gy as i64;
                    if x < cx0
                        || y < cy0
                        || x >= cx1.min(out_w as i64)
                        || y >= cy1.min(out_h as i64)
                    {
                        return;
                    }
                    let pixel = img.get_pixel_mut(x as u32, y as u32);
                    for (px, &fg) in pixel.0.iter_mut().zip(ink.iter()) {
                        let bg = *px as f32;
                        *px = (bg + (fg as f32 - bg) * coverage.clamp(0.0, 1.0)) as u8;
                    }
                });
            }
        }
    }

    Ok(img)
}

/// Value-normalize a cell color: keep the hue, push the brightest channel
/// toward full intensity. On black backgrounds glyph density already encodes
/// luminance, so without this boost colored output ends up muddy (ink covers
/// only part of a cell). Never applied on white backgrounds, where it would
/// wash dark tones into invisibility.
fn brighten(color: [u8; 3]) -> [u8; 3] {
    let max = color.into_iter().max().unwrap_or(0);
    if max == 0 {
        return color;
    }
    // Blend 70% toward full value normalization.
    let gain = 1.0 + 0.7 * (255.0 / max as f32 - 1.0);
    color.map(|c| (c as f32 * gain).min(255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{convert, Options};
    use image::RgbImage;

    fn grid_of(src: &RgbImage, width: u32, invert: bool) -> AsciiGrid {
        convert(
            src,
            &Options {
                width,
                invert,
                ..Options::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn paints_expected_dimensions_and_ink() {
        let src = RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        let grid = grid_of(&src, 8, false);
        let img = paint_png(&grid, Background::Black, false, 16.0).unwrap();
        assert_eq!(img.width() % grid.cols, 0);
        assert_eq!(img.height() % grid.rows, 0);
        // A black source maps to '@' glyphs — some pixels must be lit.
        assert!(img.pixels().any(|p| p.0[0] > 128));
    }

    #[test]
    fn white_source_paints_empty_canvas() {
        let src = RgbImage::from_pixel(64, 64, image::Rgb([255, 255, 255]));
        let grid = grid_of(&src, 8, false);
        let img = paint_png(&grid, Background::Black, false, 16.0).unwrap();
        assert!(img.pixels().all(|p| p.0 == [0, 0, 0]));
    }

    #[test]
    fn colored_on_white_keeps_dark_ink_dark() {
        // Regression: brighten() must not apply on white backgrounds, or dark
        // grays become near-invisible pastels.
        let src = RgbImage::from_pixel(64, 64, image::Rgb([20, 20, 20]));
        let grid = grid_of(&src, 8, false);
        let img = paint_png(&grid, Background::White, true, 16.0).unwrap();
        let darkest = img.pixels().map(|p| p.0[0]).min().unwrap();
        assert!(darkest <= 30, "dark source ink washed out: {darkest}");
    }

    #[test]
    fn rejects_invalid_font_px() {
        let src = RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0]));
        let grid = grid_of(&src, 4, false);
        for bad in [0.0, -5.0, f32::NAN, 4e7] {
            assert!(paint_png(&grid, Background::Black, false, bad).is_err());
        }
    }

    #[test]
    fn missing_glyphs_paint_blank_not_tofu() {
        let src = RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        let opts = Options {
            width: 8,
            charset: vec!['中', ' '], // not in DejaVu Sans Mono
            ..Options::default()
        };
        let grid = convert(&src, &opts).unwrap();
        let img = paint_png(&grid, Background::Black, false, 16.0).unwrap();
        assert!(img.pixels().all(|p| p.0 == [0, 0, 0]));
    }

    #[test]
    fn glyphs_stay_inside_their_cells() {
        // █ overshoots its advance box in DejaVu (outline starts at x = -1
        // relative to the cell); clipping must keep it out of the neighbor.
        let grid = AsciiGrid {
            cols: 2,
            rows: 1,
            chars: vec![' ', '█'],
            colors: vec![[0, 0, 0]; 2],
        };
        let img = paint_png(&grid, Background::White, false, 16.0).unwrap();
        let cell_w = img.width() / 2;
        // Every pixel of cell 0 (including its rightmost column, where the
        // neighbor's overshoot would land) must remain pure background.
        for y in 0..img.height() {
            for x in 0..cell_w {
                assert_eq!(img.get_pixel(x, y).0, [255, 255, 255], "bleed at {x},{y}");
            }
        }
        // Sanity: the glyph itself did paint something in cell 1.
        assert!(img.pixels().any(|p| p.0 != [255, 255, 255]));
    }
}
