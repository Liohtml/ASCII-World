//! PNG renderer: paints an [`AsciiGrid`] with the embedded monospace font.

use crate::engine::AsciiGrid;
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use anyhow::{Context, Result};
use image::{Rgb, RgbImage};

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
}

/// Paint the grid into an RGB image using the embedded DejaVu Sans Mono Bold.
///
/// `colored` uses each cell's average source color as the glyph color;
/// otherwise glyphs are white-on-black or black-on-white per `background`.
/// `font_px` controls the glyph size and therefore the output resolution.
pub fn paint_png(
    grid: &AsciiGrid,
    background: Background,
    colored: bool,
    font_px: f32,
) -> Result<RgbImage> {
    let font = FontRef::try_from_slice(crate::FONT_BYTES).context("embedded font is invalid")?;
    let scale = PxScale::from(font_px);
    let scaled = font.as_scaled(scale);

    let cell_w = scaled.h_advance(font.glyph_id('M')).ceil() as u32;
    let cell_h = scaled.height().ceil() as u32;
    let ascent = scaled.ascent();

    let out_w = cell_w * grid.cols;
    let out_h = cell_h * grid.rows;
    let mut img = RgbImage::from_pixel(out_w, out_h, background.fill());

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let ch = grid.char_at(row, col);
            if ch == ' ' {
                continue;
            }
            let ink = if colored {
                brighten(grid.color_at(row, col))
            } else {
                background.ink()
            };
            let glyph = font.glyph_id(ch).with_scale_and_position(
                scale,
                point((col * cell_w) as f32, row as f32 * cell_h as f32 + ascent),
            );
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let x = bounds.min.x as i64 + gx as i64;
                    let y = bounds.min.y as i64 + gy as i64;
                    if x < 0 || y < 0 || x >= out_w as i64 || y >= out_h as i64 {
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
/// toward full intensity. Glyph density already encodes luminance, so without
/// this boost colored output ends up muddy (ink covers only part of a cell).
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

    #[test]
    fn paints_expected_dimensions_and_ink() {
        let src = RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        let grid = convert(
            &src,
            &Options {
                width: 8,
                ..Options::default()
            },
        )
        .unwrap();
        let img = paint_png(&grid, Background::Black, false, 16.0).unwrap();
        assert_eq!(img.width() % grid.cols, 0);
        assert_eq!(img.height() % grid.rows, 0);
        // A black source maps to '@' glyphs — some pixels must be lit.
        assert!(img.pixels().any(|p| p.0[0] > 128));
    }

    #[test]
    fn white_source_paints_empty_canvas() {
        let src = RgbImage::from_pixel(64, 64, image::Rgb([255, 255, 255]));
        let grid = convert(
            &src,
            &Options {
                width: 8,
                ..Options::default()
            },
        )
        .unwrap();
        let img = paint_png(&grid, Background::Black, false, 16.0).unwrap();
        assert!(img.pixels().all(|p| p.0 == [0, 0, 0]));
    }
}
