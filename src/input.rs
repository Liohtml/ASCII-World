//! Input loading: raster stills, SVG, and animated GIF frames.
//!
//! Everything upstream of [`crate::engine`] lands here so the engine keeps
//! taking one plain [`RgbaImage`] and nothing else.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::io::Read;

/// One frame of an animation.
#[derive(Debug, Clone)]
pub struct Frame {
    pub image: RgbaImage,
    /// Display time in milliseconds.
    pub delay_ms: u32,
}

/// Delay used when a GIF frame declares none (the browser convention).
pub const DEFAULT_DELAY_MS: u32 = 100;

/// Read an input path, or all of stdin for `-`.
pub fn read_bytes(input: &str) -> Result<Vec<u8>> {
    if input == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("failed to read image from stdin")?;
        Ok(buf)
    } else {
        std::fs::read(input).with_context(|| format!("failed to open image '{input}'"))
    }
}

/// Does this look like an SVG document?
///
/// Sniffed rather than trusted to the extension, because `-` (stdin) has no
/// extension and neither do a lot of downloaded files.
pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg")
        || (text.starts_with("<?xml") || text.starts_with("<!--")) && {
            String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]).contains("<svg")
        }
}

/// Does this look like a GIF (possibly animated)?
pub fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

/// Decode a still image.
///
/// `target_px` is the width an SVG should be rasterized at; raster formats
/// ignore it, since they come with a resolution of their own.
pub fn decode_still(bytes: &[u8], target_px: u32) -> Result<RgbaImage> {
    if is_svg(bytes) {
        return decode_svg(bytes, target_px);
    }
    Ok(image::load_from_memory(bytes)
        .context("input is not a supported image format (PNG/JPEG/GIF/WebP/BMP/SVG)")?
        .to_rgba8())
}

/// Decode every frame of an animation, or a single frame for a still.
pub fn decode_frames(bytes: &[u8], target_px: u32) -> Result<Vec<Frame>> {
    if !is_gif(bytes) {
        return Ok(vec![Frame {
            image: decode_still(bytes, target_px)?,
            delay_ms: DEFAULT_DELAY_MS,
        }]);
    }
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .context("input is not a readable GIF")?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .context("failed to decode GIF frames")?;
    Ok(frames
        .into_iter()
        .map(|f| {
            let (num, den) = f.delay().numer_denom_ms();
            let delay_ms = if den == 0 {
                DEFAULT_DELAY_MS
            } else {
                num / den
            };
            Frame {
                image: f.into_buffer(),
                delay_ms: delay_ms.max(1),
            }
        })
        .collect())
}

/// Rasterize an SVG at `target_px` wide, preserving its aspect ratio.
#[cfg(feature = "svg")]
pub fn decode_svg(bytes: &[u8], target_px: u32) -> Result<RgbaImage> {
    use anyhow::bail;
    use resvg::{tiny_skia, usvg};

    let tree =
        usvg::Tree::from_data(bytes, &usvg::Options::default()).context("failed to parse SVG")?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        bail!("SVG has no intrinsic size");
    }
    // Vector input has no native resolution, so render exactly as much detail
    // as the requested character grid can consume.
    let target_px = target_px.clamp(MIN_SVG_PX, MAX_SVG_PX);
    let scale = target_px as f32 / size.width();
    let (w, h) = (target_px, ((size.height() * scale).round() as u32).max(1));
    let mut pixmap =
        tiny_skia::Pixmap::new(w, h).context("SVG raster target is too large to allocate")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia stores premultiplied alpha; RgbaImage wants straight.
    let mut out = RgbaImage::new(w, h);
    for (px, out_px) in pixmap.pixels().iter().zip(out.pixels_mut()) {
        let c = px.demultiply();
        out_px.0 = [c.red(), c.green(), c.blue(), c.alpha()];
    }
    Ok(out)
}

#[cfg(not(feature = "svg"))]
pub fn decode_svg(_bytes: &[u8], _target_px: u32) -> Result<RgbaImage> {
    anyhow::bail!("SVG input needs the 'svg' feature (enabled by default)")
}

/// Never rasterize an SVG smaller than this — thin strokes would vanish.
#[cfg(feature = "svg")]
const MIN_SVG_PX: u32 = 256;
/// ...nor larger than this, however many columns were asked for.
#[cfg(feature = "svg")]
const MAX_SVG_PX: u32 = 8192;

/// Raster width to rasterize an SVG at for a `cols`-wide character grid.
///
/// Eight source pixels per cell is plenty for cell averaging, and for braille
/// it still leaves four pixels per dot.
pub fn svg_target_px(cols: u32) -> u32 {
    cols.saturating_mul(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
        <rect x="0" y="0" width="20" height="20" fill="#000"/>
    </svg>"##;

    #[test]
    fn sniffs_svg() {
        assert!(is_svg(SQUARE_SVG));
        assert!(is_svg(b"  \n<svg width='1'></svg>"));
        assert!(is_svg(
            br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"/>"#
        ));
        assert!(!is_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_svg(b"GIF89a"));
        assert!(!is_svg(b""));
    }

    #[test]
    fn sniffs_gif() {
        assert!(is_gif(b"GIF89a..."));
        assert!(is_gif(b"GIF87a..."));
        assert!(!is_gif(b"\x89PNG\r\n\x1a\n"));
    }

    #[cfg(feature = "svg")]
    #[test]
    fn rasterizes_svg_at_the_requested_width() {
        let img = decode_svg(SQUARE_SVG, 400).unwrap();
        assert_eq!(img.width(), 400);
        assert_eq!(img.height(), 200, "aspect ratio not preserved");
        // Left half black and opaque, right half untouched → transparent.
        assert_eq!(img.get_pixel(10, 100).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(390, 100).0[3], 0);
    }

    #[cfg(feature = "svg")]
    #[test]
    fn svg_raster_size_is_clamped() {
        assert_eq!(decode_svg(SQUARE_SVG, 1).unwrap().width(), MIN_SVG_PX);
        assert_eq!(decode_svg(SQUARE_SVG, 99_999).unwrap().width(), MAX_SVG_PX);
    }

    #[cfg(feature = "svg")]
    #[test]
    fn broken_svg_errors() {
        assert!(decode_svg(b"<svg", 256).is_err());
    }

    #[cfg(feature = "svg")]
    #[test]
    fn decode_still_routes_svg_by_content() {
        let img = decode_still(SQUARE_SVG, 320).unwrap();
        assert_eq!(img.width(), 320);
    }

    #[test]
    fn decode_still_rejects_garbage() {
        assert!(decode_still(b"not an image at all", 256).is_err());
    }

    #[test]
    fn svg_target_px_scales_with_columns() {
        assert_eq!(svg_target_px(100), 800);
        assert_eq!(svg_target_px(u32::MAX), u32::MAX);
    }

    #[test]
    fn decode_frames_wraps_a_still_as_one_frame() {
        let png = {
            let img = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        };
        let frames = decode_frames(&png, 256).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].image.get_pixel(0, 0).0, [1, 2, 3, 255]);
    }
}
