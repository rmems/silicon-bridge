<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Independent consumer guide

How to use `silicon-bridge` without Spikenaut, a sibling checkout, or the
author's workstation. This is the packaged **public** API. Implementation
modules under `src/` are not part of the contract.

The crate was **not on crates.io or docs.rs** when this guide was written
(registry and docs.rs lookups for `silicon-bridge` returned 404 on
2026-09-15). A README badge is not publication. See
[release-readiness.md](release-readiness.md) before any `cargo publish`.

## Who owns what

| Actor | Owns |
|---|---|
| Training framework | Meaning of the floats (which values are weights, thresholds, decay) |
| `silicon-bridge` | Q8.8 quantization, checked shapes, `.mem` / JSON layout, UART codecs |
| HDL / firmware | How RAM words are read, UART frame dimensions, signed accumulate |
| This crate's tests | Host encode/decode and optional Icarus `$readmemh` simulation (#53) |

silicon-bridge does **not** interpret trained semantics, program a board, or
prove software–FPGA trajectory parity.

## HDL reader contract

An external `$readmemh` reader must implement all of the following. Changing
only the host crate is not enough.

| Property | Contract |
|---|---|
| Word width | 16 bits |
| Fractional bits | 8 (scale 256); `raw = trunc(value × 256)` toward zero |
| Signedness | Per block in `parameters.json` → `metadata.encodings`. Hardware `.mem` is signed two's complement. Do not infer from the filename or from storing the word as `i16`. |
| Flattening | Dense row-major. Hidden weights: `addr = neuron * num_channels + input`. |
| Filenames | `parameters.mem` (thresholds), `parameters_weights.mem` (hidden `N×M`), `parameters_decay.mem` (decay), optional `parameters_output_weights.mem` (readout `K×N`), `parameters.json` |
| Readout | Exporter writes `K×N` (class row, hidden column). silicon-hdl `OutputLayer` indexes `N×K` (`neuron * K + class`). Both files are recorded under `tests/golden/spikenaut_16/` (#53); they are not the same byte stream. |

Golden signed mappings: `-1 → FF00`, `-0.5 → FF80`, `0 → 0000`, `0.5 → 0080`,
`1 → 0100`, `-128 → 8000`, `127.99609375 → 7FFF`.

`MemFileWriter::write_mem_files` refuses `Q88Encoding::Unsigned` because
silicon-hdl RAM `$signed`-compares (`200.0` as unsigned `C800` would read as
`-56.0`).

## Errors and overflow

Checked export (`try_export`, `write_mem_files`) rejects:

- empty layers / empty blocks
- ragged hidden or readout matrices
- neuron-count mismatches
- readout that is not `K×N`
- `NaN` / infinities
- finite values outside the block's encoding range, under the default
  `RangePolicy::Reject`

`RangePolicy::Saturate` is applied only by `try_export_with_report`, which
returns a `SaturationReport`. `try_export` / `write_mem_files` return
`SaturationRequiresReport` so the clamp list cannot be dropped.

`ParameterExport::export` remains the documented **legacy** wrapper: it
flattens ragged rows and saturates out-of-range / non-finite values. Do not
use it for a bitstream image.

UART `encode_q88_signed` still clamps at ±127.99 (`-128.0 → 8003`). Parameter
`.mem` uses `encode_q88_signed_full` (`-128.0 → 8000`).

## Feature / support matrix

| Path | Feature | What you can do | What it does **not** prove |
|---|---|---|---|
| Export-only | default | Checked Q8.8 `.mem` + JSON | Board load, timing, accuracy |
| Pure codec | default | `encode_stimuli` / `decode_response` | That firmware accepts the frame |
| Optional UART | `uart` | Open a **caller-selected** port; blocking I/O | FPGA identity, stimulus success |

Distinguish three evidence classes:

| Class | Where | Ordinary `cargo test` |
|---|---|---|
| OS compilation | CI matrix (#24): Linux/macOS/Windows default features; Linux `uart` job | yes |
| HDL simulation | `bash tests/golden/hdl/run.sh` (Icarus, optional) | **no** |
| Board testing | Not part of this crate's examples or CI | **no** |

Native serial prerequisites (only if you enable `uart` and open a port):

- Linux: `libudev-dev` for **enumeration** (`list_serial_ports`). Opening a
  named path does not require `libudev`.
- macOS / Windows: no `libudev`.
- Always pass an explicit device (`/dev/ttyUSB0`, `COM4`,
  `/dev/cu.usbserial-…`, `/dev/serial/by-id/…`). `FpgaBridge::new()` is a
  legacy probe helper, not the public path.
- Ordinary examples and tests **never send live stimuli**. `ping()` sends a
  real 16-channel stimulus of `0.1` and is not discovery.

## UART firmware profiles

Custom [`DenseQ88Layout`](../src/fpga_codec.rs) dimensions require **matching
firmware**. Changing host channel counts does not reconfigure the FPGA.

Listed only with #53 golden-byte evidence:

| Profile | Evidence | Firmware claim |
|---|---|---|
| SiliconBridge v3.0 (16 in / 16 out, switch field) | `tests/golden/uart/legacy_v3.json` | Software profile that matches current Basys3 firmware |
| Dense 8 / 32 / 8×10 | `tests/golden/uart/dense_*.json` | Host codec only |

## Examples

See [`examples/README.md`](../examples/README.md). All three binaries compile
against the public crate surface.

## Migration

| Change | What to do |
|---|---|
| Legacy unsigned `.mem` | Re-export with signed `encode_q88_signed_full`. Old unsigned hex above `7FFF` is a different number under `$signed`. |
| Signed parameter / readout | Hidden and readout default signed; set `set_encoding` per block. Read `metadata.encodings`. |
| Legacy UART clipping | Keep `encode_q88_signed` on the wire. Do not reuse it for `.mem`. |
| Timestamps / printing | `set_timestamp` for byte-identical JSON. `write_mem_files` still prints a summary (legacy). |
| `Spikenaut-v2` tag | Layout identifier, not a model. Override with `set_format_version` for generic bundles. Do not silently retag signed-readout JSON as a new schema without a distinct name. |
| Pre-1.0 | Public types are `#[non_exhaustive]` where noted. Field additions (`tns_ns`, `encodings`) need `..Default::default()` in struct literals. No 1.0 stability promise. |

## Publication

Publication is a **separately authorized** action. This guide and the
examples do not run `cargo publish`, flash an FPGA, or implement NIR (see
GitHub [#15](https://github.com/rmems/silicon-bridge/issues/15)).

After a real publish, repeat the packaged-crate smoke test against the
registry version. A path/`cargo package` test does **not** prove crates.io
publication.
