//! Browser bindings.
//!
//! The engine is pure computation over a pixel buffer, so the whole pipeline
//! compiles to wasm32 unchanged — only the process-facing bits (CLI, MCP,
//! ffmpeg) are excluded. Build with:
//!
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown \
//!     --no-default-features --features wasm,cjk,svg
//! wasm-bindgen target/wasm32-unknown-unknown/release/ascii_world.wasm \
//!     --out-dir web/pkg --target web
//! ```

use crate::{charset, engine, input, render};
use wasm_bindgen::prelude::*;

/// Turn image bytes (PNG/JPEG/GIF/WebP/BMP/SVG) into ASCII text.
///
/// `charset` takes the same values as the CLI's `--charset`, including
/// `braille` and `custom:<chars>`.
#[wasm_bindgen(js_name = imageToAscii)]
pub fn image_to_ascii(
    bytes: &[u8],
    width: u32,
    charset: &str,
    invert: bool,
) -> Result<String, JsError> {
    let grid = convert(bytes, width, charset, invert)?;
    Ok(render::to_text(&grid.0))
}

/// Same conversion, returned as the CLI's `--json` document (a string, so the
/// caller decides whether to `JSON.parse` it).
#[wasm_bindgen(js_name = imageToJson)]
pub fn image_to_json(
    bytes: &[u8],
    width: u32,
    charset: &str,
    invert: bool,
    include_colors: bool,
) -> Result<String, JsError> {
    let grid = convert(bytes, width, charset, invert)?;
    Ok(render::to_json(&grid.0, &grid.1, invert, include_colors))
}

/// Built-in charset names, for populating a picker.
#[wasm_bindgen(js_name = charsetNames)]
pub fn charset_names() -> Vec<String> {
    charset::NAMED.iter().map(|s| s.to_string()).collect()
}

/// The crate version this module was built from.
#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

struct Converted(engine::AsciiGrid, charset::Charset);

fn convert(bytes: &[u8], width: u32, name: &str, invert: bool) -> Result<Converted, JsError> {
    let run = || -> anyhow::Result<Converted> {
        let charset = charset::resolve(name)?;
        let image = input::decode_still(bytes, input::svg_target_px(width))?;
        let grid = engine::convert(
            &image,
            &engine::Options {
                width,
                charset: charset.clone(),
                invert,
                aspect: 2.0,
                alpha_threshold: engine::DEFAULT_ALPHA_THRESHOLD,
                matte: Some(if invert { [0, 0, 0] } else { [255, 255, 255] }),
                braille_threshold: None,
            },
        )?;
        Ok(Converted(grid, charset))
    };
    run().map_err(|e| JsError::new(&format!("{e:#}")))
}
