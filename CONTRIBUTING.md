# Contributing to ASCII World

Thanks for stopping by! This project is deliberately small and hackable — a great first
Rust contribution.

## Getting started

```bash
git clone https://github.com/Liohtml/ASCII-World
cd ASCII-World
cargo test            # full suite, runs in seconds
cargo run -- txt tests/fixtures/input.jpg --width 100
```

No system dependencies. The fonts are embedded; the test fixture is in the repo. Only the `video`
subcommand needs anything extra — ffmpeg on your PATH — and its tests skip themselves without it.

## Project map

| Path | What lives there |
|---|---|
| `src/engine.rs` | Core conversion: image → grid of (char, color, alpha) cells |
| `src/charset.rs` | Built-in ramps, the braille mode, runtime glyph-density sorting |
| `src/font.rs` | The font stack: DejaVu, the CJK subset, `--font`, cell metrics |
| `src/render.rs` | Text / ANSI / JSON output |
| `src/paint.rs` | Painting to RGBA, cropping, transparent backgrounds |
| `src/input.rs` | Decoding: raster formats, SVG rasterization, GIF frames |
| `src/anim.rs` | Animated GIF encoding |
| `src/video.rs` | The ffmpeg pipeline |
| `src/mcp.rs` | The stdio MCP server (hand-rolled JSON-RPC, ~150 lines) |
| `src/wasm.rs`, `web/` | Browser bindings and the demo page |
| `src/main.rs` | clap CLI |
| `tests/e2e.rs` | End-to-end tests against the fixture image and the real binary |

## Ground rules

- `cargo fmt`, `cargo clippy --all-targets` (zero warnings), `cargo test` must pass — CI enforces all three.
- New behavior needs a test. The existing tests show the house style: small, deterministic, no snapshots.
- Keep dependencies minimal — part of the point of this project is the single lean binary.
  Adding a crate needs a good reason in the PR description, and heavy ones belong behind a cargo
  feature (see how `svg` gates resvg).
- Anything that only some builds have needs a `#[cfg(feature = "…")]` on its tests too; CI builds
  with the optional features off.
- Performance claims need numbers (see `docs/BENCHMARK.md` for the methodology and
  `cargo run --release --example bench_convert`).
- Quality claims need a measurement, not a screenshot. `braille_resolves_more_detail_than_a_ramp`
  in `tests/e2e.rs` shows the shape: score both options on one metric and assert the gap.

## Good first issues

Check the [issue tracker](https://github.com/Liohtml/ASCII-World/issues) — `good first issue`
labels mark the gentler ones. The original roadmap is done, so new ideas are especially welcome.

## Questions

Open a discussion or issue — happy to help you get a PR over the line.
