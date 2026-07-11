//! ascii-world — image-to-ASCII engine built for agent workflows.
//!
//! The library exposes the full pipeline so you can embed it in your own
//! tools: pick a [`charset`], run [`engine::convert`] to get an
//! [`engine::AsciiGrid`], then serialize it with one of the [`render`]
//! functions or paint it to a PNG with [`paint::paint_png`].

pub mod charset;
pub mod engine;
pub mod mcp;
pub mod paint;
pub mod render;

/// DejaVu Sans Mono Bold, embedded so the binary is fully standalone.
/// License: assets/fonts/DejaVu-Fonts-License.txt (free, redistributable).
pub const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono-Bold.ttf");

/// The embedded font, parsed once per process.
pub fn font() -> &'static ab_glyph::FontRef<'static> {
    static FONT: std::sync::OnceLock<ab_glyph::FontRef<'static>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        ab_glyph::FontRef::try_from_slice(FONT_BYTES).expect("embedded font is valid")
    })
}
