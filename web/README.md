# Web demo

The engine compiled to WebAssembly, plus a one-page drag-and-drop UI. Deployed to GitHub Pages by
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on every push to `master`:
**<https://liohtml.github.io/ASCII-World/>**

Images never leave the browser — the whole conversion runs in wasm.

## Build it locally

```bash
cargo install wasm-bindgen-cli --version "$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/[";]/,"",$3); print $3; exit}' ../Cargo.lock)" --locked

cargo build --release --target wasm32-unknown-unknown \
    --no-default-features --features wasm,cjk,svg
wasm-bindgen target/wasm32-unknown-unknown/release/ascii_world.wasm \
    --out-dir web/pkg --target web

python3 -m http.server -d web 8000   # http://localhost:8000
```

Run both commands from the repository root. The CLI's version has to match the `wasm-bindgen`
crate exactly, hence reading it out of the lockfile.

`web/pkg/` is generated and git-ignored.

## What the module exposes

| Function | Returns |
|---|---|
| `imageToAscii(bytes, width, charset, invert)` | plain text |
| `imageToJson(bytes, width, charset, invert, includeColors)` | the CLI's `--json` document, as a string |
| `charsetNames()` | every built-in charset name |
| `version()` | the crate version the module was built from |

`bytes` is a `Uint8Array` of any supported input file — PNG, JPEG, GIF, WebP, BMP or SVG.

The wasm bundle is ~2.1 MB (~885 KB gzipped, which is what Pages serves); roughly 40% of that is
the SVG rasterizer. Drop `svg` from the feature list for a ~1.2 MB build if you don't need vector
input.
