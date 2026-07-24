# Benchmark methodology

Honest numbers or none. Here's exactly how the README table was produced (2026-07-11).

## Setup

- Machine: Intel Core Ultra 5 225U, WSL2 (Linux 6.18), NVMe SSD
- Rust: 1.96.0, `--release` profile with LTO
- Python: 3.14, opencv-python-headless 5.0.0, NumPy (fresh `uv` venv)
- Input: `tests/fixtures/input.jpg` (the original project's demo image)
- Task: image → 300-column complex-charset ASCII text file
- Both tools produced an identical 300×82 grid (byte-identical file sizes)

## Commands

```bash
# Rust — 10 runs, wall clock, includes process startup
time for i in $(seq 10); do
  target/release/ascii-world txt tests/fixtures/input.jpg --width 300 -o /tmp/out_rs.txt
done

# Python — original img2txt.py from the legacy-python branch, same 10-run loop
time for i in $(seq 10); do
  python img2txt.py --input tests/fixtures/input.jpg --output /tmp/out_py.txt \
    --num_cols 300 --mode complex
done
```

## Results

| Implementation | avg per run |
|---|---|
| `ascii-world txt` (Rust) | **5 ms** |
| `img2txt.py` (Python + OpenCV) | 253 ms |

Most of the Python time is interpreter + import startup — which is precisely what hurts in
agent workflows and shell pipelines, where each conversion is a fresh process. For long-running
batch use the gap narrows; for CLI/tool-call use it's ~50×.

Want to add your machine? PRs with numbers + hardware info welcome.

## Parallel sampling (the `parallel` feature)

Measured 2026-07-24 on a 4-core container (Linux 6.18, rustc 1.94.1, `--release` with LTO).
The fixture is upscaled 6× to 5856×3228 (18.9 MP) so sampling dominates; only `engine::convert`
is timed, decoding and I/O happen once up front.

```bash
cargo run --release --example bench_convert                        # rayon on
cargo run --release --example bench_convert --no-default-features  # rayon off
```

| Charset | `--width` | sequential | rayon (4 cores) | speedup |
|---|---|---|---|---|
| `complex` | 300 | 22.8 ms | **7.0 ms** | 3.3× |
| `complex` | 1000 | 29.4 ms | **8.7 ms** | 3.4× |
| `braille` | 300 | 30.1 ms | **10.9 ms** | 2.8× |
| `braille` | 1000 | 126.2 ms | **72.7 ms** | 1.7× |

Near-linear for ramp charsets — row sampling is embarrassingly parallel. Braille scales less at
large widths because its error-diffusion pass is inherently sequential: at `--width 1000` that is
a 2000×1100 dot grid walked in raster order, and it grows with the *output* size rather than the
input's.

Output is byte-identical either way (`map_rows` preserves order); verified across the `complex`,
`braille`, `blocks` and `chinese` charsets at two widths:

```bash
cargo build --release && cp target/release/ascii-world /tmp/aw-par
cargo build --release --no-default-features --features cli,cjk,svg
cmp <(/tmp/aw-par txt tests/fixtures/input.jpg --width 300 --json) \
    <(target/release/ascii-world txt tests/fixtures/input.jpg --width 300 --json)
```
