//! Character sets, ordered dark → light.
//!
//! A cell whose average brightness is 0 maps to the first character of the
//! set (the densest glyph), a white cell maps to the last (usually a space).
//! Language sets are sorted at runtime by measuring real glyph coverage in
//! the embedded font — the same trick the original Python project used.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::{bail, Context, Result};

/// The 10-character set from the original ASCII-generator.
pub const SIMPLE: &str = "@%#*+=-:. ";
/// The 70-character set from the original ASCII-generator.
pub const COMPLEX: &str =
    "$@B%8&WM#*oahkbdpqwmZO0QLCJUYXzcvunxrjft/\\|()1{}[]?-_+~<>i!lI;:,\"^`'. ";
/// Unicode block elements — great for chunky, high-contrast output.
pub const BLOCKS: &str = "█▓▒░ ";

const ENGLISH: &str = "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZz";
const GERMAN: &str = "AaÄäBbßCcDdEeFfGgHhIiJjKkLlMmNnOoÖöPpQqRrSsTtUuÜüVvWwXxYyZz";
const FRENCH: &str =
    "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzÆæŒœÇçÀàÂâÉéÈèÊêËëÎîÏïÔôÛûÙùŸÿ";
const SPANISH: &str = "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzÑñáéíóú¡¿";
const ITALIAN: &str = "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzÀÈàèéìòù";
const PORTUGUESE: &str =
    "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzàÀáÁâÂãÃçÇéÉêÊíÍóÓôÔõÕúÚ";
const POLISH: &str = "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpRrSsTtUuWwYyZzĄąĘęÓóŁłŃńŻżŚśĆćŹź";
const RUSSIAN: &str = "АаБбВвГгДдЕеЁёЖжЗзИиЙйКкЛлМмНнОоПпРрСсТтУуФфХхЦцЧчШшЩщЪъЫыЬьЭэЮюЯя";

/// Names accepted by `--charset`, shown in `--help` and the MCP tool schema.
pub const NAMED: &[&str] = &[
    "simple",
    "complex",
    "blocks",
    "english",
    "german",
    "french",
    "spanish",
    "italian",
    "portuguese",
    "polish",
    "russian",
];

/// Resolve a `--charset` argument into a dark→light character ramp.
///
/// Accepts a built-in name, or `custom:<chars>` where `<chars>` is any
/// sequence of characters already ordered dark → light.
pub fn resolve(name: &str) -> Result<Vec<char>> {
    if let Some(custom) = name.strip_prefix("custom:") {
        let chars: Vec<char> = custom.chars().collect();
        if chars.len() < 2 {
            bail!("custom charset needs at least 2 characters (dark → light)");
        }
        return Ok(chars);
    }
    let ramp = match name {
        "simple" => SIMPLE.chars().collect(),
        "complex" => COMPLEX.chars().collect(),
        "blocks" => BLOCKS.chars().collect(),
        "english" => density_sort(ENGLISH)?,
        "german" => density_sort(GERMAN)?,
        "french" => density_sort(FRENCH)?,
        "spanish" => density_sort(SPANISH)?,
        "italian" => density_sort(ITALIAN)?,
        "portuguese" => density_sort(PORTUGUESE)?,
        "polish" => density_sort(POLISH)?,
        "russian" => density_sort(RUSSIAN)?,
        other => bail!("unknown charset '{other}'. Use one of {NAMED:?} or 'custom:<chars>'"),
    };
    Ok(ramp)
}

/// Sort characters dark → light by measuring per-glyph pixel coverage in the
/// embedded font, then append a space as the "white" end of the ramp.
pub fn density_sort(chars: &str) -> Result<Vec<char>> {
    let font = FontRef::try_from_slice(crate::FONT_BYTES).context("embedded font is invalid")?;
    let scale = PxScale::from(32.0);
    let scaled = font.as_scaled(scale);
    let cell_area = scaled.h_advance(font.glyph_id('M')) * scaled.height();

    let mut weighted: Vec<(f32, char)> = chars
        .chars()
        .map(|c| {
            let glyph = font
                .glyph_id(c)
                .with_scale_and_position(scale, ab_glyph::point(0.0, scaled.ascent()));
            let coverage = match font.outline_glyph(glyph) {
                Some(outline) => {
                    let mut sum = 0.0f32;
                    outline.draw(|_, _, c| sum += c);
                    sum / cell_area
                }
                None => 0.0,
            };
            (coverage, c)
        })
        .collect();
    // Densest first; ties broken by char for determinism.
    weighted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut ramp: Vec<char> = weighted.into_iter().map(|(_, c)| c).collect();
    ramp.push(' ');
    Ok(ramp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_all_resolve() {
        for name in NAMED {
            let ramp = resolve(name).unwrap();
            assert!(ramp.len() >= 2, "charset {name} too short");
        }
    }

    #[test]
    fn simple_ends_light() {
        let ramp = resolve("simple").unwrap();
        assert_eq!(*ramp.first().unwrap(), '@');
        assert_eq!(*ramp.last().unwrap(), ' ');
    }

    #[test]
    fn density_sort_puts_space_last() {
        let ramp = resolve("english").unwrap();
        assert_eq!(*ramp.last().unwrap(), ' ');
        // 'W' and 'M' are among the densest Latin glyphs; expect them early.
        let pos_w = ramp.iter().position(|&c| c == 'W').unwrap();
        let pos_i = ramp.iter().position(|&c| c == 'i').unwrap();
        assert!(pos_w < pos_i, "W should be denser than i");
    }

    #[test]
    fn custom_charset_parses() {
        assert_eq!(resolve("custom:#. ").unwrap(), vec!['#', '.', ' ']);
        assert!(resolve("custom:x").is_err());
        assert!(resolve("nope").is_err());
    }
}
