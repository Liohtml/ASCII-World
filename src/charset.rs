//! Character sets, ordered dark → light.
//!
//! A cell whose average brightness is 0 maps to the first character of the
//! set (the densest glyph), a white cell maps to the last (usually a space).
//! Language sets are sorted at runtime by measuring real glyph coverage in
//! the bundled fonts — the same idea as the original Python project (which
//! additionally thinned the ramp toward evenly spaced brightness steps; we
//! keep every glyph instead).
//!
//! `braille` is not a ramp at all: it switches [`crate::engine::convert`]
//! into a sub-cell sampling mode. See [`Charset`].

use crate::font::FontStack;
use anyhow::{bail, Result};

/// The 10-character set from the original ASCII-generator.
pub const SIMPLE: &str = "@%#*+=-:. ";
/// The 70-character set from the original ASCII-generator.
pub const COMPLEX: &str =
    "$@B%8&WM#*oahkbdpqwmZO0QLCJUYXzcvunxrjft/\\|()1{}[]?-_+~<>i!lI;:,\"^`'. ";
/// Unicode block elements — great for chunky, high-contrast output.
pub const BLOCKS: &str = "█▓▒░ ";

/// Px size used when measuring glyph coverage. Big enough that antialiasing
/// noise does not reorder neighbouring glyphs.
const DENSITY_PX: f32 = 32.0;

/// Language alphabets, density-sorted at runtime. One row per language keeps
/// `NAMED`, `resolve`, and the `charsets` subcommand in sync automatically.
///
/// The CJK rows are the original project's sets; they need a CJK-capable
/// font, which the `cjk` feature embeds (see [`crate::font`]).
const LANGUAGES: &[(&str, &str)] = &[
    (
        "english",
        "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZz",
    ),
    (
        "german",
        "AaÄäBbßCcDdEeFfGgHhIiJjKkLlMmNnOoÖöPpQqRrSsTtUuÜüVvWwXxYyZz",
    ),
    (
        "french",
        "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzÆæŒœÇçÀàÂâÉéÈèÊêËëÎîÏïÔôÛûÙùŸÿ",
    ),
    (
        "spanish",
        "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzÑñáéíóú¡¿",
    ),
    (
        "italian",
        "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzÀÈàèéìòù",
    ),
    (
        "portuguese",
        "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZzàÀáÁâÂãÃçÇéÉêÊíÍóÓôÔõÕúÚ",
    ),
    (
        "polish",
        "AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpRrSsTtUuWwYyZzĄąĘęÓóŁłŃńŻżŚśĆćŹź",
    ),
    (
        "russian",
        "АаБбВвГгДдЕеЁёЖжЗзИиЙйКкЛлМмНнОоПпРрСсТтУуФфХхЦцЧчШшЩщЪъЫыЬьЭэЮюЯя",
    ),
    (
        "chinese",
        "龘䶑瀰幗獼鑭躙䵹觿䲔釅欄鐮䥯鶒獭鰽襽螻鰱蹦屭繩圇婹歜剛屧磕媿慪像僭堳噞呱棒偁呣塙唑浠唼刻凌咄亟拮俗参坒估这聿布允仫忖玗甴木亪女去凸五圹亐囗弌九人亏产斗丩艹刂彳丬了５丄三亻讠厂丆丨１二宀冖乛一丶、",
    ),
    (
        "korean",
        "ㄱㄴㄷㄹㅁㅂㅅㅇㅈㅊㅋㅌㅍㅎㅏㅑㅓㅕㅗㅛㅜㅠㅡㅣ",
    ),
    (
        "japanese",
        "あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわをんアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン",
    ),
];

/// Sets that are not language alphabets. Only the sync test needs the list;
/// `resolve` matches them by name.
#[cfg(test)]
const FIXED: &[&str] = &["simple", "complex", "blocks", "braille"];

/// Names accepted by `--charset` and the MCP tool schema.
pub const NAMED: &[&str] = &[
    "simple",
    "complex",
    "blocks",
    "braille",
    "english",
    "german",
    "french",
    "spanish",
    "italian",
    "portuguese",
    "polish",
    "russian",
    "chinese",
    "korean",
    "japanese",
];

/// How [`crate::engine::convert`] should turn cells into characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charset {
    /// One character per cell, picked from a dark → light ramp by cell luma.
    Ramp(Vec<char>),
    /// One braille pattern per cell, composed from a thresholded 2×4 dot
    /// grid — eight times the spatial resolution of a ramp.
    Braille,
}

/// The densest braille cell (all eight dots), used for cell metrics.
pub const BRAILLE_FULL: char = '\u{28FF}';
/// The empty braille cell. Blank cells use a plain space instead, so text
/// output stays copy-pasteable, but the codepoint anchors the block.
pub const BRAILLE_BLANK: char = '\u{2800}';

impl Charset {
    /// The ramp, for the ramp mode only.
    pub fn ramp(&self) -> Option<&[char]> {
        match self {
            Charset::Ramp(ramp) => Some(ramp),
            Charset::Braille => None,
        }
    }

    /// Every glyph this charset can emit — what [`crate::font::FontStack::cell_metrics`]
    /// needs to size a cell. All 256 braille patterns share one advance, so
    /// the densest cell stands in for the block.
    pub fn glyphs(&self) -> Vec<char> {
        match self {
            Charset::Ramp(ramp) => ramp.clone(),
            Charset::Braille => vec![BRAILLE_FULL, BRAILLE_BLANK],
        }
    }

    /// Sampling mode as reported in `--json` output.
    pub fn mode(&self) -> &'static str {
        match self {
            Charset::Ramp(_) => "ramp",
            Charset::Braille => "braille",
        }
    }
}

/// Resolve a `--charset` argument using the built-in fonts.
///
/// Accepts a built-in name, or `custom:<chars>` where `<chars>` is any
/// sequence of characters already ordered dark → light.
pub fn resolve(name: &str) -> Result<Charset> {
    resolve_with(name, crate::font::embedded())
}

/// Resolve a `--charset` argument, measuring glyph density in `fonts`.
///
/// Pass the same stack you paint with so a `--font` override also decides
/// the ramp order.
pub fn resolve_with(name: &str, fonts: &FontStack) -> Result<Charset> {
    if let Some(custom) = name.strip_prefix("custom:") {
        let chars: Vec<char> = custom.chars().collect();
        if chars.len() < 2 {
            bail!("custom charset needs at least 2 characters (dark → light)");
        }
        return Ok(Charset::Ramp(chars));
    }
    let ramp = match name {
        "simple" => SIMPLE.chars().collect(),
        "complex" => COMPLEX.chars().collect(),
        "blocks" => BLOCKS.chars().collect(),
        "braille" => return Ok(Charset::Braille),
        other => match LANGUAGES.iter().find(|(lang, _)| *lang == other) {
            Some((_, chars)) => density_sort_with(chars, fonts),
            None => {
                bail!("unknown charset '{other}'. Use one of {NAMED:?} or 'custom:<chars>'")
            }
        },
    };
    Ok(Charset::Ramp(ramp))
}

/// Sort characters dark → light by glyph coverage in the built-in fonts.
pub fn density_sort(chars: &str) -> Vec<char> {
    density_sort_with(chars, crate::font::embedded())
}

/// Sort characters dark → light by measuring per-glyph pixel coverage in
/// `fonts`, then append a space as the "white" end of the ramp.
pub fn density_sort_with(chars: &str, fonts: &FontStack) -> Vec<char> {
    let mut weighted: Vec<(f32, char)> = chars
        .chars()
        .map(|c| (fonts.coverage(c, DENSITY_PX), c))
        .collect();
    // Densest first; ties broken by char for determinism.
    weighted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut ramp: Vec<char> = weighted.into_iter().map(|(_, c)| c).collect();
    ramp.push(' ');
    ramp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_of(name: &str) -> Vec<char> {
        resolve(name).unwrap().ramp().unwrap().to_vec()
    }

    #[test]
    fn builtin_names_all_resolve() {
        for name in NAMED {
            match resolve(name).unwrap() {
                Charset::Ramp(ramp) => assert!(ramp.len() >= 2, "charset {name} too short"),
                Charset::Braille => assert_eq!(*name, "braille"),
            }
        }
    }

    #[test]
    fn named_covers_every_language_and_fixed_set() {
        for (lang, _) in LANGUAGES {
            assert!(NAMED.contains(lang), "{lang} missing from NAMED");
        }
        for fixed in FIXED {
            assert!(NAMED.contains(fixed), "{fixed} missing from NAMED");
        }
        assert_eq!(NAMED.len(), LANGUAGES.len() + FIXED.len());
    }

    #[test]
    fn simple_ends_light() {
        let ramp = ramp_of("simple");
        assert_eq!(*ramp.first().unwrap(), '@');
        assert_eq!(*ramp.last().unwrap(), ' ');
    }

    #[test]
    fn density_sort_puts_space_last() {
        let ramp = ramp_of("english");
        assert_eq!(*ramp.last().unwrap(), ' ');
        // 'W' and 'M' are among the densest Latin glyphs; expect them early.
        let pos_w = ramp.iter().position(|&c| c == 'W').unwrap();
        let pos_i = ramp.iter().position(|&c| c == 'i').unwrap();
        assert!(pos_w < pos_i, "W should be denser than i");
    }

    #[test]
    fn custom_charset_parses() {
        assert_eq!(ramp_of("custom:#. "), vec!['#', '.', ' ']);
        assert!(resolve("custom:x").is_err());
        assert!(resolve("nope").is_err());
    }

    #[test]
    fn braille_is_its_own_mode() {
        let cs = resolve("braille").unwrap();
        assert_eq!(cs, Charset::Braille);
        assert!(cs.ramp().is_none());
        assert_eq!(cs.mode(), "braille");
    }

    #[cfg(feature = "cjk")]
    #[test]
    fn cjk_sets_sort_by_real_stroke_density() {
        // 龘 (48 strokes) must land far ahead of 一 (1 stroke); a font with
        // no CJK coverage would score both 0 and leave the input order.
        let ramp = ramp_of("chinese");
        let dense = ramp.iter().position(|&c| c == '龘').unwrap();
        let sparse = ramp.iter().position(|&c| c == '一').unwrap();
        assert!(dense < sparse, "龘 at {dense}, 一 at {sparse}");
        assert_eq!(*ramp.last().unwrap(), ' ');

        for name in ["korean", "japanese"] {
            let ramp = ramp_of(name);
            assert!(ramp.len() > 20, "{name} lost glyphs");
        }
    }

    #[test]
    fn glyphs_cover_what_gets_painted() {
        assert_eq!(Charset::Braille.glyphs(), vec![BRAILLE_FULL, BRAILLE_BLANK]);
        assert_eq!(
            resolve("simple").unwrap().glyphs().len(),
            SIMPLE.chars().count()
        );
    }
}
