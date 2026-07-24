//! End-to-end tests: run the real pipeline against the bundled fixture image,
//! and the real binary against real files.

use ascii_world::charset::Charset;
use ascii_world::input::Frame;
use ascii_world::paint::{Background, PaintOptions};
use ascii_world::{anim, charset, engine, font, input, paint, render};
use image::{Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> RgbaImage {
    image::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/input.jpg"
    ))
    .expect("fixture image must load")
    .to_rgba8()
}

fn fixture_path() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/input.jpg")
}

fn options(width: u32, charset: Charset) -> engine::Options {
    engine::Options {
        width,
        charset,
        ..engine::Options::default()
    }
}

/// A scratch path under the target dir, so tests never collide.
fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("e2e");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ascii-world"))
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("binary runs");
    assert!(
        out.status.success(),
        "{cmd:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("stdout is utf-8")
}

// --- library pipeline ---------------------------------------------------

#[test]
fn fixture_converts_to_text_with_expected_shape() {
    let grid = engine::convert(
        &fixture(),
        &options(150, charset::resolve("complex").unwrap()),
    )
    .unwrap();
    assert_eq!(grid.cols, 150);
    assert!(grid.rows > 10, "fixture should yield a real grid");

    let text = render::to_text(&grid);
    assert_eq!(text.lines().count(), grid.rows as usize);
    // A photo must produce more than one distinct character.
    let distinct: std::collections::HashSet<char> = text.chars().collect();
    assert!(
        distinct.len() > 5,
        "expected varied output, got {distinct:?}"
    );
}

#[test]
fn fixture_renders_to_png_in_all_modes() {
    let cs = charset::resolve("complex").unwrap();
    let grid = engine::convert(&fixture(), &options(80, cs.clone())).unwrap();
    for (bg, colored) in [
        (Background::Black, false),
        (Background::White, false),
        (Background::Black, true),
        (Background::Transparent, true),
    ] {
        let opts = PaintOptions::new(font::embedded(), &cs, 12.0)
            .unwrap()
            .background(bg)
            .colored(colored);
        let img = paint::paint_png(&grid, font::embedded(), &opts).unwrap();
        assert!(img.width() > 0 && img.height() > 0);
        // Output must not be a flat canvas.
        let first = *img.get_pixel(0, 0);
        assert!(
            img.pixels().any(|p| *p != first),
            "PNG output is flat for bg={bg:?} colored={colored}"
        );
    }
}

#[test]
fn fixture_json_output_is_valid_and_complete() {
    let cs = charset::resolve("simple").unwrap();
    let grid = engine::convert(&fixture(), &options(60, cs.clone())).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&render::to_json(&grid, &cs, false, true)).unwrap();
    assert_eq!(parsed["cols"].as_u64().unwrap(), 60);
    assert_eq!(parsed["mode"], "ramp");
    let lines = parsed["lines"].as_array().unwrap();
    let colors = parsed["colors"].as_array().unwrap();
    assert_eq!(lines.len(), colors.len());
    assert!(colors[0][0].as_str().unwrap().starts_with('#'));
    // An opaque JPEG carries no alpha, so the field stays absent.
    assert!(parsed.get("alpha").is_none());
}

#[test]
fn braille_resolves_more_detail_than_a_ramp() {
    // The acceptance criterion for the braille charset is "recognizably
    // sharper detail than `complex` at the same width". Measure it: rebuild
    // both outputs at sub-cell resolution and score them against the source
    // downsampled to that same resolution.
    let src = fixture();
    let ramp = charset::resolve("complex").unwrap();
    let braille = engine::convert(&src, &options(60, Charset::Braille)).unwrap();
    let cells = engine::convert(&src, &options(60, ramp.clone())).unwrap();
    assert_eq!((braille.cols, braille.rows), (cells.cols, cells.rows));
    assert!(braille
        .chars
        .iter()
        .all(|&c| c == ' ' || ('\u{2800}'..='\u{28FF}').contains(&c)));

    // Score only the signal that lives *inside* a cell — variation around
    // each cell's own mean. That is exactly the detail one-character-per-cell
    // throws away: a ramp predicts a flat cell, so it scores 50% here by
    // construction, whatever ramp or threshold it uses. Braille has to beat
    // that convincingly to justify the mode.
    let (dot_w, dot_h) = (braille.cols * 2, braille.rows * 4);
    // The grid covers whole cells only, so the source's leftover bottom strip
    // is outside the conversion. Resizing the full image would shear the two
    // grids apart row by row.
    let cell_h = src.width() as f64 / braille.cols as f64 * 2.0;
    let covered = (braille.rows as f64 * cell_h) as u32;
    let truth = image::imageops::resize(
        &*image::imageops::crop_imm(&src, 0, 0, src.width(), covered),
        dot_w,
        dot_h,
        image::imageops::FilterType::Triangle,
    );
    let luma = |p: &Rgba<u8>| 0.299 * p.0[0] as f32 + 0.587 * p.0[1] as f32 + 0.114 * p.0[2] as f32;

    let ramp = ramp.ramp().unwrap();
    let (mut braille_hits, mut ramp_hits, mut scored) = (0u32, 0u32, 0u32);
    for row in 0..braille.rows {
        for col in 0..braille.cols {
            let dots: Vec<f32> = (0..8)
                .map(|i| luma(truth.get_pixel(col * 2 + i % 2, row * 4 + i / 2)))
                .collect();
            let mean = dots.iter().sum::<f32>() / 8.0;
            let sd = (dots.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / 8.0).sqrt();
            // Only judge cells that *have* sub-cell structure. In flat areas
            // there is nothing to resolve and dithering is free to place its
            // dots wherever keeps the tone right.
            if sd < 20.0 {
                continue;
            }

            let ch = braille.char_at(row, col);
            let bits = if ch == ' ' { 0 } else { ch as u32 - 0x2800 };
            // One character per cell means one tone for all eight dots.
            let idx = ramp
                .iter()
                .position(|&c| c == cells.char_at(row, col))
                .expect("ramp character");
            let cell_tone = idx as f32 / (ramp.len() - 1) as f32 * 255.0;

            for (i, dot) in dots.iter().enumerate() {
                let bit = match (i % 2, i / 2) {
                    (0, 0) => 0x01,
                    (0, 1) => 0x02,
                    (0, 2) => 0x04,
                    (0, 3) => 0x40,
                    (1, 0) => 0x08,
                    (1, 1) => 0x10,
                    (1, 2) => 0x20,
                    _ => 0x80,
                };
                let dark = *dot < mean;
                braille_hits += u32::from((bits & bit != 0) == dark);
                ramp_hits += u32::from((cell_tone < mean) == dark);
                scored += 1;
            }
        }
    }

    assert!(
        scored > 500,
        "not enough textured cells to judge ({scored})"
    );
    let (braille_acc, ramp_acc) = (
        braille_hits as f32 / scored as f32,
        ramp_hits as f32 / scored as f32,
    );
    assert!(
        braille_acc > 0.65 && braille_acc > ramp_acc + 0.15,
        "sub-cell structure: braille {braille_acc:.3} vs ramp {ramp_acc:.3} over {scored} dots"
    );
}

#[test]
fn transparent_logo_stays_a_cutout_through_the_whole_pipeline() {
    // A red disc on a fully transparent canvas, as exported by any design tool.
    let mut src = RgbaImage::from_pixel(120, 120, Rgba([0, 0, 0, 0]));
    for y in 0..120u32 {
        for x in 0..120u32 {
            let (dx, dy) = (x as f32 - 60.0, y as f32 - 60.0);
            if dx * dx + dy * dy < 40.0 * 40.0 {
                src.put_pixel(x, y, Rgba([220, 30, 30, 255]));
            }
        }
    }
    let png = tmp("logo.png");
    src.save(&png).unwrap();

    // txt: the corners must be blank with default flags.
    let text = run(cli().args(["txt", png.to_str().unwrap(), "--width", "40"]));
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].trim().is_empty(),
        "top row not blank:\n{}",
        &text[..text.len().min(400)]
    );
    assert!(
        lines[lines.len() / 2].trim().len() > 3,
        "middle row lost the subject"
    );

    // img --bg transparent: a canvas whose only opaque pixels are glyphs.
    let out = tmp("logo_ascii.png");
    run(cli().args([
        "img",
        png.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--width",
        "40",
        "--bg",
        "transparent",
        "--color",
    ]));
    let painted = image::open(&out).unwrap().to_rgba8();
    assert!(painted.pixels().any(|p| p.0[3] == 0), "no transparency");
    assert!(painted.pixels().any(|p| p.0[3] > 200), "no glyphs");
    // Reds survived; nothing turned into a black box.
    assert!(painted
        .pixels()
        .filter(|p| p.0[3] > 200)
        .any(|p| p.0[0] > 150 && p.0[1] < 120));
}

#[cfg(feature = "svg")]
#[test]
fn svg_input_rasterizes_and_converts() {
    let svg = tmp("shape.svg");
    std::fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40">
             <rect x="0" y="0" width="40" height="40" fill="#101010"/>
           </svg>"##,
    )
    .unwrap();

    let json = run(cli().args([
        "txt",
        svg.to_str().unwrap(),
        "--width",
        "20",
        "--charset",
        "simple",
        "--json",
    ]));
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cols"], 20);
    let first_line = parsed["lines"][0].as_str().unwrap();
    // Left half is near-black ink, right half is transparent → blank.
    assert!(
        first_line[..10].contains('@'),
        "left half not inked: {first_line:?}"
    );
    assert!(
        first_line.chars().skip(10).all(|c| c == ' '),
        "right half not blank: {first_line:?}"
    );
    // Transparency means the alpha field is present.
    assert!(parsed["alpha"].is_array());
}

#[test]
fn animated_gif_round_trips_through_the_cli() {
    // Build a two-frame source animation: black frame, then white frame.
    let src = tmp("blink.gif");
    let frames: Vec<Frame> = [0u8, 255]
        .into_iter()
        .map(|v| Frame {
            image: RgbaImage::from_pixel(64, 32, Rgba([v, v, v, 255])),
            delay_ms: 80,
        })
        .collect();
    anim::write_gif(&src, &frames).unwrap();

    let out = tmp("blink_ascii.gif");
    run(cli().args([
        "img",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--width",
        "20",
        "--color",
        "--no-crop",
    ]));

    let bytes = std::fs::read(&out).unwrap();
    let decoded = input::decode_frames(&bytes, 256).unwrap();
    assert_eq!(decoded.len(), 2, "animation collapsed to a still");
    assert_eq!(decoded[0].delay_ms, 80, "frame timing lost");
    assert_eq!(
        decoded[0].image.dimensions(),
        decoded[1].image.dimensions(),
        "frames must share one canvas"
    );
    // Default invert on a black background: the black frame draws dense
    // glyphs, the white frame draws none.
    let lit = |f: &Frame| f.image.pixels().filter(|p| p.0[0] > 128).count();
    assert!(
        lit(&decoded[1]) > lit(&decoded[0]),
        "frames are indistinguishable ({} vs {})",
        lit(&decoded[0]),
        lit(&decoded[1])
    );
}

#[test]
fn animation_cropping_keeps_every_frame_whole() {
    // Frame 1 is blank, frame 2 draws top-left, frame 3 draws bottom-right.
    // Cropping to the first frame that has content would guillotine the
    // other corner, and a blank first frame would leave frames of different
    // sizes — which is not encodable at all.
    let blank = RgbaImage::from_pixel(80, 80, Rgba([255, 255, 255, 255]));
    let mut top_left = blank.clone();
    let mut bottom_right = blank.clone();
    for y in 0..20 {
        for x in 0..20 {
            top_left.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            bottom_right.put_pixel(x + 60, y + 60, Rgba([0, 0, 0, 255]));
        }
    }
    let src = tmp("moving.gif");
    anim::write_gif(
        &src,
        &[blank, top_left, bottom_right]
            .into_iter()
            .map(|image| Frame {
                image,
                delay_ms: 50,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();

    let out = tmp("moving_ascii.gif");
    run(cli().args([
        "img",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--width",
        "40",
        "--bg",
        "white",
    ]));

    let decoded = input::decode_frames(&std::fs::read(&out).unwrap(), 256).unwrap();
    assert_eq!(decoded.len(), 3);
    let size = decoded[0].image.dimensions();
    assert!(
        decoded.iter().all(|f| f.image.dimensions() == size),
        "frames disagree on size: {:?}",
        decoded
            .iter()
            .map(|f| f.image.dimensions())
            .collect::<Vec<_>>()
    );

    // Both corners survived the crop, each in its own frame and its own half.
    let ink = |f: &Frame, right: bool, bottom: bool| {
        let (w, h) = f.image.dimensions();
        let xs = if right { w / 2..w } else { 0..w / 2 };
        let ys = if bottom { h / 2..h } else { 0..h / 2 };
        ys.flat_map(|y| xs.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| f.image.get_pixel(x, y).0[0] < 128)
            .count()
    };
    assert!(
        ink(&decoded[1], false, false) > 0,
        "top-left frame lost its ink"
    );
    assert!(
        ink(&decoded[2], true, true) > 0,
        "bottom-right frame was cropped away"
    );
}

#[test]
fn cropping_trims_the_canvas_and_can_be_turned_off() {
    // Content in the middle only, so there is something to trim.
    let mut src = RgbaImage::from_pixel(80, 80, Rgba([255, 255, 255, 255]));
    for y in 30..50 {
        for x in 30..50 {
            src.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }
    let png = tmp("centered.png");
    src.save(&png).unwrap();

    let cropped = tmp("centered_cropped.png");
    let full = tmp("centered_full.png");
    for (out, extra) in [(&cropped, vec![]), (&full, vec!["--no-crop"])] {
        let mut cmd = cli();
        cmd.args([
            "img",
            png.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--width",
            "40",
            "--bg",
            "white",
        ]);
        cmd.args(extra);
        run(&mut cmd);
    }
    let cropped = image::open(&cropped).unwrap();
    let full = image::open(&full).unwrap();
    assert!(
        cropped.width() < full.width() && cropped.height() < full.height(),
        "crop did not trim ({}x{} vs {}x{})",
        cropped.width(),
        cropped.height(),
        full.width(),
        full.height()
    );
}

#[cfg(feature = "cjk")]
#[test]
fn cjk_charsets_render_real_glyphs_not_blanks() {
    let out = tmp("cjk.png");
    run(cli().args([
        "img",
        fixture_path(),
        "-o",
        out.to_str().unwrap(),
        "--width",
        "40",
        "--charset",
        "chinese",
        "--no-crop",
    ]));
    let img = image::open(&out).unwrap().to_rgba8();
    let lit = img.pixels().filter(|p| p.0[0] > 128).count();
    let total = (img.width() * img.height()) as usize;
    assert!(
        lit > total / 100,
        "chinese charset painted {lit}/{total} pixels — the font fallback is missing"
    );
}

#[test]
fn cli_surface_still_works() {
    // charsets lists every built-in, including the new ones.
    let listed = run(cli().arg("charsets"));
    for name in charset::NAMED {
        assert!(listed.contains(name), "{name} missing from `charsets`");
    }

    // stdin streaming.
    let text = run(cli().args([
        "txt",
        fixture_path(),
        "--width",
        "40",
        "--charset",
        "simple",
    ]));
    assert!(text.lines().count() > 3);

    // ANSI output carries truecolor escapes.
    let ansi = run(cli().args(["txt", fixture_path(), "--width", "40", "--ansi"]));
    assert!(ansi.contains("\x1b[38;2;"));

    // Unknown charsets fail loudly rather than silently defaulting.
    let out = cli()
        .args(["txt", fixture_path(), "--charset", "klingon"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown charset"));
}

// --- video (skipped when ffmpeg is not installed) -----------------------

fn has_ffmpeg() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

#[test]
fn video_converts_frame_by_frame() {
    if !has_ffmpeg() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let src = tmp("clip.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=1:size=96x64:rate=8")
        .args(["-pix_fmt", "yuv420p"])
        .arg(&src)
        .status()
        .expect("ffmpeg runs");
    assert!(made.success(), "could not synthesize a test clip");

    let out = tmp("clip_ascii.mp4");
    run(cli().args([
        "video",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--width",
        "40",
        "--color",
        "--overlay",
        "0.2",
    ]));

    let spec = ascii_world::video::probe(&out).unwrap();
    assert!(spec.width > 0 && spec.height > 0);
    assert_eq!(spec.width % 2, 0, "h.264 needs even dimensions");
    assert_eq!(spec.height % 2, 0, "h.264 needs even dimensions");
    assert!(
        (spec.fps - 8.0).abs() < 0.5,
        "fps not preserved: {}",
        spec.fps
    );
    assert!(std::fs::metadata(&out).unwrap().len() > 0);
}

#[test]
fn video_to_gif_uses_the_gif_encoder() {
    if !has_ffmpeg() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let src = tmp("clip_for_gif.mp4");
    Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=1:size=64x48:rate=6")
        .args(["-pix_fmt", "yuv420p"])
        .arg(&src)
        .status()
        .expect("ffmpeg runs");

    let out = tmp("clip_ascii.gif");
    run(cli().args([
        "video",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--width",
        "24",
        "--fps",
        "6",
    ]));
    let bytes = std::fs::read(&out).unwrap();
    assert!(input::is_gif(&bytes));
    assert!(input::decode_frames(&bytes, 256).unwrap().len() > 1);
}

#[test]
fn video_without_ffmpeg_fails_with_an_actionable_message() {
    // Only meaningful where ffmpeg is absent; where it exists, a missing
    // input file has to produce a clear error too.
    let out = cli()
        .args(["video", "/nonexistent/clip.mp4", "-o"])
        .arg(tmp("never.mp4"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        ["ffmpeg", "ffprobe", "no usable video stream"]
            .iter()
            .any(|hint| stderr.contains(hint)),
        "unhelpful error: {stderr}"
    );
}
