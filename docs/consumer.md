<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Independent consumer guide

How to use `silicon-bridge` without Spikenaut, a sibling checkout, or the
author's workstation. This is the packaged **public** API. Implementation
modules under `src/` are not part of the contract.

The crate is **not on crates.io or docs.rs**. `[package].version` is
`0.3.1`, but an authorized `cargo publish` is still a separate maintainer
action. Until then, depend on git or a path. A README badge is not
publication. See [release-readiness.md](release-readiness.md).

```toml
silicon-bridge = { git = "https://github.com/rmems/silicon-bridge" }
```

After crates.io actually serves version 0.3.1:

```toml
silicon-bridge = "0.3.1"
```

## Bring your own floats

You already have trained `f32` values. This crate does not train a network
and does not interpret what the floats mean.

| Input | Shape | Output file |
|---|---|---|
| Thresholds | `N` | `parameters.mem` |
| Hidden weights | `N×M` dense row-major | `parameters_weights.mem` |
| Decay | `N` | `parameters_decay.mem` |
| Optional readout | `K×N` (class row, hidden column) | `parameters_output_weights.mem` |
| Manifest | — | `parameters.json` |

```rust
use silicon_bridge::FpgaParameterExporter;

let mut exporter = FpgaParameterExporter::from_params(
    thresholds,  // Vec<f32>, length N
    weights,     // Vec<Vec<f32>>, N rows × M columns
    decay,       // Vec<f32>, length N
);
// exporter.set_output_weights(readout); // optional K×N

let params = exporter.try_export()?; // rejects empty/ragged/NaN/overflow
let report = exporter.write_generic("fpga_output")?; // generic-dense-q88
```

`try_export` uses `RangePolicy::Reject` by default. Hardware `.mem` is signed
two's complement Q8.8. The generic path writes no Spikenaut tag, no timestamp,
and no `target_latency_us`. Existing files are refused unless you pass
`ExportConfig::generic().allow_replace()`. Prefer `write_generic` over
`ParameterExport::export` / `MemFileWriter::write_mem_files`.

Required signed `K×N` readout: `ExportConfig::generic_with_required_readout()`
(`spikenaut_signed_output_v1()` is the Spikenaut-named alias). Spikenaut-v2
files are **opt-in**: `MemFileWriter::write_mem_files` /
`ExportConfig::legacy_spikenaut_v2()`.

## Who owns what

| Actor | Owns |
|---|---|
| Training framework | Meaning of the floats (which values are weights, thresholds, decay) |
| `silicon-bridge` | Q8.8 quantization, checked shapes, `.mem` / JSON layout, UART codecs |
| HDL / firmware | How RAM words are read, UART frame dimensions, signed accumulate |
| This crate's tests | Host encode/decode and optional Icarus `$readmemh` simulation (#53) |

silicon-bridge does **not** interpret trained semantics, program a board, or
prove software–FPGA trajectory parity.

## Reference hardware

The default public path is **board-agnostic** Q8.8 `.mem` (`write_generic`).
The **named reference companion path** is Digilent Basys 3 plus
[`rmems/silicon-hdl`](https://github.com/rmems/silicon-hdl). That is one
named reference, not a multi-board claim.

| Field | Value |
|---|---|
| Board | **Digilent Basys 3** |
| FPGA | **Xilinx Artix-7** `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`) |
| Companion RTL | `spikenaut_soc_basys3_top` (`Basys3_Top.sv`) |
| Constraints | `constraints/basys3.xdc`, `basys3_soc.xdc` |
| Host UART | SiliconBridge **v3.0** (16 in / 16 out) — example layout with golden bytes matching Basys 3 firmware, not “any board” |
| Prior board smoke | silicon-hdl [#68](https://github.com/rmems/silicon-hdl/issues/68) / [`docs/phase-c-board-smoke.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/phase-c-board-smoke.md) — LED heartbeat **PASS** only; **not** a silicon-bridge UART host session |

silicon-hdl also ships `constraints/artix7_trainer.xdc`. That pinout is
**not** the claimed reference demo (`build_soc.tcl` uses the Basys 3 XDCs
only). A silicon-bridge UART host session on Basys 3 is the 0.3.1 publish
gate ([#84](https://github.com/rmems/silicon-bridge/issues/84)); this
guide does not flash a board.

Companion contracts:
[silicon-hdl README](https://github.com/rmems/silicon-hdl#readme),
[`docs/interface-alignment.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/interface-alignment.md),
[`docs/host-soc-e2e.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/host-soc-e2e.md).

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

For the pinned silicon-hdl v3 reference contract, use
`ExportConfig::silicon_hdl_v3()`. It supports exactly 16 input channels, 16
hidden neurons, and 3 output classes; it preserves
`parameters_output_weights.mem` as the generic class-major `K×N` readout and
also emits `hdl_readout_neuron_major.mem` as the HDL-native neuron-major
`N×K` image. See [host-hdl-contract.md](host-hdl-contract.md).

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
| Board testing | Not part of this crate's examples or CI. silicon-hdl LED heartbeat smoke is not a UART host session | **no** |

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

Listed **example layouts** with #53 golden-byte evidence. SiliconBridge
v3.0 matches current Basys 3 firmware; that is not a claim that any board
speaks the frame, and it is not a claim that this crate only exports for
Basys 3. Custom [`DenseQ88Layout`](../src/fpga_codec.rs) dimensions require
**matching firmware**. Changing host channel counts does not reconfigure
the FPGA.

| Profile | Evidence | What it is |
|---|---|---|
| SiliconBridge v3.0 (16 in / 16 out, switch field) | `tests/golden/uart/legacy_v3.json` | Example layout with golden bytes matching Basys 3 firmware |
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
| `Spikenaut-v2` tag | Layout identifier, not a model. The default checked path omits it. Use `ExportConfig::legacy_spikenaut_v2()` / `write_mem_files` when a consumer keys on that string. Prefer `write_generic`. Required readout: `ExportConfig::generic_with_required_readout()`. Pinned silicon-hdl reference export: `ExportConfig::silicon_hdl_v3()`. |
| Timestamps / printing | `set_timestamp` requires RFC 3339 UTC. The checked path omits timestamps by default. `write_mem_files` records a wall-clock stamp and does not print. |
| Pre-1.0 | Public types are `#[non_exhaustive]` where noted. Field additions (`tns_ns`, `encodings`) need `..Default::default()` in struct literals. No 1.0 stability promise. |

## Publication

Publication is a **separately authorized** action. This guide and the
examples do not run `cargo publish`, flash an FPGA, or implement NIR (see
GitHub [#15](https://github.com/rmems/silicon-bridge/issues/15)).

This tree is packaged as **0.3.1**, but crates.io / docs.rs are **not** live
until a maintainer runs a real `cargo publish`. After a real publish, confirm
the registry and repeat the packaged-crate smoke test against the **registry**
version. A path/`cargo package` test does **not** prove crates.io publication.
