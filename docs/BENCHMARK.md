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
