# AGENTS.md

Guidance for AI coding agents working in this repository.

## What this is

`ascii-world`: a Rust CLI + library that converts images to ASCII art (text, ANSI, JSON, PNG)
and exposes itself to agents as an MCP server (`ascii-world mcp`).

## Build & verify

```bash
cargo build --release          # binary at target/release/ascii-world
cargo test                     # full suite, < 30 s cold
cargo clippy --all-targets     # must be warning-free
cargo fmt --check
```

Quick smoke test: `cargo run -- txt tests/fixtures/input.jpg --width 80`

## Architecture (5 files, read in this order)

1. `src/engine.rs` — `convert(&RgbImage, &Options) -> AsciiGrid`. Pure, no I/O.
2. `src/charset.rs` — ramps ordered **dark → light**; `resolve()` maps CLI names to ramps.
3. `src/render.rs` — `AsciiGrid` → text / ANSI / JSON strings.
4. `src/paint.rs` — `AsciiGrid` → `RgbImage` via the embedded DejaVu font.
5. `src/mcp.rs` — newline-delimited JSON-RPC 2.0 over stdio; one tool: `image_to_ascii`.

`src/main.rs` is the only place that touches the filesystem or stdin/stdout (besides mcp.rs).

## Invariants to preserve

- Charset ramps run dark → light; `engine::convert` maps luma 0 → index 0.
- `--json` output shape (`cols`, `rows`, `charset`, `lines`, `colors`) is a public contract —
  downstream agents parse it. Add fields, never rename or remove. The `charset` field is the
  ramp *as applied* (`render::effective_ramp`): index 0 is always the darkest-cell character,
  even when `--invert` was used.
- The MCP tool name `image_to_ascii` and its input schema are a public contract.
- The binary must stay standalone: no runtime file dependencies (fonts are `include_bytes!`).
- Zero clippy warnings; every public function has a doc comment.

## Testing conventions

Unit tests live next to the code (`#[cfg(test)]`), e2e tests in `tests/e2e.rs` use
`tests/fixtures/input.jpg`. Tests are deterministic — no randomness, no network, no timing.
