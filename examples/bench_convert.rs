//! Time `engine::convert` on a large input.
//!
//! Used to justify the `parallel` feature. Run it both ways:
//!
//! ```sh
//! cargo run --release --example bench_convert                       # rayon on
//! cargo run --release --example bench_convert --no-default-features # rayon off
//! ```
//!
//! Only the conversion is timed — decoding and I/O happen once, up front, so
//! the number is the engine and nothing else.

use ascii_world::charset::Charset;
use ascii_world::{charset, engine};
use std::time::Instant;

/// Upscale factor applied to the fixture, to reach a size where sampling
/// dominates and per-call overhead does not.
const SCALE: u32 = 6;
const RUNS: u32 = 10;

fn main() {
    let source = image::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/input.jpg"
    ))
    .expect("fixture image")
    .to_rgba8();

    let (w, h) = (source.width() * SCALE, source.height() * SCALE);
    let big = image::imageops::resize(&source, w, h, image::imageops::FilterType::Nearest);
    let megapixels = (w as f64 * h as f64) / 1e6;
    println!(
        "input {w}x{h} ({megapixels:.1} MP), parallel = {}",
        cfg!(feature = "parallel")
    );

    for (label, cs) in [
        ("complex", charset::resolve("complex").unwrap()),
        ("braille", Charset::Braille),
    ] {
        for width in [300, 1000] {
            let opts = engine::Options {
                width,
                charset: cs.clone(),
                ..engine::Options::default()
            };
            // Warm up: first touch of a fresh buffer pays page faults.
            let grid = engine::convert(&big, &opts).expect("conversion");

            let start = Instant::now();
            for _ in 0..RUNS {
                std::hint::black_box(engine::convert(&big, &opts).expect("conversion"));
            }
            let per_run = start.elapsed().as_secs_f64() * 1000.0 / RUNS as f64;
            println!(
                "  {label:<8} --width {width:<5} {per_run:7.1} ms/run   ({}x{} cells)",
                grid.cols, grid.rows
            );
        }
    }
}
