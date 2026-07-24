//! ascii-world — image-to-ASCII engine built for agent workflows.
//!
//! The library exposes the full pipeline so you can embed it in your own
//! tools: load pixels with [`input`], pick a [`charset`], run
//! [`engine::convert`] to get an [`engine::AsciiGrid`], then serialize it with
//! one of the [`render`] functions or paint it to a PNG with
//! [`paint::paint_png`]. [`anim`] and [`video`] loop that same pipeline over
//! animation frames.

pub mod anim;
pub mod charset;
pub mod engine;
pub mod font;
pub mod input;
pub mod paint;
pub mod render;

// Both shell out to the host OS, which a browser does not have.
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod video;

#[cfg(feature = "wasm")]
mod wasm;

/// DejaVu Sans Mono Bold, embedded so the binary is fully standalone.
/// License: assets/fonts/DejaVu-Fonts-License.txt (free, redistributable).
pub use font::DEJAVU_BYTES as FONT_BYTES;

/// The primary embedded font, parsed once per process.
///
/// For anything that may hit a non-Latin charset, use [`font::embedded`]
/// instead — it carries the CJK fallback too.
pub fn font() -> &'static ab_glyph::FontArc {
    font::embedded().primary()
}
