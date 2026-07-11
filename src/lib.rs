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
