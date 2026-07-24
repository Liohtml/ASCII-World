//! Video → ASCII video, with ffmpeg doing the codec work.
//!
//! Keeping ffmpeg external is deliberate: linking a decoder would multiply
//! the binary size and the build's system dependencies for a feature most
//! users of a still-image tool never touch. We speak raw RGBA over pipes, so
//! the engine itself is untouched — this module is pure plumbing.

use crate::anim;
use crate::engine::{self, Options};
use crate::font::FontStack;
use crate::paint::{self, Bounds, PaintOptions};
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

/// What ffprobe says about the input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

/// Knobs that only apply to video.
#[derive(Debug, Clone, Copy)]
pub struct VideoOptions {
    /// Output frame rate; `None` keeps the source's.
    pub fps: Option<f64>,
    /// Inset the source video in the corner at this fraction of the output
    /// width (0 disables). The original project called it `overlay_ratio`.
    pub overlay: f32,
    /// x264 quality (lower is better); ignored for GIF output.
    pub crf: u8,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            fps: None,
            overlay: 0.0,
            crf: 20,
        }
    }
}

/// What a conversion produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub frames: u64,
    pub width: u32,
    pub height: u32,
}

/// Fail early, with an actionable message, when ffmpeg is not installed.
pub fn ensure_ffmpeg() -> Result<()> {
    for tool in ["ffmpeg", "ffprobe"] {
        Command::new(tool)
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| {
                format!(
                    "'{tool}' not found. The video subcommand shells out to ffmpeg — \
                     install it (apt install ffmpeg / brew install ffmpeg) and retry"
                )
            })?;
    }
    Ok(())
}

/// Ask ffprobe for the first video stream's geometry and frame rate.
pub fn probe(input: &Path) -> Result<Spec> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output()
        .context("failed to run ffprobe")?;
    if !out.status.success() {
        bail!(
            "ffprobe could not read '{}': {}",
            input.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
        .with_context(|| format!("'{}' has no usable video stream", input.display()))
}

/// Parse `width,height,num/den` as printed by `ffprobe -of csv=p=0`.
fn parse_probe(text: &str) -> Result<Spec> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .context("ffprobe returned nothing")?;
    let mut fields = line.split(',');
    let width: u32 = fields
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .context("bad width")?;
    let height: u32 = fields
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .context("bad height")?;
    let fps = fields.next().unwrap_or("").trim();
    let fps = match fps.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().context("bad fps")?, d.parse().context("bad fps")?);
            if d == 0.0 {
                bail!("frame rate has a zero denominator");
            }
            n / d
        }
        None => fps.parse().context("bad fps")?,
    };
    if width == 0 || height == 0 {
        bail!("video stream is {width}x{height}");
    }
    if !(fps.is_finite() && fps > 0.0) {
        bail!("video stream reports {fps} fps");
    }
    Ok(Spec { width, height, fps })
}

/// Decoder: source video in, raw RGBA frames out.
fn spawn_decoder(input: &Path) -> Result<Child> {
    Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(input)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start ffmpeg for decoding")
}

/// Encoder: raw RGBA frames in, an encoded file out.
fn spawn_encoder(output: &Path, w: u32, h: u32, fps: f64, opts: &VideoOptions) -> Result<Child> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-s", &format!("{w}x{h}")])
        .args(["-framerate", &format!("{fps}")])
        .args(["-i", "-", "-an"]);
    if anim::wants_gif(output) {
        // A shared palette generated from the whole clip beats ffmpeg's
        // default 256-color guess per frame, which dithers ASCII art to mud.
        cmd.args([
            "-vf",
            "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer",
            "-loop",
            "0",
        ]);
    } else {
        cmd.args([
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &opts.crf.to_string(),
            "-pix_fmt",
            "yuv420p",
        ]);
    }
    cmd.arg(output)
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to start ffmpeg for encoding")
}

/// Convert a video file frame by frame.
///
/// `engine_opts` and `paint_opts` are exactly what the `img` path uses, so a
/// still and a frame of video are rendered identically. Cropping, if enabled,
/// is measured once on the first frame and reused — every frame of a video
/// has to be the same size.
pub fn convert_video(
    input: &Path,
    output: &Path,
    fonts: &FontStack,
    engine_opts: &Options,
    paint_opts: &PaintOptions,
    video_opts: &VideoOptions,
    mut on_progress: impl FnMut(u64),
) -> Result<Stats> {
    ensure_ffmpeg()?;
    let spec = probe(input)?;
    let fps = video_opts.fps.unwrap_or(spec.fps);
    if !(fps.is_finite() && fps > 0.0) {
        bail!("--fps must be a positive number");
    }
    if !(0.0..1.0).contains(&video_opts.overlay) {
        bail!("--overlay must be between 0 (off) and 1");
    }

    let mut decoder = spawn_decoder(input)?;
    let mut frames_in = decoder
        .stdout
        .take()
        .expect("decoder stdout was requested as a pipe");

    let mut raw = vec![0u8; spec.width as usize * spec.height as usize * 4];
    let mut encoder: Option<(Child, ChildStdin)> = None;
    let mut bounds: Option<Bounds> = None;
    let mut stats = Stats {
        frames: 0,
        width: 0,
        height: 0,
    };

    while read_frame(&mut frames_in, &mut raw)? {
        let source = RgbaImage::from_raw(spec.width, spec.height, raw.clone())
            .expect("buffer is exactly one frame");
        let grid = engine::convert(&source, engine_opts)?;
        let mut canvas = paint::paint_canvas(&grid, fonts, paint_opts)?;

        if paint_opts.crop {
            let bounds = *bounds.get_or_insert_with(|| {
                paint::content_bounds(&canvas, paint_opts.background).unwrap_or(Bounds {
                    x0: 0,
                    y0: 0,
                    x1: canvas.width(),
                    y1: canvas.height(),
                })
            });
            canvas = paint::crop_to(&canvas, bounds);
        }
        canvas = anim::to_even(&canvas);
        if video_opts.overlay > 0.0 {
            overlay_source(&mut canvas, &source, video_opts.overlay);
        }

        let (w, h) = canvas.dimensions();
        let (_, stdin) = match encoder {
            Some(ref mut e) => e,
            None => {
                stats.width = w;
                stats.height = h;
                let mut child = spawn_encoder(output, w, h, fps, video_opts)?;
                let stdin = child
                    .stdin
                    .take()
                    .expect("encoder stdin was requested as a pipe");
                encoder.insert((child, stdin))
            }
        };
        stdin
            .write_all(canvas.as_raw())
            .context("ffmpeg stopped accepting frames (check the output path and codec)")?;
        stats.frames += 1;
        on_progress(stats.frames);
    }

    let status = decoder.wait().context("ffmpeg decoder did not exit")?;
    if !status.success() {
        bail!("ffmpeg failed to decode '{}'", input.display());
    }
    let Some((mut child, stdin)) = encoder else {
        bail!("'{}' produced no frames", input.display());
    };
    drop(stdin); // EOF: let ffmpeg flush and close the container.
    let status = child.wait().context("ffmpeg encoder did not exit")?;
    if !status.success() {
        bail!("ffmpeg failed to write '{}'", output.display());
    }
    Ok(stats)
}

/// Fill `buf` with exactly one frame. `false` means the stream ended.
fn read_frame(reader: &mut impl Read, buf: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            // A partial frame means ffmpeg died mid-write; treat the stream
            // as finished rather than encoding a torn frame.
            Ok(0) => return Ok(false),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("failed to read a decoded frame"),
        }
    }
    Ok(true)
}

/// Paste the source frame into the bottom-right corner, like the original
/// project's `--overlay_ratio`.
fn overlay_source(canvas: &mut RgbaImage, source: &RgbaImage, ratio: f32) {
    let (cw, ch) = canvas.dimensions();
    let w = ((cw as f32 * ratio) as u32).max(1);
    let h = ((w as f32 * source.height() as f32 / source.width() as f32) as u32).max(1);
    if w > cw || h > ch {
        return;
    }
    let thumb = image::imageops::resize(source, w, h, image::imageops::FilterType::Triangle);
    image::imageops::replace(canvas, &thumb, (cw - w) as i64, (ch - h) as i64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_csv() {
        let spec = parse_probe("1920,1080,30000/1001\n").unwrap();
        assert_eq!(spec.width, 1920);
        assert_eq!(spec.height, 1080);
        assert!((spec.fps - 29.97).abs() < 0.01, "{}", spec.fps);

        assert_eq!(parse_probe("640,480,25\n").unwrap().fps, 25.0);
    }

    #[test]
    fn rejects_unusable_probe_output() {
        for bad in [
            "",
            "\n",
            "abc,480,25",
            "640,480,0/1",
            "0,480,25",
            "640,480,x/y",
        ] {
            assert!(parse_probe(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn reads_whole_frames_and_stops_at_eof() {
        let data = vec![7u8; 10];
        let mut cursor = std::io::Cursor::new(data);
        let mut buf = [0u8; 4];
        assert!(read_frame(&mut cursor, &mut buf).unwrap());
        assert_eq!(buf, [7; 4]);
        assert!(read_frame(&mut cursor, &mut buf).unwrap());
        // 2 bytes left: a torn frame ends the stream instead of being used.
        assert!(!read_frame(&mut cursor, &mut buf).unwrap());
        assert!(!read_frame(&mut std::io::empty(), &mut buf).unwrap());
    }

    #[test]
    fn overlay_lands_in_the_bottom_right_corner() {
        let mut canvas = RgbaImage::from_pixel(100, 50, image::Rgba([0, 0, 0, 255]));
        let source = RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255]));
        overlay_source(&mut canvas, &source, 0.2);
        assert_eq!(
            canvas.get_pixel(99, 49).0,
            [255, 0, 0, 255],
            "corner not pasted"
        );
        assert_eq!(canvas.get_pixel(0, 0).0, [0, 0, 0, 255], "overlay leaked");
    }

    #[test]
    fn overlay_that_would_not_fit_is_skipped() {
        let mut canvas = RgbaImage::from_pixel(10, 2, image::Rgba([0, 0, 0, 255]));
        let source = RgbaImage::from_pixel(40, 400, image::Rgba([255, 0, 0, 255]));
        overlay_source(&mut canvas, &source, 0.9);
        assert!(canvas.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }
}
