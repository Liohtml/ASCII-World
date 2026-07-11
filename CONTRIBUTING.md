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

No system dependencies. The font is embedded; the test fixture is in the repo.

## Project map

| Path | What lives there |
|---|---|
| `src/engine.rs` | Core conversion: image → grid of (char, color) cells |
| `src/charset.rs` | Built-in ramps + runtime glyph-density sorting |
| `src/render.rs` | Text / ANSI / JSON output |
| `src/paint.rs` | PNG painting with the embedded font |
| `src/mcp.rs` | The stdio MCP server (hand-rolled JSON-RPC, ~150 lines) |
| `src/main.rs` | clap CLI |
| `tests/e2e.rs` | End-to-end tests against the fixture image |

## Ground rules

- `cargo fmt`, `cargo clippy --all-targets` (zero warnings), `cargo test` must pass — CI enforces all three.
- New behavior needs a test. The existing tests show the house style: small, deterministic, no snapshots.
- Keep dependencies minimal — part of the point of this project is the single lean binary.
  Adding a crate needs a good reason in the PR description.
- Performance claims need numbers (see `docs/BENCHMARK.md` for the methodology).

## Good first issues

Check the [issue tracker](https://github.com/Liohtml/ASCII-World/issues) — roadmap items are
filed as issues with scope notes. `good first issue` labels mark the gentler ones.

## Questions

Open a discussion or issue — happy to help you get a PR over the line.
