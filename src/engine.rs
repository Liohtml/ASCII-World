//! Core conversion: image → grid of (character, average color) cells.

use crate::charset::Charset;
use anyhow::{bail, Result};
use image::RgbaImage;

/// Cells whose mean alpha falls below this are blank by default — low enough
/// to keep antialiased edges, high enough to erase a transparent background.
pub const DEFAULT_ALPHA_THRESHOLD: u8 = 64;

/// Braille cells are two dots wide and four tall.
const DOT_COLS: u32 = 2;
const DOT_ROWS: u32 = 4;

/// Conversion parameters.
#[derive(Debug, Clone)]
pub struct Options {
    /// Output width in characters (columns). Clamped to the image width.
    pub width: u32,
    /// How cells become characters: a dark → light ramp, or braille dots.
    pub charset: Charset,
    /// Flip the ramp (useful for light terminals / white backgrounds).
    pub invert: bool,
    /// Cell height as a multiple of cell width. Terminal glyphs are roughly
    /// twice as tall as wide, so 2.0 preserves the image's aspect ratio.
    pub aspect: f32,
    /// Cells whose mean alpha is below this render blank, whatever the
    /// charset or `invert` say — that is what keeps a cutout a cutout.
    pub alpha_threshold: u8,
    /// Composite semi-transparent pixels over this color before sampling, so
    /// edge tones match the surface the art lands on. `None` keeps the
    /// alpha-weighted source color, for transparent output targets.
    pub matte: Option<[u8; 3]>,
    /// Hard luma cutoff for braille dots. `None` dithers instead, which keeps
    /// sub-cell detail in flat regions; set a value for crisp, predictable
    /// two-tone output. Ignored by ramp charsets.
    pub braille_threshold: Option<u8>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 100,
            charset: Charset::Ramp(crate::charset::SIMPLE.chars().collect()),
            invert: false,
            aspect: 2.0,
            alpha_threshold: DEFAULT_ALPHA_THRESHOLD,
            matte: Some([255, 255, 255]),
            braille_threshold: None,
        }
    }
}

/// The result of a conversion: a row-major grid of characters plus the
/// average color and alpha of each source cell (used by ANSI/PNG rendering).
#[derive(Debug, Clone)]
pub struct AsciiGrid {
    pub cols: u32,
    pub rows: u32,
    pub chars: Vec<char>,
    pub colors: Vec<[u8; 3]>,
    /// Mean source alpha per cell; 255 everywhere for opaque inputs.
    pub alphas: Vec<u8>,
}

impl AsciiGrid {
    pub fn char_at(&self, row: u32, col: u32) -> char {
        self.chars[(row * self.cols + col) as usize]
    }

    pub fn color_at(&self, row: u32, col: u32) -> [u8; 3] {
        self.colors[(row * self.cols + col) as usize]
    }

    pub fn alpha_at(&self, row: u32, col: u32) -> u8 {
        self.alphas[(row * self.cols + col) as usize]
    }

    /// Whether any cell carries partial transparency.
    pub fn has_alpha(&self) -> bool {
        self.alphas.iter().any(|&a| a < u8::MAX)
    }
}

/// Cell geometry shared by both sampling modes.
struct Geometry {
    img_w: u32,
    img_h: u32,
    cols: u32,
    rows: u32,
    cell_w: f64,
    cell_h: f64,
}

impl Geometry {
    fn new(img_w: u32, img_h: u32, opts: &Options) -> Self {
        let cols = opts.width.clamp(1, img_w);
        let cell_w = img_w as f64 / cols as f64;
        let cell_h = (cell_w * opts.aspect as f64).max(1.0);
        let rows = ((img_h as f64 / cell_h) as u32).max(1);
        Self {
            img_w,
            img_h,
            cols,
            rows,
            cell_w,
            cell_h,
        }
    }

    /// Source-pixel span of a cell, always at least one pixel wide/tall.
    fn cell_bounds(&self, row: u32, col: u32) -> (u32, u32, u32, u32) {
        let x0 = (col as f64 * self.cell_w) as u32;
        let x1 = (((col + 1) as f64 * self.cell_w) as u32)
            .min(self.img_w)
            .max(x0 + 1);
        let y0 = (row as f64 * self.cell_h) as u32;
        let y1 = (((row + 1) as f64 * self.cell_h) as u32)
            .min(self.img_h)
            .max(y0 + 1);
        (x0, x1, y0, y1)
    }
}

/// Mean color, alpha and luma of one rectangle of source pixels.
#[derive(Debug, Clone, Copy)]
struct Sample {
    color: [u8; 3],
    alpha: u8,
    luma: f64,
}

/// Average a pixel rectangle, compositing over `matte` when it is set.
///
/// Colors are summed premultiplied by alpha, so a transparent pixel cannot
/// drag the mean toward whatever RGB happens to sit behind it — a very common
/// artifact, since fully transparent pixels are usually stored as black.
fn sample(image: &RgbaImage, bounds: (u32, u32, u32, u32), matte: Option<[u8; 3]>) -> Sample {
    let (x0, x1, y0, y1) = bounds;
    let stride = image.width() as usize * 4;
    let raw = image.as_raw();

    let mut sum = [0u64; 3];
    let mut sum_a = 0u64;
    for y in y0..y1 {
        let start = y as usize * stride + x0 as usize * 4;
        let end = y as usize * stride + x1 as usize * 4;
        for p in raw[start..end].chunks_exact(4) {
            let a = p[3] as u64;
            sum[0] += p[0] as u64 * a;
            sum[1] += p[1] as u64 * a;
            sum[2] += p[2] as u64 * a;
            sum_a += a;
        }
    }

    let count = (y1 - y0) as u64 * (x1 - x0) as u64;
    let alpha = (sum_a / count) as u8;
    let color = match matte {
        // Mean of every pixel composited over the matte:
        // (c·a + m·(255 − a)) / 255, averaged over the cell.
        Some(m) => {
            let clear = count * 255 - sum_a;
            [0, 1, 2].map(|i| ((sum[i] + m[i] as u64 * clear) / (count * 255)) as u8)
        }
        // Alpha-weighted mean: the color the visible part of the cell has.
        None if sum_a > 0 => [0, 1, 2].map(|i| (sum[i] / sum_a) as u8),
        None => [0, 0, 0],
    };
    // Rec. 601 luma — perceptually better than a plain channel mean.
    let luma = 0.299 * color[0] as f64 + 0.587 * color[1] as f64 + 0.114 * color[2] as f64;
    Sample { color, alpha, luma }
}

/// One row of output cells.
struct Row {
    chars: Vec<char>,
    colors: Vec<[u8; 3]>,
    alphas: Vec<u8>,
}

/// Map over rows in parallel when the `parallel` feature is on.
///
/// `map()` on an indexed parallel iterator is order-preserving, so output is
/// byte-identical to the sequential path — the parallelism is invisible
/// except in wall-clock time.
#[cfg(feature = "parallel")]
fn map_rows<T, F>(rows: u32, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(u32) -> T + Send + Sync,
{
    use rayon::prelude::*;
    (0..rows).into_par_iter().map(f).collect()
}

#[cfg(not(feature = "parallel"))]
fn map_rows<T, F>(rows: u32, f: F) -> Vec<T>
where
    F: Fn(u32) -> T,
{
    (0..rows).map(f).collect()
}

fn assemble(rows: Vec<Row>, cols: u32) -> AsciiGrid {
    let n = rows.len() * cols as usize;
    let mut grid = AsciiGrid {
        cols,
        rows: rows.len() as u32,
        chars: Vec::with_capacity(n),
        colors: Vec::with_capacity(n),
        alphas: Vec::with_capacity(n),
    };
    for row in rows {
        grid.chars.extend(row.chars);
        grid.colors.extend(row.colors);
        grid.alphas.extend(row.alphas);
    }
    grid
}

/// Convert an RGBA image into an [`AsciiGrid`].
///
/// Each output cell averages a `cell_width × cell_height` block of source
/// pixels; the block's luma picks a character and the block's mean RGB
/// becomes the cell color. With [`Charset::Braille`] each cell is instead
/// sampled as a 2×4 dot grid and composed into one braille pattern.
///
/// Cells whose mean alpha is below [`Options::alpha_threshold`] come out
/// blank, so images with transparent backgrounds stay cutouts.
pub fn convert(image: &RgbaImage, opts: &Options) -> Result<AsciiGrid> {
    let (img_w, img_h) = image.dimensions();
    if img_w == 0 || img_h == 0 {
        bail!("input image is empty");
    }
    if let Some(ramp) = opts.charset.ramp() {
        if ramp.len() < 2 {
            bail!("charset needs at least 2 characters");
        }
    }
    if !opts.aspect.is_finite() || opts.aspect <= 0.0 {
        bail!("aspect must be a positive, finite number");
    }

    let geom = Geometry::new(img_w, img_h, opts);
    Ok(match &opts.charset {
        Charset::Ramp(ramp) => convert_ramp(image, opts, &geom, ramp),
        Charset::Braille => convert_braille(image, opts, &geom),
    })
}

fn convert_ramp(image: &RgbaImage, opts: &Options, geom: &Geometry, ramp: &[char]) -> AsciiGrid {
    let n = ramp.len();
    let rows = map_rows(geom.rows, |row| {
        let mut out = Row {
            chars: Vec::with_capacity(geom.cols as usize),
            colors: Vec::with_capacity(geom.cols as usize),
            alphas: Vec::with_capacity(geom.cols as usize),
        };
        for col in 0..geom.cols {
            let s = sample(image, geom.cell_bounds(row, col), opts.matte);
            let ch = if s.alpha < opts.alpha_threshold {
                ' '
            } else {
                let mut idx = ((s.luma * n as f64 / 255.0) as usize).min(n - 1);
                if opts.invert {
                    idx = n - 1 - idx;
                }
                ramp[idx]
            };
            out.chars.push(ch);
            out.colors.push(s.color);
            out.alphas.push(s.alpha);
        }
        out
    });
    assemble(rows, geom.cols)
}

/// Sub-cell sampling: every cell becomes a 2×4 grid of dots, each turned on
/// or off, then packed into a braille codepoint.
fn convert_braille(image: &RgbaImage, opts: &Options, geom: &Geometry) -> AsciiGrid {
    let dot_w = geom.cols * DOT_COLS;
    let dot_h = geom.rows * DOT_ROWS;

    // Pass 1: sample every dot, in raster order across the whole dot grid.
    // Sub-rectangles are derived from the cell bounds, so dots stay inside
    // their cell no matter how the cell rounded.
    let bands: Vec<Vec<Sample>> = map_rows(geom.rows, |row| {
        let mut band = Vec::with_capacity((dot_w * DOT_ROWS) as usize);
        for dy in 0..DOT_ROWS {
            for col in 0..geom.cols {
                let (x0, x1, y0, y1) = geom.cell_bounds(row, col);
                let (w, h) = ((x1 - x0) as f64, (y1 - y0) as f64);
                let sy0 = y0 + (dy as f64 * h / DOT_ROWS as f64) as u32;
                let sy1 = (y0 + ((dy + 1) as f64 * h / DOT_ROWS as f64) as u32)
                    .min(y1)
                    .max(sy0 + 1);
                for dx in 0..DOT_COLS {
                    let sx0 = x0 + (dx as f64 * w / DOT_COLS as f64) as u32;
                    let sx1 = (x0 + ((dx + 1) as f64 * w / DOT_COLS as f64) as u32)
                        .min(x1)
                        .max(sx0 + 1);
                    band.push(sample(image, (sx0, sx1, sy0, sy1), opts.matte));
                }
            }
        }
        band
    });
    let dots: Vec<Sample> = bands.into_iter().flatten().collect();

    // Pass 2: decide which dots are ink.
    let lit = match opts.braille_threshold {
        Some(t) => dots
            .iter()
            .map(|d| d.alpha >= opts.alpha_threshold && is_ink(d.luma, t as f64, opts.invert))
            .collect(),
        None => dither(&dots, dot_w, dot_h, opts),
    };

    // Pass 3: pack each cell's eight dots into one codepoint.
    let rows = map_rows(geom.rows, |row| {
        let mut out = Row {
            chars: Vec::with_capacity(geom.cols as usize),
            colors: Vec::with_capacity(geom.cols as usize),
            alphas: Vec::with_capacity(geom.cols as usize),
        };
        for col in 0..geom.cols {
            let mut bits = 0u8;
            let mut sum = [0u32; 3];
            let mut sum_a = 0u32;
            for dy in 0..DOT_ROWS {
                for dx in 0..DOT_COLS {
                    let i = ((row * DOT_ROWS + dy) * dot_w + col * DOT_COLS + dx) as usize;
                    if lit[i] {
                        bits |= dot_bit(dx, dy);
                    }
                    for (c, channel) in sum.iter_mut().enumerate() {
                        *channel += dots[i].color[c] as u32;
                    }
                    sum_a += dots[i].alpha as u32;
                }
            }
            let n = DOT_COLS * DOT_ROWS;
            out.chars
                .push(if bits == 0 { ' ' } else { braille_char(bits) });
            out.colors.push([0, 1, 2].map(|i| (sum[i] / n) as u8));
            out.alphas.push((sum_a / n) as u8);
        }
        out
    });
    assemble(rows, geom.cols)
}

/// Is this dot dark enough (or, inverted, bright enough) to be ink?
fn is_ink(luma: f64, threshold: f64, invert: bool) -> bool {
    if invert {
        luma > threshold
    } else {
        luma < threshold
    }
}

/// Floyd–Steinberg error diffusion over the dot grid.
///
/// A single global cutoff would flatten every region that sits well to one
/// side of it — exactly the regions where braille's eight samples per cell
/// should be paying off. Diffusing the quantization error keeps local
/// structure *and* average tone, at the cost of a sequential pass (cheap: the
/// dot grid is a few thousand values, while sampling stayed parallel).
fn dither(dots: &[Sample], w: u32, h: u32, opts: &Options) -> Vec<bool> {
    // Quantizing at the midpoint of the two output levels is what makes the
    // dithered result preserve mean brightness.
    const PIVOT: f32 = 128.0;
    const NEIGHBORS: [(i64, i64, f32); 4] = [
        (1, 0, 7.0 / 16.0),
        (-1, 1, 3.0 / 16.0),
        (0, 1, 5.0 / 16.0),
        (1, 1, 1.0 / 16.0),
    ];

    let mut level: Vec<f32> = dots
        .iter()
        .map(|d| {
            let luma = d.luma as f32;
            if opts.invert {
                255.0 - luma
            } else {
                luma
            }
        })
        .collect();
    let mut lit = vec![false; dots.len()];

    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            // Transparent dots are not ink and have no error to pass on.
            if dots[i].alpha < opts.alpha_threshold {
                continue;
            }
            let old = level[i];
            let on = old < PIVOT;
            lit[i] = on;
            let error = old - if on { 0.0 } else { 255.0 };
            for (dx, dy, weight) in NEIGHBORS {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                level[(ny as u32 * w + nx as u32) as usize] += error * weight;
            }
        }
    }
    lit
}

/// Bit for the dot at column `dx` (0..2), row `dy` (0..4).
///
/// Braille numbers its dots down the left column then down the right, with
/// the fourth row bolted on afterwards — hence the jump to 0x40/0x80.
fn dot_bit(dx: u32, dy: u32) -> u8 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        _ => 0x80,
    }
}

/// The braille pattern (U+2800–U+28FF) for a set of dot bits.
fn braille_char(bits: u8) -> char {
    char::from_u32(0x2800 + bits as u32).expect("braille block is fully assigned")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::charset;
    use image::Rgba;

    fn flat_image(w: u32, h: u32, value: u8) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([value, value, value, 255]))
    }

    fn simple() -> Options {
        Options::default()
    }

    #[test]
    fn black_image_maps_to_densest_char() {
        let grid = convert(&flat_image(100, 100, 0), &simple()).unwrap();
        assert!(grid.chars.iter().all(|&c| c == '@'));
    }

    #[test]
    fn white_image_maps_to_space() {
        let grid = convert(&flat_image(100, 100, 255), &simple()).unwrap();
        assert!(grid.chars.iter().all(|&c| c == ' '));
    }

    #[test]
    fn invert_flips_ramp() {
        let opts = Options {
            invert: true,
            ..simple()
        };
        let grid = convert(&flat_image(100, 100, 0), &opts).unwrap();
        assert!(grid.chars.iter().all(|&c| c == ' '));
    }

    #[test]
    fn grid_dimensions_follow_width_and_aspect() {
        let grid = convert(&flat_image(200, 200, 128), &simple()).unwrap();
        assert_eq!(grid.cols, 100);
        assert_eq!(grid.rows, 50); // aspect 2.0 → cells twice as tall
        assert_eq!(grid.chars.len(), (grid.cols * grid.rows) as usize);
        assert_eq!(grid.colors.len(), grid.chars.len());
        assert_eq!(grid.alphas.len(), grid.chars.len());
    }

    #[test]
    fn width_is_clamped_to_image_width() {
        let opts = Options {
            width: 5000,
            ..simple()
        };
        let grid = convert(&flat_image(64, 64, 128), &opts).unwrap();
        assert_eq!(grid.cols, 64);
    }

    #[test]
    fn colors_average_the_cell() {
        let img = RgbaImage::from_pixel(10, 10, Rgba([200, 100, 50, 255]));
        let opts = Options {
            width: 1,
            ..simple()
        };
        let grid = convert(&img, &opts).unwrap();
        assert_eq!(grid.color_at(0, 0), [200, 100, 50]);
        assert_eq!(grid.alpha_at(0, 0), 255);
        assert!(!grid.has_alpha());
    }

    #[test]
    fn gradient_is_monotonic() {
        // Left-to-right black→white gradient must produce a ramp that never
        // gets darker as brightness increases.
        let mut img = RgbaImage::new(256, 32);
        for (x, _, p) in img.enumerate_pixels_mut() {
            let v = x as u8;
            *p = Rgba([v, v, v, 255]);
        }
        let opts = Options {
            width: 16,
            ..simple()
        };
        let grid = convert(&img, &opts).unwrap();
        let ramp: Vec<char> = opts.charset.ramp().unwrap().to_vec();
        let mut last_pos = 0usize;
        for col in 0..grid.cols {
            let c = grid.char_at(0, col);
            let pos = ramp.iter().position(|&r| r == c).unwrap();
            assert!(pos >= last_pos, "ramp went darker at col {col}");
            last_pos = pos;
        }
    }

    #[test]
    fn rejects_degenerate_input() {
        assert!(convert(&RgbaImage::new(0, 4), &simple()).is_err());
        let bad_charset = Options {
            charset: charset::Charset::Ramp(vec!['x']),
            ..simple()
        };
        assert!(convert(&flat_image(8, 8, 0), &bad_charset).is_err());
        for aspect in [0.0, -1.0, f32::NAN] {
            let opts = Options { aspect, ..simple() };
            assert!(convert(&flat_image(8, 8, 0), &opts).is_err(), "{aspect}");
        }
    }

    // --- transparency ---------------------------------------------------

    fn cutout() -> RgbaImage {
        // Opaque mid-dark square in the middle, transparent around it. The
        // subject is deliberately neither black nor white so that no ramp
        // direction can map it onto a space by accident.
        let mut img = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        for y in 16..48 {
            for x in 16..48 {
                img.put_pixel(x, y, Rgba([100, 100, 100, 255]));
            }
        }
        img
    }

    #[test]
    fn transparent_cells_are_blank_whatever_the_ramp_says() {
        for invert in [false, true] {
            for matte in [None, Some([0, 0, 0]), Some([255, 255, 255])] {
                let opts = Options {
                    width: 16,
                    invert,
                    matte,
                    ..simple()
                };
                let grid = convert(&cutout(), &opts).unwrap();
                assert_eq!(
                    grid.char_at(0, 0),
                    ' ',
                    "corner not blank ({invert}, {matte:?})"
                );
                assert_eq!(grid.alpha_at(0, 0), 0);
                // ...and the opaque middle still draws something.
                let mid = grid.char_at(grid.rows / 2, grid.cols / 2);
                assert_ne!(mid, ' ', "cutout lost its subject ({invert}, {matte:?})");
            }
        }
    }

    #[test]
    fn transparent_pixels_do_not_darken_the_mean_color() {
        // A red logo on a transparent (stored as black) background: the
        // half-covered cell must stay red, not turn maroon.
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 0]));
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let opts = Options {
            width: 1,
            matte: None,
            ..simple()
        };
        let grid = convert(&img, &opts).unwrap();
        assert_eq!(grid.color_at(0, 0), [255, 0, 0]);
        assert_eq!(grid.alpha_at(0, 0), 127);
    }

    #[test]
    fn matte_composites_edge_tones() {
        // Half-transparent black over white reads as mid-gray.
        let img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 128]));
        let opts = Options {
            width: 1,
            matte: Some([255, 255, 255]),
            alpha_threshold: 0,
            ..simple()
        };
        let grid = convert(&img, &opts).unwrap();
        assert_eq!(grid.color_at(0, 0), [127, 127, 127]);
    }

    #[test]
    fn opaque_images_are_unaffected_by_the_matte() {
        // Regression guard: alpha support must not change existing output.
        let img = RgbaImage::from_pixel(32, 32, Rgba([90, 140, 200, 255]));
        let opaque = convert(&img, &simple()).unwrap();
        let no_matte = convert(
            &img,
            &Options {
                matte: None,
                ..simple()
            },
        )
        .unwrap();
        assert_eq!(opaque.colors, no_matte.colors);
        assert_eq!(opaque.chars, no_matte.chars);
    }

    #[test]
    fn alpha_threshold_is_configurable() {
        let img = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 100]));
        let blank = convert(
            &img,
            &Options {
                width: 1,
                alpha_threshold: 200,
                ..simple()
            },
        )
        .unwrap();
        assert_eq!(blank.char_at(0, 0), ' ');
        let drawn = convert(
            &img,
            &Options {
                width: 1,
                alpha_threshold: 50,
                matte: Some([0, 0, 0]),
                ..simple()
            },
        )
        .unwrap();
        assert_ne!(drawn.char_at(0, 0), ' ');
    }

    // --- braille --------------------------------------------------------

    fn braille_opts(width: u32) -> Options {
        Options {
            width,
            charset: Charset::Braille,
            ..Options::default()
        }
    }

    #[test]
    fn braille_black_fills_every_dot() {
        let grid = convert(&flat_image(64, 64, 0), &braille_opts(8)).unwrap();
        assert!(
            grid.chars.iter().all(|&c| c == '\u{28FF}'),
            "{:?}",
            grid.chars
        );
    }

    #[test]
    fn braille_white_stays_empty() {
        let grid = convert(&flat_image(64, 64, 255), &braille_opts(8)).unwrap();
        assert!(grid.chars.iter().all(|&c| c == ' '));
    }

    #[test]
    fn braille_resolves_detail_inside_one_cell() {
        // A single cell whose left half is black and right half white must
        // come out as the left-column dots only — a ramp charset could not
        // express this at all.
        let mut img = RgbaImage::from_pixel(2, 4, Rgba([255, 255, 255, 255]));
        for y in 0..4 {
            img.put_pixel(0, y, Rgba([0, 0, 0, 255]));
        }
        let opts = Options {
            width: 1,
            aspect: 2.0,
            ..braille_opts(1)
        };
        let grid = convert(&img, &opts).unwrap();
        assert_eq!(grid.cols, 1);
        // dots 1,2,3,7 = 0x01|0x02|0x04|0x40 = 0x47
        assert_eq!(grid.char_at(0, 0), '\u{2847}');
    }

    #[test]
    fn braille_invert_flips_which_dots_light_up() {
        let mut img = RgbaImage::from_pixel(2, 4, Rgba([255, 255, 255, 255]));
        for y in 0..4 {
            img.put_pixel(0, y, Rgba([0, 0, 0, 255]));
        }
        let opts = Options {
            invert: true,
            ..braille_opts(1)
        };
        let grid = convert(&img, &opts).unwrap();
        // Now the *white* half lights up: dots 4,5,6,8 = 0xB8
        assert_eq!(grid.char_at(0, 0), '\u{28B8}');
    }

    #[test]
    fn braille_honours_transparency_and_manual_threshold() {
        let grid = convert(&cutout(), &braille_opts(16)).unwrap();
        assert_eq!(grid.char_at(0, 0), ' ');
        assert_ne!(grid.char_at(grid.rows / 2, grid.cols / 2), ' ');

        // A manual threshold below every luma turns everything off.
        let opts = Options {
            braille_threshold: Some(0),
            ..braille_opts(8)
        };
        let grid = convert(&flat_image(64, 64, 10), &opts).unwrap();
        assert!(grid.chars.iter().all(|&c| c == ' '));
    }

    #[test]
    fn dithering_preserves_average_tone() {
        // A flat 25%-gray field must come out roughly 75% inked: that is what
        // error diffusion buys over a hard threshold, which would ink all of
        // it or none of it.
        let grid = convert(&flat_image(128, 128, 64), &braille_opts(16)).unwrap();
        let dots: usize = grid
            .chars
            .iter()
            .map(|&c| {
                if c == ' ' {
                    0
                } else {
                    (c as u32 - 0x2800).count_ones() as usize
                }
            })
            .sum();
        let coverage = dots as f32 / (grid.chars.len() * 8) as f32;
        assert!(
            (0.65..0.85).contains(&coverage),
            "25% gray dithered to {coverage:.2} ink coverage"
        );
    }

    #[test]
    fn a_hard_threshold_disables_dithering() {
        // Same flat field, explicit cutoff: strictly all-or-nothing.
        let opts = Options {
            braille_threshold: Some(128),
            ..braille_opts(16)
        };
        let grid = convert(&flat_image(128, 128, 64), &opts).unwrap();
        assert!(grid.chars.iter().all(|&c| c == '\u{28FF}'));
    }

    #[test]
    fn dot_bits_cover_the_whole_pattern() {
        let mut all = 0u8;
        for dy in 0..DOT_ROWS {
            for dx in 0..DOT_COLS {
                let bit = dot_bit(dx, dy);
                assert_eq!(all & bit, 0, "duplicate bit for ({dx},{dy})");
                all |= bit;
            }
        }
        assert_eq!(all, 0xFF);
        assert_eq!(braille_char(0), '\u{2800}');
        assert_eq!(braille_char(0xFF), '\u{28FF}');
    }
}
