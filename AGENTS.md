# AGENTS.md

Guidance for AI coding agents working in this repository.

## What this is

`ascii-world`: a Rust CLI + library that converts images, GIFs and video to ASCII art (text, ANSI,
JSON, PNG, GIF, MP4) and exposes itself to agents as an MCP server (`ascii-world mcp`).

## Build & verify

```bash
cargo build --release          # binary at target/release/ascii-world
cargo test                     # full suite, < 30 s cold
cargo clippy --all-targets     # must be warning-free
cargo fmt --check
```

Quick smoke test: `cargo run -- txt tests/fixtures/input.jpg --width 80`

Feature combinations CI also builds — check them before touching `Cargo.toml` or any `#[cfg]`:

```bash
cargo test --no-default-features --features cli          # no rayon, no CJK font, no SVG
cargo build --release --target wasm32-unknown-unknown \
    --no-default-features --features wasm,cjk,svg        # the browser build
```

The `video` tests skip themselves when ffmpeg is missing — install it to actually exercise them.

## Architecture (read in this order)

1. `src/engine.rs` — `convert(&RgbaImage, &Options) -> AsciiGrid`. Pure, no I/O. Two sampling
   modes: one character per cell from a ramp, or a 2×4 braille dot grid.
2. `src/charset.rs` — `Charset::Ramp` ordered **dark → light**, or `Charset::Braille`;
   `resolve()` maps CLI names to either.
3. `src/font.rs` — `FontStack`: DejaVu Sans Mono, the CJK subset, plus any `--font`. Owns glyph
   lookup and cell metrics.
4. `src/render.rs` — `AsciiGrid` → text / ANSI / JSON strings.
5. `src/paint.rs` — `AsciiGrid` → `RgbaImage`, with cropping and transparent backgrounds.
6. `src/input.rs` — bytes → `RgbaImage`, including SVG rasterization and GIF frame decoding.
7. `src/anim.rs` / `src/video.rs` — the same pipeline per frame, out to GIF / ffmpeg.
8. `src/mcp.rs` — newline-delimited JSON-RPC 2.0 over stdio; one tool: `image_to_ascii`.
9. `src/wasm.rs` — thin `wasm-bindgen` wrapper; the demo page lives in `web/`.

`src/main.rs` is the only place that parses arguments; `input.rs`, `anim.rs`, `video.rs` and
`mcp.rs` are the only other modules that touch the filesystem or processes.

## Invariants to preserve

- Charset ramps run dark → light; `engine::convert` maps luma 0 → index 0.
- `--json` output shape (`cols`, `rows`, `charset`, `mode`, `lines`, `colors`, `alpha`) is a public
  contract — downstream agents parse it. Add fields, never rename or remove. `charset` is the ramp
  *as applied* (`render::effective_ramp`): index 0 is always the darkest-cell character, even when
  `--invert` was used. In braille mode `charset` is the literal `"braille"` (dots are composed, not
  indexed) — `mode` is what tells the two apart. `alpha` appears only when the source had
  transparency, so opaque inputs still produce the document older parsers expect.
- The MCP tool name `image_to_ascii` and its input schema are a public contract.
- The binary must stay standalone: no runtime file dependencies (fonts are `include_bytes!`). The
  one exception is `video`, which needs ffmpeg on PATH and says so when it is missing.
- Cells below `Options::alpha_threshold` render blank regardless of charset or `invert`. That rule
  is what keeps cutouts cutouts; do not let a ramp lookup override it.
- Sampling parallelism must stay invisible: `map_rows` is order-preserving, and output has to be
  byte-identical with and without the `parallel` feature.
- Animations (GIF, video) measure the crop box once and reuse it — frames of differing size are not
  encodable.
- Zero clippy warnings; every public function has a doc comment.

## Testing conventions

Unit tests live next to the code (`#[cfg(test)]`), e2e tests in `tests/e2e.rs` use
`tests/fixtures/input.jpg` and drive the real binary via `CARGO_BIN_EXE_ascii-world`. Tests are
deterministic — no randomness, no network, no timing. Feature-dependent tests carry the matching
`#[cfg(feature = "…")]` so the reduced-feature CI job stays green.

Quality claims get measured, not asserted by eye: see `braille_resolves_more_detail_than_a_ramp`
for the pattern (score both approaches on the same metric, assert the gap).
