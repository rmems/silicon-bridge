<p align="center">
  <img src="docs/logo.png" width="220" alt="Spikenaut">
</p>

<h1 align="center">silicon-bridge</h1>
<p align="center">SNN-to-FPGA deployment pipeline: Q8.8 parameter export, .mem generation, and UART spike readback</p>

<p align="center">
  <a href="https://github.com/rmems/silicon-bridge/actions/workflows/ci.yml"><img src="https://github.com/rmems/silicon-bridge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/crates.io-not%20published-lightgrey" alt="crates.io not published">
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
  Prefer `FpgaBridge::open_with_config` / `FpgaBridge::builder` with an
  explicit port, baud rate, and finite nonzero per-I/O timeout. Platform
  paths (`ttyUSB`/`ttyACM`, macOS `cu.*`, Windows `COM*`, custom
  `/dev/serial/by-id/...`) are accepted as-is; the explicit API does not
  filter them. `FpgaBridge::new()` is a **legacy** probe helper: it
  enumerates USB-serial-looking names and falls back to `/dev/ttyUSB0..2`
  when enumeration is empty or fails. Opening a port is transport-open
  only — not FPGA identity. `list_serial_ports()` returns enumerator
  errors instead of an empty list. On Linux, enumeration typically needs
  `libudev`; opening a named path does not.
- `FpgaMetrics` — Vivado report parser for CI/CD gating: **WNS** and **TNS**
  from timing summary reports, **LUT utilization** from `report_utilization`
  reports (missing TNS or LUT values degrade to `0.0`)

## Installation

`silicon-bridge` is **not published on crates.io** as of 2026-09-15
(registry lookup `GET https://crates.io/api/v1/crates/silicon-bridge` returned
404; docs.rs likewise). The former crates.io / docs.rs badges were not proof
of publication. `[package].version` in this tree is `0.1.0` and
`rust-version` is `1.98.1`. A real release is a [separately authorized
publish](docs/release-readiness.md).

### Unpublished / development (current)

```toml
# git (pin a rev for reproducible builds)
silicon-bridge = { git = "https://github.com/rmems/silicon-bridge" }
# silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", rev = "<commit>" }

# path, while hacking on a checkout — not a sibling-workspace requirement
# silicon-bridge = { path = "../silicon-bridge" }
```

### After an authorized crates.io publish

Only once `https://crates.io/crates/silicon-bridge` serves a version:

```toml
silicon-bridge = "x.y.z"  # the version that was actually published
```

Verify docs.rs **after** that publish. A `cargo package` / path smoke test is
not a registry test.

Optional UART I/O:

```toml
silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", features = ["uart"] }
```

## Quick Start

Runnable copies live in [`examples/`](examples/README.md). They compile
against the public crate API and run offline (no FPGA, no serial device).

### Generic checked export (4 neurons × 6 inputs)

```rust
use silicon_bridge::{CheckedParameterExport, FpgaParameterExporter, format_q88_hex};

let mut exporter = FpgaParameterExporter::from_params(
    vec![1.0, 0.5, 1.5, 0.75],
    vec![
        vec![0.5, -1.0, 0.25, 1.0, -0.5, 0.0],
        vec![-128.0, 127.99609375, 1.0 / 256.0, -1.0 / 256.0, 2.0, -2.0],
        vec![1.0; 6],
        vec![-0.5, 0.5, -0.5, 0.5, -0.5, 0.5],
    ],
    vec![0.5, 0.75, 0.25, 1.0],
);
exporter.set_format_version("generic-dense-q88");
exporter.set_timestamp("1970-01-01T00:00:00Z");

let params = CheckedParameterExport::try_export(&exporter).expect("rectangular, finite, in-range");
assert_eq!(params.metadata.version, "generic-dense-q88");
assert_eq!(format_q88_hex(-1.0), "FF00"); // negative weights survive
assert_eq!(params.output_weights, None);   // no readout, no Spikenaut identity
// MemFileWriter::write_mem_files(&exporter, "out").unwrap();
```

`ParameterExport::export` is the documented legacy wrapper: it still flattens
a ragged matrix and saturates out-of-range values. Prefer `try_export` (or
`MemFileWriter::write_mem_files`) for any image that will be synthesized.

A separate synthetic 16-neuron + signed-readout example is
`cargo run --example spikenaut_profile_export`. It names the layout tag
`Spikenaut-v2` (`EXPORT_FORMAT_VERSION`) and does **not** claim trained
weights or validated old artifacts. Golden files: `tests/golden/spikenaut_16/`
(#53).

### UART Spike Readback (requires the `uart` feature)

Not part of ordinary example or test runs. Codec encode/decode does **not**
need this feature (`cargo run --example dense_codec`).

```rust
use std::time::Duration;
use silicon_bridge::{FpgaBridge, SerialConfig};

let config = SerialConfig::new(115_200, Duration::from_millis(100))?;
let mut bridge = FpgaBridge::open_with_config("/dev/ttyUSB0", config)?;
// Windows: "COM4". macOS: "/dev/cu.usbserial-210319B".
let stimuli = vec![0.1; 16];
let (_potentials, spikes) = bridge.process_stimuli(&stimuli)?;
```

`FpgaBridge` is synchronous. The recommended path is an explicit port plus
`SerialConfig` (baud rate and a finite nonzero per-I/O timeout). Defaults
match SiliconBridge v3.0 firmware: 115200 baud, 100 ms. `FpgaBridge::new()`
still exists as a legacy convenience that probes USB-serial-looking names
(and `/dev/ttyUSB0..2` when enumeration is empty or fails); it is not the
recommended public path. A port that opens is transport-open only —
construction does not send stimulus frames or verify the peer.

`list_serial_ports()` reports OS enumerator failures instead of swallowing
them. `find_fpga_ports()` is a name heuristic on that list, not FPGA
authentication. On Linux, enumeration typically requires `libudev`; macOS
and Windows do not. Default-feature builds do not link `serialport`.

Request/response bytes are encoded by `DenseQ88Layout` / `encode_stimuli` /
`decode_response` (no `serialport` dependency). SiliconBridge v3.0 is the
16-channel profile that matches current firmware. Other dense sizes are
host codecs only — they need matching FPGA firmware; changing the host
layout is not enough. The checked path requires exactly `input_channels`
finite stimuli. `process_stimuli` remains the legacy pad/truncate wrapper.

`process_stimuli` writes the request frame and then `read_exact`s the reply.
The configured timeout is the `serialport` **per-I/O** timeout, not a hard
wall-clock budget for the entire call: filling 36 bytes may take several
reads, so the call can exceed the timeout. Extra reads are not a
retransmission of the stimulus. A timeout after the write has already
updated FPGA state; retrying applies the stimulus again. After a failed
exchange the handle requires `recover()` before further stimuli. The
unframed v3 reply cannot detect every stale same-length frame. `ping()`
sends a real 16-channel stimulus of `0.1` — it is not passive discovery.
Nothing in this API returns a `Future`, and no async executor is required.

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

## HDL reader contract

silicon-bridge quantizes and writes files. It does **not** interpret trained
parameter meaning. An external HDL reader must implement:

| Property | Value |
|---|---|
| Width | 16-bit words |
| Fractional bits | 8 (scale 256), truncate toward zero |
| Signedness | Per-block `metadata.encodings`; hardware `.mem` is signed two's complement |
| Flattening | Row-major dense: `addr = neuron * num_channels + input` |
| Filenames | `parameters.mem`, `parameters_weights.mem`, `parameters_decay.mem`, optional `parameters_output_weights.mem`, `parameters.json` |

Readout is `K×N` as this crate writes it. silicon-hdl `OutputLayer` indexes
`N×K`. #53 records both; they are not rewritten to match. Full table:
[docs/consumer.md](docs/consumer.md).

Checked export rejects empty/ragged/mismatched shapes, non-finite values, and
(default) overflow. `RangePolicy::Saturate` is only applied by
`try_export_with_report`. Legacy `ParameterExport::export` still saturates
silently. UART `encode_q88_signed(-128.0)` is `8003`; parameter `.mem` is
`8000`.

## Feature / support matrix

| Path | Features | Ordinary `cargo test` / examples | Board / live UART |
|---|---|---|---|
| Export-only | default | yes | no |
| Pure codec (`encode_stimuli` / `decode_response`) | default | yes (`dense_codec` example) | no |
| Optional synchronous UART | `uart` + native `serialport` | compile + unit tests on Linux CI (#24) | **explicit device only**; not in examples |

Evidence classes: **OS compilation** (CI #24: Linux/macOS/Windows; Linux `uart`
job with `libudev-dev`), **HDL simulation** (`tests/golden/hdl/run.sh`, Icarus,
optional), **board testing** (not this crate's default examples or CI).

Serial prerequisites: Linux needs `libudev` to **enumerate** ports; opening a
named path does not. Always select the device yourself. `ping()` sends a real
16-channel stimulus and is not passive discovery. Custom
`DenseQ88Layout::dense` sizes need matching firmware.

Compatibility profiles with #53 golden evidence: SiliconBridge **v3.0**
(16/16, current firmware) and host-only dense 8 / 32 / 8×10 frames. Do not
list other revisions without fixtures.

## Migration

- **Unsigned `.mem`:** re-export signed. `FFFF` is unsigned `255.996` and
  signed `-1/256`.
- **Signed parameters / readout:** defaults are signed; read
  `metadata.encodings`. The `Spikenaut-v2` string is a layout id — override
  with `set_format_version` for generic bundles.
- **UART clamp:** keep `encode_q88_signed` on the wire; do not use it for
  `.mem`.
- **Timestamps / printing:** `set_timestamp` for deterministic JSON.
  `write_mem_files` still prints a summary.
- **Pre-1.0:** no stability promise. New public fields may appear; use
  `..Default::default()` on struct literals.

## Public-release readiness

See [docs/release-readiness.md](docs/release-readiness.md). Checklist: verify
registry state, choose the real version, test Rust `1.98.1`, review
license/`cargo package --list`, run default and UART checks, deny rustdoc
warnings, `cargo publish --dry-run` on the candidate commit. Verify docs.rs
and a **registry** consumer only after publish. This ticket does not run
`cargo publish`. Packaged-crate smoke: `bash scripts/smoke-packaged-consumer.sh`
(not a crates.io proof). CI matrix remains [#24](https://github.com/rmems/silicon-bridge/issues/24).

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
