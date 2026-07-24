//! PNG renderer: paints an [`AsciiGrid`] with the bundled monospace fonts.

use crate::charset::Charset;
use crate::engine::AsciiGrid;
use crate::font::{CellMetrics, FontStack};
use ab_glyph::{point, Font, PxScale};
use anyhow::{bail, Result};
use image::{Rgba, RgbaImage};

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
    /// No canvas at all: only glyphs carry opacity, so the art drops onto any
    /// surface. Requires an output format with an alpha channel (PNG, GIF).
    Transparent,
}

impl Background {
    fn fill(self) -> Rgba<u8> {
        match self {
            Background::Black => Rgba([0, 0, 0, 255]),
            Background::White => Rgba([255, 255, 255, 255]),
            Background::Transparent => Rgba([0, 0, 0, 0]),
        }
    }

    /// Default glyph color when not painting per-cell colors.
    fn ink(self) -> [u8; 3] {
        match self {
            Background::White => [0, 0, 0],
            Background::Black | Background::Transparent => [255, 255, 255],
        }
    }

    /// The ramp inversion that keeps glyph density tracking contrast against
    /// this canvas: on black, bright cells need dense glyphs. Pass this as
    /// [`crate::engine::Options::invert`] when converting for [`paint_png`].
    pub fn default_invert(self) -> bool {
        !matches!(self, Background::White)
    }

    /// The color to composite semi-transparent source pixels over — see
    /// [`crate::engine::Options::matte`]. Transparent output keeps the
    /// source color and carries alpha through instead.
    pub fn matte(self) -> Option<[u8; 3]> {
        match self {
            Background::Black => Some([0, 0, 0]),
            Background::White => Some([255, 255, 255]),
            Background::Transparent => None,
        }
    }
}

/// A pixel rectangle, `x1`/`y1` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Bounds {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }
}

/// How to paint a grid.
#[derive(Debug, Clone)]
pub struct PaintOptions {
    pub background: Background,
    /// Paint each glyph with the cell's average source color.
    pub colored: bool,
    /// Trim blank margins from the finished canvas.
    pub crop: bool,
    /// Cell geometry, sized for the charset that produced the grid.
    pub metrics: CellMetrics,
    /// Glyph size in pixels; `metrics` must have been measured at this size.
    pub font_px: f32,
}

impl PaintOptions {
    /// Measure `charset` in `fonts` and default to white-on-black, cropped.
    pub fn new(fonts: &FontStack, charset: &Charset, font_px: f32) -> Result<Self> {
        if !(1.0..=MAX_FONT_PX).contains(&font_px) {
            bail!("font_px must be between 1 and {MAX_FONT_PX} (got {font_px})");
        }
        Ok(Self {
            background: Background::Black,
            colored: false,
            crop: true,
            metrics: fonts.cell_metrics(charset.glyphs().iter(), font_px),
            font_px,
        })
    }

    pub fn background(mut self, background: Background) -> Self {
        self.background = background;
        self
    }

    pub fn colored(mut self, colored: bool) -> Self {
        self.colored = colored;
        self
    }

    pub fn crop(mut self, crop: bool) -> Self {
        self.crop = crop;
        self
    }
}

/// Paint the grid into an RGBA image.
///
/// `colored` uses each cell's average source color as the glyph color
/// (value-boosted on dark backgrounds, where glyph density already encodes
/// luminance); otherwise glyphs are white-on-black or black-on-white per
/// `background`. Characters no bundled font can draw are left blank. With
/// [`PaintOptions::crop`] the blank margins are trimmed afterwards.
pub fn paint_png(grid: &AsciiGrid, fonts: &FontStack, opts: &PaintOptions) -> Result<RgbaImage> {
    let img = paint_canvas(grid, fonts, opts)?;
    if !opts.crop {
        return Ok(img);
    }
    Ok(match content_bounds(&img, opts.background) {
        Some(bounds) => crop_to(&img, bounds),
        // Nothing was drawn: a 0×0 image is not a file anyone can open.
        None => img,
    })
}

/// Paint the full, uncropped canvas.
///
/// Animations use this directly: every frame has to keep the same dimensions,
/// so they crop with one shared [`Bounds`] instead of per frame.
pub fn paint_canvas(grid: &AsciiGrid, fonts: &FontStack, opts: &PaintOptions) -> Result<RgbaImage> {
    if !(1.0..=MAX_FONT_PX).contains(&opts.font_px) {
        bail!(
            "font_px must be between 1 and {MAX_FONT_PX} (got {})",
            opts.font_px
        );
    }
    let CellMetrics {
        width: cell_w,
        height: cell_h,
        ascent,
    } = opts.metrics;

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
    let mut img = RgbaImage::from_pixel(out_w, out_h, opts.background.fill());
    let scale = PxScale::from(opts.font_px);
    let located = fonts.locate_all(grid.chars.iter());
    let transparent = opts.background == Background::Transparent;

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let ch = grid.char_at(row, col);
            if ch == ' ' {
                continue;
            }
            let ink = if !opts.colored {
                opts.background.ink()
            } else if opts.background == Background::White {
                grid.color_at(row, col)
            } else {
                brighten(grid.color_at(row, col))
            };
            // On a transparent canvas the source alpha has nowhere to hide —
            // it rides on the glyph. Opaque canvases already had it composited
            // into the cell color by the engine's matte.
            let cell_alpha = if transparent {
                grid.alpha_at(row, col) as f32 / 255.0
            } else {
                1.0
            };

            // Clip to the cell so overshooting glyphs (e.g. █ block elements)
            // cannot bleed color into neighboring cells.
            let (cx0, cy0) = ((col * cell_w) as i64, (row * cell_h) as i64);
            let (cx1, cy1) = (
                (cx0 + cell_w as i64).min(out_w as i64),
                (cy0 + cell_h as i64).min(out_h as i64),
            );

            match located.get(&ch) {
                Some(loc) => {
                    let glyph = loc
                        .glyph
                        .with_scale_and_position(scale, point(cx0 as f32, cy0 as f32 + ascent));
                    let Some(outline) = fonts.get(loc.font).outline_glyph(glyph) else {
                        continue;
                    };
                    let bounds = outline.px_bounds();
                    outline.draw(|gx, gy, coverage| {
                        let x = bounds.min.x as i64 + gx as i64;
                        let y = bounds.min.y as i64 + gy as i64;
                        if x < cx0 || y < cy0 || x >= cx1 || y >= cy1 {
                            return;
                        }
                        let alpha = coverage.clamp(0.0, 1.0) * cell_alpha;
                        blend(&mut img, x as u32, y as u32, ink, alpha, transparent);
                    });
                }
                // No bundled font draws braille, and a tofu box in every cell
                // would destroy the ramp — so plot the dots ourselves.
                None if is_braille(ch) => draw_braille(
                    &mut img,
                    (cx0 as u32, cy0 as u32, cell_w, cell_h),
                    (ch as u32 - 0x2800) as u8,
                    ink,
                    cell_alpha,
                    transparent,
                ),
                // Anything else the fonts cannot draw stays blank.
                None => continue,
            }
        }
    }

    Ok(img)
}

/// Paint one pixel of ink at `alpha` coverage.
fn blend(img: &mut RgbaImage, x: u32, y: u32, ink: [u8; 3], alpha: f32, transparent: bool) {
    let pixel = img.get_pixel_mut(x, y);
    if transparent {
        // Glyphs are clipped to their own cell, so no two ever touch the same
        // pixel: straight alpha, no compositing.
        pixel.0 = [ink[0], ink[1], ink[2], (alpha * 255.0).round() as u8];
    } else {
        for (px, &fg) in pixel.0.iter_mut().take(3).zip(ink.iter()) {
            let bg = *px as f32;
            *px = (bg + (fg as f32 - bg) * alpha) as u8;
        }
    }
}

/// Is this one of the 256 braille patterns?
fn is_braille(ch: char) -> bool {
    ('\u{2800}'..='\u{28FF}').contains(&ch)
}

/// Fraction of a dot's radius used for the drawn disc. Below ~0.5 the dots
/// read as a halftone screen; at 0.5 they just touch.
const DOT_FILL: f32 = 0.48;

/// Plot a braille cell as eight possible dots on a 2×4 grid.
fn draw_braille(
    img: &mut RgbaImage,
    cell: (u32, u32, u32, u32),
    bits: u8,
    ink: [u8; 3],
    cell_alpha: f32,
    transparent: bool,
) {
    let (cx, cy, cell_w, cell_h) = cell;
    let (sub_w, sub_h) = (cell_w as f32 / 2.0, cell_h as f32 / 4.0);
    let radius = (sub_w.min(sub_h) * DOT_FILL).max(0.5);

    for (i, bit) in [0x01u8, 0x08, 0x02, 0x10, 0x04, 0x20, 0x40, 0x80]
        .into_iter()
        .enumerate()
    {
        if bits & bit == 0 {
            continue;
        }
        let (dx, dy) = (i as u32 % 2, i as u32 / 2);
        let center = (
            cx as f32 + (dx as f32 + 0.5) * sub_w,
            cy as f32 + (dy as f32 + 0.5) * sub_h,
        );
        // Only the pixels the disc can reach, clamped to the canvas.
        let x0 = (center.0 - radius - 1.0).floor().max(cx as f32) as u32;
        let x1 = ((center.0 + radius + 1.0).ceil() as u32)
            .min(cx + cell_w)
            .min(img.width());
        let y0 = (center.1 - radius - 1.0).floor().max(cy as f32) as u32;
        let y1 = ((center.1 + radius + 1.0).ceil() as u32)
            .min(cy + cell_h)
            .min(img.height());
        for y in y0..y1 {
            for x in x0..x1 {
                let d = ((x as f32 + 0.5 - center.0).powi(2) + (y as f32 + 0.5 - center.1).powi(2))
                    .sqrt();
                // One pixel of feathering at the rim keeps dots from aliasing
                // into squares at small font sizes.
                let coverage = (radius + 0.5 - d).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    blend(img, x, y, ink, coverage * cell_alpha, transparent);
                }
            }
        }
    }
}

/// Bounding box of everything that is not background, or `None` for a canvas
/// that stayed entirely blank.
pub fn content_bounds(img: &RgbaImage, background: Background) -> Option<Bounds> {
    let fill = background.fill();
    let (mut x0, mut y0) = (u32::MAX, u32::MAX);
    let (mut x1, mut y1) = (0u32, 0u32);
    for (x, y, px) in img.enumerate_pixels() {
        let drawn = match background {
            Background::Transparent => px.0[3] != 0,
            _ => px.0 != fill.0,
        };
        if drawn {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    (x0 != u32::MAX).then_some(Bounds { x0, y0, x1, y1 })
}

/// Copy the `bounds` rectangle out of `img`.
pub fn crop_to(img: &RgbaImage, bounds: Bounds) -> RgbaImage {
    RgbaImage::from_fn(bounds.width(), bounds.height(), |x, y| {
        *img.get_pixel(bounds.x0 + x, bounds.y0 + y)
    })
}

/// Value-normalize a cell color: keep the hue, push the brightest channel
/// toward full intensity. On dark backgrounds glyph density already encodes
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
    use crate::charset;
    use crate::engine::{convert, Options};
    use image::Rgba;

    fn fonts() -> &'static FontStack {
        crate::font::embedded()
    }

    fn simple() -> Charset {
        charset::resolve("simple").unwrap()
    }

    fn opts(background: Background) -> PaintOptions {
        PaintOptions::new(fonts(), &simple(), 16.0)
            .unwrap()
            .background(background)
            .crop(false)
    }

    fn grid_of(src: &RgbaImage, width: u32, invert: bool) -> AsciiGrid {
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

    fn flat(v: u8) -> RgbaImage {
        RgbaImage::from_pixel(64, 64, Rgba([v, v, v, 255]))
    }

    #[test]
    fn paints_expected_dimensions_and_ink() {
        let grid = grid_of(&flat(0), 8, false);
        let img = paint_png(&grid, fonts(), &opts(Background::Black)).unwrap();
        assert_eq!(img.width() % grid.cols, 0);
        assert_eq!(img.height() % grid.rows, 0);
        // A black source maps to '@' glyphs — some pixels must be lit.
        assert!(img.pixels().any(|p| p.0[0] > 128));
        assert!(
            img.pixels().all(|p| p.0[3] == 255),
            "opaque bg stays opaque"
        );
    }

    #[test]
    fn white_source_paints_empty_canvas() {
        let grid = grid_of(&flat(255), 8, false);
        let img = paint_png(&grid, fonts(), &opts(Background::Black)).unwrap();
        assert!(img.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn colored_on_white_keeps_dark_ink_dark() {
        // Regression: brighten() must not apply on white backgrounds, or dark
        // grays become near-invisible pastels.
        let grid = grid_of(&flat(20), 8, false);
        let img = paint_png(&grid, fonts(), &opts(Background::White).colored(true)).unwrap();
        let darkest = img.pixels().map(|p| p.0[0]).min().unwrap();
        assert!(darkest <= 30, "dark source ink washed out: {darkest}");
    }

    #[test]
    fn rejects_invalid_font_px() {
        for bad in [0.0, -5.0, f32::NAN, 4e7] {
            assert!(PaintOptions::new(fonts(), &simple(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_absurd_canvases() {
        let grid = AsciiGrid {
            cols: 100_000,
            rows: 100_000,
            chars: vec![' '],
            colors: vec![[0, 0, 0]],
            alphas: vec![255],
        };
        let err = paint_canvas(&grid, fonts(), &opts(Background::Black)).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn missing_glyphs_paint_blank_not_tofu() {
        let opts_charset = Options {
            width: 8,
            // U+E000 is private-use: no bundled font maps it.
            charset: Charset::Ramp(vec!['\u{E000}', ' ']),
            ..Options::default()
        };
        let grid = convert(&flat(0), &opts_charset).unwrap();
        let paint = PaintOptions::new(fonts(), &opts_charset.charset, 16.0)
            .unwrap()
            .crop(false);
        let img = paint_png(&grid, fonts(), &paint).unwrap();
        assert!(img.pixels().all(|p| p.0 == [0, 0, 0, 255]));
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
            alphas: vec![255; 2],
        };
        let blocks = charset::resolve("blocks").unwrap();
        let paint = PaintOptions::new(fonts(), &blocks, 16.0)
            .unwrap()
            .background(Background::White)
            .crop(false);
        let img = paint_png(&grid, fonts(), &paint).unwrap();
        let cell_w = img.width() / 2;
        // Every pixel of cell 0 (including its rightmost column, where the
        // neighbor's overshoot would land) must remain pure background.
        for y in 0..img.height() {
            for x in 0..cell_w {
                assert_eq!(
                    img.get_pixel(x, y).0,
                    [255, 255, 255, 255],
                    "bleed at {x},{y}"
                );
            }
        }
        // Sanity: the glyph itself did paint something in cell 1.
        assert!(img.pixels().any(|p| p.0 != [255, 255, 255, 255]));
    }

    // --- cropping -------------------------------------------------------

    #[test]
    fn crop_trims_blank_margins() {
        // Content only in the middle third: cropping must shrink the canvas
        // but keep every drawn pixel.
        let mut src = RgbaImage::from_pixel(64, 64, Rgba([255, 255, 255, 255]));
        for y in 24..40 {
            for x in 24..40 {
                src.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let grid = grid_of(&src, 16, false);
        let full = paint_png(&grid, fonts(), &opts(Background::Black)).unwrap();
        let cropped = paint_png(&grid, fonts(), &opts(Background::Black).crop(true)).unwrap();
        assert!(cropped.width() < full.width(), "width not trimmed");
        assert!(cropped.height() < full.height(), "height not trimmed");
        let lit = |img: &RgbaImage| img.pixels().filter(|p| p.0[0] > 0).count();
        assert_eq!(lit(&full), lit(&cropped), "cropping lost drawn pixels");
    }

    #[test]
    fn crop_keeps_a_blank_canvas_intact() {
        let grid = grid_of(&flat(255), 8, false);
        let img = paint_png(&grid, fonts(), &opts(Background::Black).crop(true)).unwrap();
        assert!(img.width() > 0 && img.height() > 0);
        assert!(content_bounds(&img, Background::Black).is_none());
    }

    #[test]
    fn content_bounds_finds_the_drawn_box() {
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
        img.put_pixel(3, 4, Rgba([255, 255, 255, 255]));
        assert_eq!(
            content_bounds(&img, Background::Black),
            Some(Bounds {
                x0: 3,
                y0: 4,
                x1: 4,
                y1: 5
            })
        );
        let empty = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
        assert_eq!(content_bounds(&empty, Background::Transparent), None);
    }

    // --- transparency ---------------------------------------------------

    #[test]
    fn transparent_background_only_inks_glyphs() {
        let grid = grid_of(&flat(0), 8, false);
        let img = paint_png(&grid, fonts(), &opts(Background::Transparent)).unwrap();
        assert!(img.pixels().any(|p| p.0[3] == 0), "no transparent pixels");
        assert!(img.pixels().any(|p| p.0[3] > 200), "no opaque glyph pixels");
        // Every visible pixel carries the ink color, not a dark halo.
        assert!(img
            .pixels()
            .filter(|p| p.0[3] > 0)
            .all(|p| p.0[..3] == [255, 255, 255]));
    }

    #[test]
    fn transparent_cells_stay_invisible() {
        let mut src = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 0]));
        for y in 0..32 {
            for x in 0..16 {
                src.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let grid = convert(
            &src,
            &Options {
                width: 8,
                matte: None,
                ..Options::default()
            },
        )
        .unwrap();
        let img = paint_png(&grid, fonts(), &opts(Background::Transparent)).unwrap();
        // The right half of the canvas came from transparent cells.
        let right_half_lit = (img.width() / 2..img.width())
            .flat_map(|x| (0..img.height()).map(move |y| (x, y)))
            .any(|(x, y)| img.get_pixel(x, y).0[3] != 0);
        assert!(!right_half_lit, "transparent cells painted ink");
    }

    // --- font fallback --------------------------------------------------

    #[cfg(feature = "cjk")]
    #[test]
    fn cjk_charsets_paint_real_glyphs() {
        let chinese = charset::resolve("chinese").unwrap();
        let paint = PaintOptions::new(fonts(), &chinese, 16.0)
            .unwrap()
            .crop(false);
        let grid = convert(
            &flat(0),
            &Options {
                width: 8,
                charset: chinese,
                ..Options::default()
            },
        )
        .unwrap();
        let img = paint_png(&grid, fonts(), &paint).unwrap();
        let lit = img.pixels().filter(|p| p.0[0] > 128).count();
        assert!(lit > 100, "CJK charset painted {lit} pixels — blanks?");
    }

    #[test]
    fn braille_paints_from_the_primary_font() {
        let grid = convert(
            &flat(0),
            &Options {
                width: 8,
                charset: Charset::Braille,
                ..Options::default()
            },
        )
        .unwrap();
        let paint = PaintOptions::new(fonts(), &Charset::Braille, 16.0)
            .unwrap()
            .crop(false);
        let img = paint_png(&grid, fonts(), &paint).unwrap();
        assert!(img.pixels().filter(|p| p.0[0] > 128).count() > 50);
    }
}
