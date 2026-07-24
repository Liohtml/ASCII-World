//! Text-based renderers: plain text, ANSI true-color, and JSON.

use crate::charset::Charset;
use crate::engine::AsciiGrid;
use serde::Serialize;

/// Render the grid as plain text, one line per row.
pub fn to_text(grid: &AsciiGrid) -> String {
    let mut out = String::with_capacity((grid.cols as usize + 1) * grid.rows as usize);
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            out.push(grid.char_at(row, col));
        }
        out.push('\n');
    }
    out
}

/// Render the grid with 24-bit ANSI foreground colors for terminals.
pub fn to_ansi(grid: &AsciiGrid) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity((grid.cols as usize * 20 + 1) * grid.rows as usize);
    for row in 0..grid.rows {
        let mut last: Option<[u8; 3]> = None;
        for col in 0..grid.cols {
            let ch = grid.char_at(row, col);
            // Blank cells show the terminal background; recoloring them only
            // bloats the output (and breaks up runs of one color).
            if ch != ' ' {
                let color = grid.color_at(row, col);
                if last != Some(color) {
                    let [r, g, b] = color;
                    let _ = write!(out, "\x1b[38;2;{r};{g};{b}m");
                    last = Some(color);
                }
            }
            out.push(ch);
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// The ramp as it was actually applied: reversed when `invert` was set.
///
/// Pass the result to [`to_json`] so the serialized `charset` field keeps its
/// contract — index 0 is the character used for the darkest cells — even for
/// inverted conversions.
pub fn effective_ramp(charset: &[char], invert: bool) -> Vec<char> {
    let mut ramp = charset.to_vec();
    if invert {
        ramp.reverse();
    }
    ramp
}

/// Render the grid as machine-readable JSON.
///
/// `charset` and `invert` must be the ones applied to this grid. Fields:
///
/// - `cols`, `rows` — grid size
/// - `mode` — `"ramp"` or `"braille"`
/// - `charset` — for `"ramp"`, the ramp as applied (index 0 = darkest cell);
///   for `"braille"` the literal `"braille"`, since dots are composed, not
///   indexed
/// - `lines` — one string per row
/// - `colors` — per row, one `#rrggbb` per cell (when `include_colors`)
/// - `alpha` — per row, one 0–255 mean alpha per cell; present only when the
///   source actually carried transparency
pub fn to_json(grid: &AsciiGrid, charset: &Charset, invert: bool, include_colors: bool) -> String {
    #[derive(Serialize)]
    struct Out {
        cols: u32,
        rows: u32,
        charset: String,
        mode: &'static str,
        lines: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        colors: Option<Vec<Vec<String>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alpha: Option<Vec<Vec<u8>>>,
    }

    let lines: Vec<String> = (0..grid.rows)
        .map(|row| (0..grid.cols).map(|col| grid.char_at(row, col)).collect())
        .collect();

    let colors = include_colors.then(|| {
        (0..grid.rows)
            .map(|row| {
                (0..grid.cols)
                    .map(|col| {
                        let [r, g, b] = grid.color_at(row, col);
                        format!("#{r:02x}{g:02x}{b:02x}")
                    })
                    .collect()
            })
            .collect()
    });

    let alpha = grid.has_alpha().then(|| {
        (0..grid.rows)
            .map(|row| (0..grid.cols).map(|col| grid.alpha_at(row, col)).collect())
            .collect()
    });

    let out = Out {
        cols: grid.cols,
        rows: grid.rows,
        charset: match charset {
            Charset::Ramp(ramp) => effective_ramp(ramp, invert).iter().collect(),
            Charset::Braille => "braille".into(),
        },
        mode: charset.mode(),
        lines,
        colors,
        alpha,
    };
    serde_json::to_string(&out).expect("grid serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::charset;
    use crate::engine::{convert, Options};
    use image::{Rgba, RgbaImage};

    fn grid_of(img: &RgbaImage, opts: Options) -> AsciiGrid {
        convert(img, &opts).unwrap()
    }

    fn tiny_grid() -> AsciiGrid {
        let img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        grid_of(
            &img,
            Options {
                width: 2,
                ..Options::default()
            },
        )
    }

    fn simple_charset() -> Charset {
        charset::resolve("simple").unwrap()
    }

    #[test]
    fn text_has_one_line_per_row() {
        let grid = tiny_grid();
        let text = to_text(&grid);
        assert_eq!(text.lines().count(), grid.rows as usize);
        assert!(text
            .lines()
            .all(|l| l.chars().count() == grid.cols as usize));
    }

    #[test]
    fn ansi_contains_truecolor_escape_and_reset() {
        let ansi = to_ansi(&tiny_grid());
        assert!(ansi.contains("\x1b[38;2;0;0;0m"));
        assert!(ansi.ends_with("\x1b[0m\n"));
    }

    #[test]
    fn ansi_skips_color_for_blank_cells() {
        let img = RgbaImage::from_pixel(4, 4, Rgba([255, 255, 255, 255]));
        let ansi = to_ansi(&grid_of(
            &img,
            Options {
                width: 2,
                ..Options::default()
            },
        ));
        assert!(!ansi.contains("38;2;"), "blank rows should carry no color");
    }

    #[test]
    fn json_roundtrips() {
        let grid = tiny_grid();
        let parsed: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &simple_charset(), false, true)).unwrap();
        assert_eq!(parsed["cols"], 2);
        assert_eq!(parsed["mode"], "ramp");
        assert_eq!(parsed["charset"], charset::SIMPLE);
        assert_eq!(
            parsed["lines"].as_array().unwrap().len(),
            grid.rows as usize
        );
        assert_eq!(parsed["colors"][0][0], "#000000");
        // Opaque input: no alpha field at all, so existing parsers see the
        // exact same document they always did.
        assert!(parsed.get("alpha").is_none());

        let no_colors: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &simple_charset(), false, false)).unwrap();
        assert!(no_colors.get("colors").is_none());
    }

    #[test]
    fn json_charset_is_the_ramp_as_applied() {
        let grid = tiny_grid();
        let parsed: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &simple_charset(), true, false)).unwrap();
        let reversed: String = charset::SIMPLE.chars().rev().collect();
        assert_eq!(parsed["charset"], reversed);
    }

    #[test]
    fn json_reports_alpha_only_for_transparent_sources() {
        // The whole left column is transparent, so cell (0,0) — which spans
        // two source rows at aspect 2.0 — averages to alpha 0.
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        for y in 0..4 {
            img.put_pixel(0, y, Rgba([0, 0, 0, 0]));
        }
        let grid = grid_of(
            &img,
            Options {
                width: 4,
                ..Options::default()
            },
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &simple_charset(), false, true)).unwrap();
        assert_eq!(parsed["alpha"][0][0], 0);
        assert_eq!(parsed["alpha"][0][1], 255);
        assert_eq!(
            parsed["lines"][0].as_str().unwrap().chars().next(),
            Some(' ')
        );
    }

    #[test]
    fn json_marks_braille_mode() {
        let img = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255]));
        let grid = grid_of(
            &img,
            Options {
                width: 4,
                charset: Charset::Braille,
                ..Options::default()
            },
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &Charset::Braille, false, false)).unwrap();
        assert_eq!(parsed["mode"], "braille");
        assert_eq!(parsed["charset"], "braille");
        assert!(parsed["lines"][0].as_str().unwrap().contains('\u{28FF}'));
    }

    #[test]
    fn effective_ramp_reverses_only_when_inverted() {
        assert_eq!(effective_ramp(&['@', '.', ' '], false), vec!['@', '.', ' ']);
        assert_eq!(effective_ramp(&['@', '.', ' '], true), vec![' ', '.', '@']);
    }
}
