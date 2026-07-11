//! End-to-end tests: run the real pipeline against the bundled fixture image.

use ascii_world::{charset, engine, paint, render};

fn fixture() -> image::RgbImage {
    image::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/input.jpg"
    ))
    .expect("fixture image must load")
    .to_rgb8()
}

#[test]
fn fixture_converts_to_text_with_expected_shape() {
    let ramp = charset::resolve("complex").unwrap();
    let grid = engine::convert(
        &fixture(),
        &engine::Options {
            width: 150,
            charset: ramp,
            invert: false,
            aspect: 2.0,
        },
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
    let ramp = charset::resolve("complex").unwrap();
    let grid = engine::convert(
        &fixture(),
        &engine::Options {
            width: 80,
            charset: ramp,
            invert: false,
            aspect: 2.0,
        },
    )
    .unwrap();
    for (bg, colored) in [
        (paint::Background::Black, false),
        (paint::Background::White, false),
        (paint::Background::Black, true),
    ] {
        let img = paint::paint_png(&grid, bg, colored, 12.0).unwrap();
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
    let ramp = charset::resolve("simple").unwrap();
    let grid = engine::convert(
        &fixture(),
        &engine::Options {
            width: 60,
            charset: ramp.clone(),
            invert: false,
            aspect: 2.0,
        },
    )
    .unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&render::to_json(&grid, &ramp, true)).unwrap();
    assert_eq!(parsed["cols"].as_u64().unwrap(), 60);
    let lines = parsed["lines"].as_array().unwrap();
    let colors = parsed["colors"].as_array().unwrap();
    assert_eq!(lines.len(), colors.len());
    assert!(colors[0][0].as_str().unwrap().starts_with('#'));
}
