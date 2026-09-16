<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Codec fuzz targets

Pure Q8.8 and UART frame fuzzing for Linear RM-1352. These binaries never
open a serial port and do not enable the `uart` feature.

| Target | What it hammers |
|---|---|
| `q88_codecs` | `encode_q88_signed` / `_full` / `unsigned` plus every `i16`/`u16` round-trip |
| `encode_stimuli` | `encode_stimuli` and `encode_stimuli_legacy_v3` |
| `decode_response` | `decode_response` for arbitrary dense layouts and payloads |
| `chunked_frames` | decoder stability across chunk splits, truncation, and suffixes |

Seed corpora under `corpus/` are the committed `tests/golden/` UART frames and
Q8.8 extrema, not encoder-generated.

## CI vs a long campaign

Compile-only CI uses `cargo +nightly fuzz build --dev --sanitizer none`
with `CC=gcc`, `CXX=g++`, and `g++` as the linker so libFuzzer can find
`libstdc++`. GitHub-hosted Ubuntu already has those tools; install `g++`
if a local image exposes `clang` as `c++`.

Opt-in, after `cargo install cargo-fuzz`:

```bash
cargo fuzz run q88_codecs
cargo fuzz run encode_stimuli
cargo fuzz run decode_response
cargo fuzz run chunked_frames
```

Promote any crash from `fuzz/artifacts/<target>/` into
`tests/fuzz_regressions.rs` so `cargo test` keeps it.

Regenerate seeds after changing golden fixtures:

```bash
python3 fuzz/seed_from_golden.py
```
