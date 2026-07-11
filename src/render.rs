//! Text-based renderers: plain text, ANSI true-color, and JSON.

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
    let mut out = String::with_capacity((grid.cols as usize * 20 + 1) * grid.rows as usize);
    for row in 0..grid.rows {
        let mut last: Option<[u8; 3]> = None;
        for col in 0..grid.cols {
            let color = grid.color_at(row, col);
            if last != Some(color) {
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{}m",
                    color[0], color[1], color[2]
                ));
                last = Some(color);
            }
            out.push(grid.char_at(row, col));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// Render the grid as machine-readable JSON.
///
/// `include_colors` adds a `colors` field: per row, one `#rrggbb` hex string
/// per cell. Agents that render downstream (HTML, SVG, terminals) use it to
/// reconstruct the colored image.
pub fn to_json(grid: &AsciiGrid, charset: &[char], include_colors: bool) -> String {
    #[derive(Serialize)]
    struct Out {
        cols: u32,
        rows: u32,
        charset: String,
        lines: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        colors: Option<Vec<Vec<String>>>,
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

    let out = Out {
        cols: grid.cols,
        rows: grid.rows,
        charset: charset.iter().collect(),
        lines,
        colors,
    };
    serde_json::to_string(&out).expect("grid serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{convert, Options};
    use image::{Rgb, RgbImage};

    fn tiny_grid() -> AsciiGrid {
        let img = RgbImage::from_pixel(4, 4, Rgb([0, 0, 0]));
        convert(
            &img,
            &Options {
                width: 2,
                ..Options::default()
            },
        )
        .unwrap()
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
    fn json_roundtrips() {
        let grid = tiny_grid();
        let parsed: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &['@', ' '], true)).unwrap();
        assert_eq!(parsed["cols"], 2);
        assert_eq!(
            parsed["lines"].as_array().unwrap().len(),
            grid.rows as usize
        );
        assert_eq!(parsed["colors"][0][0], "#000000");
        let no_colors: serde_json::Value =
            serde_json::from_str(&to_json(&grid, &['@', ' '], false)).unwrap();
        assert!(no_colors.get("colors").is_none());
    }
}
