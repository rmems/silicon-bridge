<p align="center">
  <img src="docs/logo.png" width="220" alt="Spikenaut">
</p>

<h1 align="center">silicon-bridge</h1>
<p align="center">SNN-to-FPGA deployment pipeline: Q8.8 parameter export, .mem generation, and UART spike readback</p>

<p align="center">
  <a href="https://github.com/rmems/silicon-bridge/actions/workflows/ci.yml"><img src="https://github.com/rmems/silicon-bridge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/silicon-bridge"><img src="https://img.shields.io/crates/v/silicon-bridge" alt="crates.io"></a>
  <a href="https://docs.rs/silicon-bridge"><img src="https://docs.rs/silicon-bridge/badge.svg" alt="docs.rs"></a>
  <img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue" alt="MIT/Apache-2.0">
</p>

---

The Rust-side bridge between trained SNN parameters and FPGA hardware. Exports
weights and thresholds as Q8.8 fixed-point `.mem` files for Vivado/Quartus
`$readmemh`, and provides an optional **synchronous** UART bridge — built on the
blocking `serialport` crate and gated behind the `uart` feature — for sending
stimuli and reading back spike states at runtime.

## Features

- **Export traits** for hardware alignment with [silicon-hdl](https://github.com/rmems/silicon-hdl):
  - `FixedPointEncode` — `f32` → signed Q8.8 (`i16`)
  - `ParameterExport` — build the FPGA parameter bundle (infallible, legacy)
  - `CheckedParameterExport` — same bundle, or a typed `ParameterShapeError`
  - `MemFileWriter` — write `$readmemh` `.mem` files (validates before writing)
- `FpgaParameterExporter` — default implementation of those traits
- `format_q88_hex` / `encode_q88_signed_full` / `q88_signed_to_f32` — signed
  parameter Q8.8 helpers (full `i16` range). `encode_q88_signed` remains the
  UART helper with the narrower ±127.99 clamp. `encode_q88_unsigned` is an
  explicit in-memory unsigned mode.
- `FpgaBridge` — blocking UART host protocol for host–FPGA spike exchange,
  backed by `serialport` (`uart` feature); no async runtime is involved.
  `FpgaBridge::new()` enumerates ports with `serialport::available_ports`
  and probes any that look like a USB-serial FPGA bridge — `ttyUSB`/`ttyACM`
  on Linux, `cu.usb*`/`tty.usb*` on macOS, `COM*` on Windows — falling back
  to the historical `/dev/ttyUSB0..2` probe list only when enumeration
  returns nothing (`libudev` unavailable, for instance). It does not verify
  the peer beyond accepting whichever candidate opens; see
  `FpgaBridge::new`'s doc comment for why
- `FpgaMetrics` — Vivado report parser for CI/CD gating: **WNS** and **TNS**
  from timing summary reports, **LUT utilization** from `report_utilization`
  reports (missing TNS or LUT values degrade to `0.0`)

## Installation

```toml
silicon-bridge = "0.1"
```

## Quick Start

### Export Parameters

```rust
use silicon_bridge::{CheckedParameterExport, FpgaParameterExporter};

let mut exporter = FpgaParameterExporter::new();
exporter.set_thresholds(vec![0.6; 16]);
exporter.set_weights(vec![vec![0.5; 16]; 16]);
exporter.set_decay_rates(vec![0.9; 16]);

let params = exporter.try_export().expect("rectangular, finite, in-range");
// → params.thresholds, .weights, .decay_rates are Vec<i16> (signed Q8.8)
// → negative (Dale-inhibitory) weights survive: -1.0 → -256 → `FF00`
// → ready for silicon-hdl WeightRam / NeuronParamRam via Vivado $readmemh

// ParameterExport::export is the documented legacy wrapper: it still
// flattens a ragged matrix and saturates out-of-range values. Prefer try_export
// (or MemFileWriter::write_mem_files) for any image that will be synthesized.
let _legacy = silicon_bridge::ParameterExport::export(&exporter);
```

### UART Spike Readback (requires the `uart` feature)

```toml
silicon-bridge = { version = "0.1", features = ["uart"] }
```

```rust
use silicon_bridge::FpgaBridge;

let mut bridge = FpgaBridge::new()?;
let stimuli = vec![0.1; 16];
let (_potentials, spikes) = bridge.process_stimuli(&stimuli)?;
```

`FpgaBridge` is synchronous. `FpgaBridge::new()` discovers USB-serial ports
across Linux, macOS, and Windows and opens the first FPGA-looking one that
accepts 115200 baud (falling back to the historical `/dev/ttyUSB0..2` probe
list when port enumeration itself comes back empty). A port that opens is
assumed to be the board — construction does not verify the peer.

`process_stimuli` writes the request frame and then `read_exact`s the reply.
The 100 ms value is the `serialport` **per-read** timeout, not a hard
wall-clock budget for the entire call: a partial reply can retry, so the
call can exceed 100 ms. Nothing in this API returns a `Future`, and no async
executor is required.

## Q8.8 Fixed-Point Format

Q8.8 always means “value × 256 packed into a 16-bit word”, truncated toward
zero. This crate keeps **three** interpretations of that word. Signedness is
selected per parameter block (`set_encoding`) and recorded on
`FpgaMetadata::encodings` — never inferred from a filename or from storing
the word as `i16`.

| Aspect | Signed parameter (`.mem`) | Unsigned parameter | Legacy UART clamp |
|---|---|---|---|
| Encode with | `encode_q88_signed_full` / `FixedPointEncode::encode_q88` | `encode_q88_unsigned` | `encode_q88_signed` |
| Decode with | `q88_signed_to_f32` | `q88_to_f32` | `q88_signed_to_f32` |
| Range | `[-128, 127.99609375]` | `[0, 255.99609375]` | `[-127.99, 127.99]` |
| Raw output | `8000`..=`7FFF` | `0000`..=`FFFF` | `8003`..=`7FFD` |
| Serialized as | ASCII hex (`{:04X}` of the 16-bit pattern) | same hex of the unsigned pattern | raw binary, big-endian |
| Use it for | hidden + readout weights; hardware `.mem` | thresholds/decay when explicitly unsigned | host stimuli, RX membrane potentials |

Golden signed-parameter mappings: `-1 → FF00`, `-0.5 → FF80`, `0 → 0000`,
`0.5 → 0080`, `1 → 0100`, `-128 → 8000`, `127.99609375 → 7FFF`. Unsigned
maximum is `255.99609375 → FFFF`; that same hex is signed `-1/256`.

**Why signed for `.mem`.** silicon-hdl reads every image as signed —
`LifNeuron` / `LifNeuronArray` and `OutputLayer` all `$signed`-compare at
runtime, so a Dale-inhibitory weight subtracts from the membrane instead of
adding a large positive (see silicon-hdl `spikenaut-core-sv/mem/README.md`,
“Signedness contract (GH#73)”). `write_mem_files` therefore refuses
`Q88Encoding::Unsigned`. Thresholds and decay may still be selected unsigned
for existing in-memory consumers.

**Do not reuse the UART helper for parameters.** `encode_q88_signed` clamps
at ±127.99 (`-128.0 → 8003`). That bound is mirrored by silicon-hdl's
`scripts/q88.py` for the stimulus path; parameter export uses the full
`i16` range instead.

Exported `.mem` files are directly loadable by silicon-hdl `WeightRam.sv` and
`NeuronParamRam.sv`
([rmems/silicon-hdl](https://github.com/rmems/silicon-hdl)).

## Vivado Timing Metrics

`FpgaMetrics` parses **WNS** (worst negative slack) and **TNS** (total
negative slack) from a Vivado timing summary report, and **LUT utilization**
from `report_utilization` output, so CI can gate on timing and resource usage.
`parse_from_report` / `load_from_path` require a parsable WNS; TNS and LUT
utilization degrade to `0.0` when those columns or reports are absent.

```rust
use silicon_bridge::FpgaMetrics;

// Fail closed: a missing or unparseable report must not let the gate pass.
let metrics = FpgaMetrics::load_from_path("Basys3_Top_timing_summary_routed.rpt")
    .expect("timing report missing or unrecognized");
assert!(
    metrics.wns_ns.is_finite() && metrics.wns_ns >= 0.0,
    "timing violation or non-finite WNS: {} ns",
    metrics.wns_ns
);
```

WNS is required. Optional fields (a gate must treat `0.0` as "not reported",
not as "clean"):

- **TNS** — `tns_ns` is filled from the `TNS(ns)` column of the same data
  row as WNS, or `0.0` when that column is absent
- **LUT utilization** — `lut_utilization` is read from a
  `report_utilization` table (`load_from_reports` / a concatenated report),
  or left at `0.0`

See also [docs/boundary-matrix.md](docs/boundary-matrix.md).

## Repo boundaries

See [docs/boundary-matrix.md](docs/boundary-matrix.md) for what this crate owns
versus `neuromod`, `brainstem-daemon`, `limbic-critic`, and `silicon-hdl`.

## Extracted from Production

Extracted from [Eagle-Lander](https://github.com/rmems/Eagle-Lander), a private
neuromorphic GPU supervisor. The FPGA export pipeline was decoupled from the private
training orchestrator so it works with any SNN framework.

## Related Ecosystem

| Library | Purpose |
|---------|---------|
| [silicon-hdl](https://github.com/rmems/silicon-hdl) | SystemVerilog core, bridge, and SoC for Basys3 / Artix-7 |
| [SynapticDistill.jl](https://github.com/rmems/SynapticDistill.jl) | Julia training + distillation (Q8.8 export path) |
| [neuromod](https://github.com/Limen-Neural/neuromod) | SNN dynamics / core runtime traits (still hosted under Limen-Neural) |

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.

## Ownership and wiki

Live source, issues, and Actions live under
[`rmems/silicon-bridge`](https://github.com/rmems/silicon-bridge) after return
from Limen-Neural. The GitHub wiki is **enabled** at
[`rmems/silicon-bridge/wiki`](https://github.com/rmems/silicon-bridge/wiki).
Prefer in-repo `README.md` and `docs/` for durable documentation; treat the wiki
as optional narrative. Some generated wiki pages still mention the pre-transfer
org — this tree is the source of truth.

## CI

GitHub Actions (`.github/workflows/ci.yml`) runs three job groups on every push
to `main` and every pull request. No secrets are required.

| Job | Runner | What it runs |
|-----|--------|--------------|
| `fmt (ubuntu-latest)` | Linux | `cargo fmt --check` (once — formatting is OS-independent) |
| `test (ubuntu-latest)` | Linux | `cargo clippy --all-targets -- -D warnings`, `cargo build`, `cargo test` |
| `test (macos-latest)` | macOS | same as above |
| `test (windows-latest)` | Windows | same as above |
| `uart (ubuntu-latest)` | Linux | installs `libudev-dev`, then `cargo check --features uart`, `cargo test --features uart`, and `cargo doc --no-deps --features uart` with `RUSTDOCFLAGS: -D warnings` |

The `test` matrix uses default features and has `fail-fast: false`, so one OS
failing does not cancel the others. The `uart` job is Linux-only because
`serialport` needs `libudev` there; it runs unit tests only — no serial
hardware is attached to CI runners.
