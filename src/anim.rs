//! Animated GIF output.
//!
//! The engine is per-frame and stateless, so animation is just "convert every
//! frame with the same settings and keep the timing" — the only subtlety is
//! that every frame must come out the same size (see [`crate::paint::Bounds`]).

use crate::input::Frame;
use anyhow::{bail, Context, Result};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, RgbaImage};
use std::io::{BufWriter, Write};
use std::path::Path;

/// Encoder speed passed to `image` (1 = best quality, 30 = fastest).
/// ASCII frames are flat-colored, so a middling setting already quantizes
/// cleanly and keeps long GIFs from taking minutes.
const SPEED: i32 = 10;

/// Write frames as an animated GIF that loops forever.
pub fn write_gif(path: &Path, frames: &[Frame]) -> Result<()> {
    let file = std::fs::File::create(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    write_gif_to(BufWriter::new(file), frames)
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Write frames as an animated GIF into any sink.
pub fn write_gif_to<W: Write>(writer: W, frames: &[Frame]) -> Result<()> {
    if frames.is_empty() {
        bail!("no frames to encode");
    }
    let (w, h) = frames[0].image.dimensions();
    if let Some(odd) = frames.iter().find(|f| f.image.dimensions() != (w, h)) {
        bail!(
            "every frame must share one size; got {}x{} after {w}x{h}",
            odd.image.width(),
            odd.image.height()
        );
    }

    let mut encoder = GifEncoder::new_with_speed(writer, SPEED);
    encoder
        .set_repeat(Repeat::Infinite)
        .context("failed to set GIF loop flag")?;
    for (i, frame) in frames.iter().enumerate() {
        let delay = Delay::from_numer_denom_ms(frame.delay_ms.max(1), 1);
        encoder
            .encode_frame(image::Frame::from_parts(frame.image.clone(), 0, 0, delay))
            .with_context(|| format!("failed to encode frame {}", i + 1))?;
    }
    Ok(())
}

/// Resize-free sanity check used by the CLI: does this output path want a GIF?
pub fn wants_gif(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
}

/// Drop a frame's last row/column when odd — h.264 with yuv420p needs even
/// dimensions, and cropping beats padding with a stripe of background.
pub fn to_even(image: &RgbaImage) -> RgbaImage {
    let (w, h) = (image.width() & !1, image.height() & !1);
    if (w, h) == image.dimensions() {
        return image.clone();
    }
    RgbaImage::from_fn(w.max(1), h.max(1), |x, y| *image.get_pixel(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn frame(w: u32, h: u32, v: u8, delay_ms: u32) -> Frame {
        Frame {
            image: RgbaImage::from_pixel(w, h, Rgba([v, v, v, 255])),
            delay_ms,
        }
    }

    #[test]
    fn writes_a_multi_frame_gif_that_decodes_back() {
        let frames = vec![frame(8, 8, 0, 40), frame(8, 8, 255, 40)];
        let mut buf = Vec::new();
        write_gif_to(&mut buf, &frames).unwrap();
        assert!(buf.starts_with(b"GIF89a"));

        let decoded = crate::input::decode_frames(&buf, 256).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].image.dimensions(), (8, 8));
        assert_eq!(decoded[0].delay_ms, 40, "frame timing lost");
        // Frame 0 black, frame 1 white — the animation is not a still.
        assert!(decoded[0].image.get_pixel(0, 0).0[0] < 64);
        assert!(decoded[1].image.get_pixel(0, 0).0[0] > 192);
    }

    #[test]
    fn rejects_empty_and_ragged_input() {
        let mut buf = Vec::new();
        assert!(write_gif_to(&mut buf, &[]).is_err());
        let ragged = vec![frame(8, 8, 0, 10), frame(9, 8, 0, 10)];
        let err = write_gif_to(&mut buf, &ragged).unwrap_err();
        assert!(err.to_string().contains("one size"), "{err}");
    }

    #[test]
    fn detects_gif_output_paths() {
        assert!(wants_gif(Path::new("out.gif")));
        assert!(wants_gif(Path::new("OUT.GIF")));
        assert!(!wants_gif(Path::new("out.png")));
        assert!(!wants_gif(Path::new("out")));
    }

    #[test]
    fn to_even_rounds_dimensions_down() {
        let odd = RgbaImage::from_pixel(7, 5, Rgba([1, 2, 3, 255]));
        assert_eq!(to_even(&odd).dimensions(), (6, 4));
        let even = RgbaImage::from_pixel(8, 4, Rgba([1, 2, 3, 255]));
        assert_eq!(to_even(&even).dimensions(), (8, 4));
    }
}
