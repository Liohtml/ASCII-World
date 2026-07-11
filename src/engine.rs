//! Core conversion: image → grid of (character, average color) cells.

use anyhow::{bail, Result};
use image::RgbImage;

/// Conversion parameters.
#[derive(Debug, Clone)]
pub struct Options {
    /// Output width in characters (columns). Clamped to the image width.
    pub width: u32,
    /// Character ramp ordered dark → light.
    pub charset: Vec<char>,
    /// Flip the ramp (useful for light terminals / white backgrounds).
    pub invert: bool,
    /// Cell height as a multiple of cell width. Terminal glyphs are roughly
    /// twice as tall as wide, so 2.0 preserves the image's aspect ratio.
    pub aspect: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 100,
            charset: crate::charset::SIMPLE.chars().collect(),
            invert: false,
            aspect: 2.0,
        }
    }
}

/// The result of a conversion: a row-major grid of characters plus the
/// average color of each source cell (used by ANSI/PNG color rendering).
#[derive(Debug)]
pub struct AsciiGrid {
    pub cols: u32,
    pub rows: u32,
    pub chars: Vec<char>,
    pub colors: Vec<[u8; 3]>,
}

impl AsciiGrid {
    pub fn char_at(&self, row: u32, col: u32) -> char {
        self.chars[(row * self.cols + col) as usize]
    }

    pub fn color_at(&self, row: u32, col: u32) -> [u8; 3] {
        self.colors[(row * self.cols + col) as usize]
    }
}

/// Convert an RGB image into an [`AsciiGrid`].
///
/// Each output cell averages a `cell_width × cell_height` block of source
/// pixels; the block's luma picks a character from the ramp and the block's
/// mean RGB becomes the cell color.
pub fn convert(image: &RgbImage, opts: &Options) -> Result<AsciiGrid> {
    let (img_w, img_h) = image.dimensions();
    if img_w == 0 || img_h == 0 {
        bail!("input image is empty");
    }
    if opts.charset.len() < 2 {
        bail!("charset needs at least 2 characters");
    }
    if opts.aspect <= 0.0 {
        bail!("aspect must be positive");
    }

    let cols = opts.width.clamp(1, img_w);
    let cell_w = img_w as f64 / cols as f64;
    let cell_h = (cell_w * opts.aspect as f64).max(1.0);
    let rows = ((img_h as f64 / cell_h) as u32).max(1);

    let n = opts.charset.len();
    let mut chars = Vec::with_capacity((cols * rows) as usize);
    let mut colors = Vec::with_capacity((cols * rows) as usize);

    for row in 0..rows {
        let y0 = (row as f64 * cell_h) as u32;
        let y1 = (((row + 1) as f64 * cell_h) as u32).min(img_h).max(y0 + 1);
        for col in 0..cols {
            let x0 = (col as f64 * cell_w) as u32;
            let x1 = (((col + 1) as f64 * cell_w) as u32).min(img_w).max(x0 + 1);

            let mut sum = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = image.get_pixel(x, y).0;
                    sum[0] += p[0] as u64;
                    sum[1] += p[1] as u64;
                    sum[2] += p[2] as u64;
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            let avg = [
                (sum[0] / count) as u8,
                (sum[1] / count) as u8,
                (sum[2] / count) as u8,
            ];
            // Rec. 601 luma — perceptually better than a plain channel mean.
            let luma = 0.299 * avg[0] as f64 + 0.587 * avg[1] as f64 + 0.114 * avg[2] as f64;
            let mut idx = ((luma * n as f64 / 255.0) as usize).min(n - 1);
            if opts.invert {
                idx = n - 1 - idx;
            }
            chars.push(opts.charset[idx]);
            colors.push(avg);
        }
    }

    Ok(AsciiGrid {
        cols,
        rows,
        chars,
        colors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn flat_image(w: u32, h: u32, value: u8) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb([value, value, value]))
    }

    #[test]
    fn black_image_maps_to_densest_char() {
        let grid = convert(&flat_image(100, 100, 0), &Options::default()).unwrap();
        assert!(grid.chars.iter().all(|&c| c == '@'));
    }

    #[test]
    fn white_image_maps_to_space() {
        let grid = convert(&flat_image(100, 100, 255), &Options::default()).unwrap();
        assert!(grid.chars.iter().all(|&c| c == ' '));
    }

    #[test]
    fn invert_flips_ramp() {
        let opts = Options {
            invert: true,
            ..Options::default()
        };
        let grid = convert(&flat_image(100, 100, 0), &opts).unwrap();
        assert!(grid.chars.iter().all(|&c| c == ' '));
    }

    #[test]
    fn grid_dimensions_follow_width_and_aspect() {
        let grid = convert(&flat_image(200, 200, 128), &Options::default()).unwrap();
        assert_eq!(grid.cols, 100);
        assert_eq!(grid.rows, 50); // aspect 2.0 → cells twice as tall
        assert_eq!(grid.chars.len(), (grid.cols * grid.rows) as usize);
        assert_eq!(grid.colors.len(), grid.chars.len());
    }

    #[test]
    fn width_is_clamped_to_image_width() {
        let opts = Options {
            width: 5000,
            ..Options::default()
        };
        let grid = convert(&flat_image(64, 64, 128), &opts).unwrap();
        assert_eq!(grid.cols, 64);
    }

    #[test]
    fn colors_average_the_cell() {
        let img = RgbImage::from_pixel(10, 10, Rgb([200, 100, 50]));
        let opts = Options {
            width: 1,
            ..Options::default()
        };
        let grid = convert(&img, &opts).unwrap();
        assert_eq!(grid.color_at(0, 0), [200, 100, 50]);
    }

    #[test]
    fn gradient_is_monotonic() {
        // Left-to-right black→white gradient must produce a ramp that never
        // gets darker as brightness increases.
        let mut img = RgbImage::new(256, 32);
        for (x, _, p) in img.enumerate_pixels_mut() {
            let v = x as u8;
            *p = Rgb([v, v, v]);
        }
        let opts = Options {
            width: 16,
            ..Options::default()
        };
        let grid = convert(&img, &opts).unwrap();
        let ramp: Vec<char> = opts.charset.clone();
        let mut last_pos = 0usize;
        for col in 0..grid.cols {
            let c = grid.char_at(0, col);
            let pos = ramp.iter().position(|&r| r == c).unwrap();
            assert!(pos >= last_pos, "ramp went darker at col {col}");
            last_pos = pos;
        }
    }
}
