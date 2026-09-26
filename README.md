<p align="center">
  <img src="docs/logo.png" width="220" alt="silicon-bridge">
</p>

<h1 align="center">silicon-bridge</h1>
<p align="center">SNN-to-FPGA deployment pipeline: Q8.8 parameter export, .mem generation, and UART spike readback</p>

<p align="center">
  <a href="https://github.com/rmems/silicon-bridge/actions/workflows/ci.yml"><img src="https://github.com/rmems/silicon-bridge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://app.codecov.io/gh/rmems/silicon-bridge"><img src="https://codecov.io/gh/rmems/silicon-bridge/branch/main/graph/badge.svg" alt="Codecov coverage"></a>
  <a href="https://qlty.sh/gh/rmems/projects/silicon-bridge"><img src="https://qlty.sh/gh/rmems/projects/silicon-bridge/coverage.svg" alt="Qlty coverage"></a>
  <a href="https://github.com/rmems/silicon-bridge/actions/workflows/quality.yml"><img src="https://github.com/rmems/silicon-bridge/actions/workflows/quality.yml/badge.svg" alt="Quality services"></a>
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

- **Export traits** for hardware consumers, with a pinned [silicon-hdl](https://github.com/rmems/silicon-hdl) reference contract:
  - `FixedPointEncode` — `f32` → signed Q8.8 (`i16`)
  - `ParameterExport` — build the FPGA parameter bundle (infallible, **legacy**; prefer `try_export`)
  - `CheckedParameterExport` — same bundle, or a typed `ParameterShapeError`
  - `ExportConfig` / `write_generic` — default **generic-dense-q88** path
  - `MemFileWriter` — explicit Spikenaut-v2 compatibility writer (prefer `write_generic`)
  - `write_with_config` — generic, custom, required-readout, reference, and legacy contracts
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

## Quality reporting

The `Quality` workflow generates one Rust LCOV report and publishes it to
Codecov and Qlty on pushes to `main` and same-repository pull requests. Fork
pull requests still generate the report; Codecov uses its public-repository
tokenless path there, while Qlty skips its upload because fork workflows cannot
use this repository's OIDC authorization. Trusted events use GitHub Actions OIDC,
so Codecov and Qlty do not require long-lived coverage tokens.
Codacy analysis and coverage are also wired in; add a repository secret named
`CODACY_PROJECT_TOKEN` to enable those two uploads. The existing [`.codacy.yml`](.codacy.yml)
keeps repository-specific analyzer exclusions in version control.

## Reference hardware

The default public path is **board-agnostic** Q8.8 `.mem` export: any
`$readmemh` consumer can load the files. The **named reference companion
path** is the Digilent Basys 3 board plus
[`rmems/silicon-hdl`](https://github.com/rmems/silicon-hdl)
(`spikenaut_soc_basys3_top`). That is one named reference, not a
multi-board claim.

| Field | Value |
|---|---|
| Board | **Digilent Basys 3** |
| FPGA | **Xilinx Artix-7** `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`) |
| Companion RTL | [`rmems/silicon-hdl`](https://github.com/rmems/silicon-hdl) — `spikenaut_soc_basys3_top` (`Basys3_Top.sv`) |
| Constraints | `constraints/basys3.xdc`, `basys3_soc.xdc` |
| Host UART | SiliconBridge **v3.0** (16 in / 16 out) — example layout with [golden bytes](https://github.com/rmems/silicon-bridge/tree/main/tests/golden/uart) matching Basys 3 firmware, not “any board” |
| Prior board smoke | silicon-hdl [#68](https://github.com/rmems/silicon-hdl/issues/68) / [`docs/phase-c-board-smoke.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/phase-c-board-smoke.md) — LED heartbeat **PASS** only; **not** a silicon-bridge UART host session |

silicon-hdl also ships `constraints/artix7_trainer.xdc` for a generic
Artix-7 trainer pinout. That file is **not** the claimed reference demo;
`scripts/build_soc.tcl` reads `basys3.xdc` + `basys3_soc.xdc` only.

A silicon-bridge UART host session on this board is release evidence for the
0.3.1 publish path ([#84](https://github.com/rmems/silicon-bridge/issues/84)).
When run, that session is recorded in
[`docs/hardware-smoke-note.md`](docs/hardware-smoke-note.md); until that note is
filled in, no such host-session proof exists. Raw logs may also be linked on
#84. This crate’s examples and CI do not flash an FPGA. Lab JTAG serials belong
in smoke logs, not here.

Companion contracts live in silicon-hdl:
[README](https://github.com/rmems/silicon-hdl#readme),
[`docs/interface-alignment.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/interface-alignment.md),
[`docs/host-soc-e2e.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/host-soc-e2e.md).

## Installation

`silicon-bridge` is **not published on crates.io**. A version badge or an
in-tree version string is not a live registry crate. `[package].version`
is `0.3.1`; an authorized `cargo publish` is a separate maintainer action.
Until then, depend on git or a path:

```toml
silicon-bridge = { git = "https://github.com/rmems/silicon-bridge" }
# silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", rev = "<commit>" }
# silicon-bridge = { path = "../silicon-bridge" }
```

Optional UART I/O:

```toml
silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", features = ["uart"] }
```

Optional NIR interop dependency, for downstream crates that want to access
the upstream NIR type surface re-exported as `silicon_bridge::nir` without
adding an HDF5 reader here:

```toml
silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", features = ["nir"] }
```

The default feature set does not depend on `nir-rs`. The `nir` feature pins
`nir-rs = 0.4.3`, whose own crate metadata currently raises the effective
feature-specific toolchain floor above this crate's default `rust-version`.

`[package].rust-version` is **`1.88.0`**: the **MSRV** (minimum supported
Rust version) — the language floor this crate needs (edition 2024 plus
`if`/`let` chains). The dependency graph would compile on **1.85.0** without
those language features. CI runs a dedicated **1.88.0** job on every push
and PR, and separately exercises the moving GitHub Actions **`stable`**
toolchain (clippy, multi-OS tests, full `--all-features` including `nir`, fuzz
compile). The optional `nir` feature is not part of the 1.88.0 MSRV job because
`nir-rs` requires a newer compiler. Raising MSRV requires
an explicit compatibility decision and a CHANGELOG entry; see
[docs/release-readiness.md](docs/release-readiness.md).

### After an authorized crates.io publish

Only once `https://crates.io/crates/silicon-bridge` serves a version
(current candidate: **0.3.1**):

```toml
silicon-bridge = "0.3.1"
```

Verify docs.rs **after** that publish. A `cargo package` / path smoke test
is not a registry test.

## Quick Start

Runnable copies live in [`examples/`](examples/README.md). They compile
against the public crate API and run offline (no FPGA, no serial device),
except [`uart_host_smoke`](examples/uart_host_smoke.rs), which opens an
explicit serial port when built with `--features uart` (maintainer hardware
evidence only; see [release-readiness §5](docs/release-readiness.md)).

### Bring your own floats (generic-dense-q88)

You already have trained `f32` thresholds, hidden weights, decay rates, and
optionally a `K×N` readout. This crate does not train a network. Load the
floats, run the checked exporter, write `$readmemh` `.mem` files:

```rust
use silicon_bridge::FpgaParameterExporter;

let exporter = FpgaParameterExporter::from_params(
    vec![1.0, 0.5, 1.5, 0.75],           // thresholds (N)
    vec![
        vec![0.5, -1.0, 0.25, 1.0, -0.5, 0.0],
        vec![-128.0, 127.99609375, 1.0 / 256.0, -1.0 / 256.0, 2.0, -2.0],
        vec![1.0; 6],
        vec![-0.5, 0.5, -0.5, 0.5, -0.5, 0.5],
    ],                                   // hidden weights (N×M, row-major)
    vec![0.5, 0.75, 0.25, 1.0],          // decay (N)
);
// exporter.set_output_weights(vec![vec![1.0, 0.0, 0.0, 0.0]]); // optional K×N readout

let params = exporter.try_export().expect("rectangular, finite, in-range");
// → params.thresholds, .weights, .decay_rates are Vec<i16> (signed Q8.8)
// → negative (Dale-inhibitory) weights survive: -1.0 → -256 → `FF00`

// Default public disk path: no Spikenaut tag, no invented timing, no stdout.
let report = exporter
    .write_generic("fpga_output")
    .expect("generic-dense-q88 export");
assert_eq!(report.written[0], "parameters.mem");

// ParameterExport::export is the documented legacy wrapper: it still
// flattens a ragged matrix, saturates out-of-range values, and stamps
// Spikenaut-v2. Prefer try_export for any image that will be synthesized.
// write_mem_files is the explicit Spikenaut-v2 compatibility path.
let _legacy = silicon_bridge::ParameterExport::export(&exporter);
```

That writes `parameters.mem` (thresholds), `parameters_weights.mem` (`N×M`),
`parameters_decay.mem`, optional `parameters_output_weights.mem` (`K×N`), and
`parameters.json`. Existing files are refused unless you call
`ExportConfig::generic().allow_replace()` (or `write_with_config` with that
config). Full HDL reader contract: [docs/consumer.md](docs/consumer.md).

### Export profiles

Dense `.mem` export is profiled. See
[docs/export-profiles.md](docs/export-profiles.md) for the migration note.

| Profile | API | Use when |
|---|---|---|
| **Generic** `generic-dense-q88` (**default**) | `write_generic` / `ExportConfig::generic()` / `ExportConfig::default()` | New callers. No Spikenaut metadata, no timestamp unless you supply one, no `target_latency_us` unless you declare a target (never a measurement). Refuses to overwrite files unless you call `allow_replace()`. |
| **Custom contract** | `ExportContract::custom(...)` + `ExportConfig::from_contract(...)` | Bring your own profile id, schema id, filenames, and readout rule. This is the path for another HDL package or board-specific firmware that consumes the same dense Q8.8 files without inheriting Spikenaut or silicon-hdl metadata. |
| **Required readout** | `ExportConfig::generic_with_required_readout()` | Dense bundle that **requires** a signed `K×N` readout (or rejects). On-disk profile id remains `spikenaut-signed-output-v1` (not a redefinition of `Spikenaut-v2`). `ExportConfig::spikenaut_signed_output_v1()` is the Spikenaut-named alias. |
| **silicon-hdl v3** | `ExportConfig::silicon_hdl_v3()` | Pinned reference profile for 16 input channels, 16 hidden neurons, and 3 output classes. Preserves `parameters_output_weights.mem` as generic `K×N`, also writes `hdl_readout_neuron_major.mem` as HDL-native `N×K`, and records a generic upstream `reference` for the pinned silicon-hdl revision. See [docs/host-hdl-contract.md](docs/host-hdl-contract.md). |
| **Legacy Spikenaut-v2** | `MemFileWriter::write_mem_files` / `ExportConfig::legacy_spikenaut_v2()` | Reproduce historical `version: "Spikenaut-v2"` files. Replaces existing files. Records a declared 35 µs target, not a measured latency. Names alone do not imply HDL compatibility. |

The checked writer returns an `ExportReport` and does not print. Same
input, config, and metadata produce byte-identical `.mem` and JSON.
Only dense **row-major** flattening is implemented; other layouts are
rejected. ASCII hex word order is not UART byte order.

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
(115200 baud, 100 ms) match SiliconBridge **v3.0**, the 16-channel example
layout whose [golden bytes](https://github.com/rmems/silicon-bridge/tree/main/tests/golden/uart) match current Basys 3
firmware — not a claim that any board speaks this frame. See
[Reference hardware](#reference-hardware). `FpgaBridge::new()` still exists
as a legacy convenience that probes USB-serial-looking names (and
`/dev/ttyUSB0..2` when enumeration is empty or fails); it is not the
recommended public path. A port that opens is transport-open only —
construction does not send stimulus frames or verify the peer. The host API
will open any caller-selected serial device; matching firmware is a
separate concern.

`list_serial_ports()` reports OS enumerator failures instead of swallowing
them. `find_fpga_ports()` is a name heuristic on that list, not FPGA
authentication. On Linux, enumeration typically requires `libudev`; macOS
and Windows do not. Default-feature builds do not link `serialport`.

Request/response bytes are encoded by `DenseQ88Layout` / `encode_stimuli` /
`decode_response` (no `serialport` dependency). SiliconBridge v3.0 is the
**16-channel example layout** with committed golden bytes for Basys 3
firmware. Other dense sizes are host codecs only — they need matching FPGA
firmware; changing the host layout is not enough.
The checked path requires exactly `input_channels` finite stimuli.
`process_stimuli` remains the legacy pad/truncate wrapper.

The pure [`UART v4 codec contract`](docs/uart-v4-protocol.md) adds sync,
version/length, a request id, and CRC-16 to requests and replies. It is
host-only and opt-in: it is **not compatible** with current Basys 3
SiliconBridge v3.0 firmware, and `FpgaBridge` does not select it. Hardware use
requires a separately pinned matching `silicon-hdl` implementation.

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
| Optional synchronous UART | `uart` + native `serialport` | compile + unit tests on Linux CI (#24) | **explicit device only**; live I/O only in `uart_host_smoke` (`--features uart`) |

Evidence classes: **OS compilation** (CI #24: Linux/macOS/Windows; Linux `uart`
job with `libudev-dev`), **HDL simulation** (`tests/golden/hdl/run.sh`, Icarus,
optional), **board testing** (not this crate's default examples or CI). The
only captured physical Basys 3 evidence so far is silicon-hdl’s LED heartbeat
smoke, not a silicon-bridge UART host session (that is [#84](https://github.com/rmems/silicon-bridge/issues/84)).

Serial prerequisites: Linux needs `libudev` to **enumerate** ports; opening a
named path does not. Always select the device yourself. `ping()` sends a real
16-channel stimulus and is not passive discovery. Custom
`DenseQ88Layout::dense` sizes need matching firmware.

Compatibility **example layouts** with #53 golden-byte evidence: SiliconBridge
**v3.0** (16/16, matching Basys 3 firmware) and host-only dense 8 / 32 / 8×10
frames. Changing host channel counts does not reconfigure an FPGA. Do not
list other revisions without fixtures.

## Migration

- **Unsigned `.mem`:** re-export signed. `FFFF` is unsigned `255.996` and
  signed `-1/256`.
- **Signed parameters / readout:** defaults are signed; read
  `metadata.encodings`. The default public path is generic-dense-q88
  (`write_generic`). Prefer `ExportConfig::generic_with_required_readout()`
  when a signed `K×N` readout is required. The `Spikenaut-v2` string is a
  layout id used only by the explicit legacy profile (`write_mem_files` /
  `ExportConfig::legacy_spikenaut_v2()`).
  Prefer `try_export` / `write_generic` over `ParameterExport::export`.
- **UART clamp:** keep `encode_q88_signed` on the wire; do not use it for
  `.mem`.
- **Timestamps / printing:** `set_timestamp` requires RFC 3339 UTC. The
  checked path omits timestamps by default. `write_mem_files` still records
  a wall-clock stamp (legacy).
- **Pre-1.0:** no stability promise. New public fields may appear; use
  `..Default::default()` on struct literals.

## Public-release readiness

See [docs/release-readiness.md](docs/release-readiness.md). This tree is the
**0.3.1** package candidate (`[package].version` is `0.3.1`). An authorized
human `cargo publish` is still required before crates.io / docs.rs are live.
Do not treat the README badge or future crates.io install snippets as a live
registry crate.
Packaged-crate smoke (`bash scripts/smoke-packaged-consumer.sh`) is not
registry proof. CI matrix remains
[#24](https://github.com/rmems/silicon-bridge/issues/24).

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
neuromorphic GPU supervisor. The FPGA export pipeline was decoupled from that
training orchestrator so any SNN framework can supply `f32` thresholds,
weights, and decay. The default public path is board-agnostic Q8.8 `.mem`.
The named reference companion board is Digilent Basys 3 (see
[Reference hardware](#reference-hardware)); Spikenaut export layouts remain
**opt-in profiles**.

## Related Ecosystem

| Library | Purpose |
|---------|---------|
| [silicon-hdl](https://github.com/rmems/silicon-hdl) | Reference RTL: Digilent Basys 3 / Artix-7 XC7A35T SoC (`spikenaut_soc_basys3_top`) |
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

GitHub Actions (`.github/workflows/ci.yml`) runs four job groups on every push
to `main` and every pull request. No secrets are required.

| Job | Runner | What it runs |
|-----|--------|--------------|
| `fmt (ubuntu-latest)` | Linux | `cargo fmt --check` (once — formatting is OS-independent) |
| `test (ubuntu-latest)` | Linux | `cargo clippy --all-targets -- -D warnings`, `cargo build`, `cargo test` |
| `test (macos-latest)` | macOS | same as above |
| `test (windows-latest)` | Windows | same as above |
| `uart (ubuntu-latest)` | Linux | installs `libudev-dev`, then `cargo clippy --all-targets --all-features -- -D warnings`, `cargo check --features uart`, `cargo test --all-features`, and `cargo doc --no-deps --features uart` with `RUSTDOCFLAGS: -D warnings` |
| `fuzz-build (ubuntu-latest)` | Linux | nightly + `cargo fuzz build --dev --sanitizer none` (compile only; no campaign, no serial I/O) |

The `test` matrix uses default features and has `fail-fast: false`, so one OS
failing does not cancel the others. The `uart` job is Linux-only because
`serialport` needs `libudev` there; it runs unit tests only — no serial
hardware is attached to CI runners. Fuzz campaigns stay opt-in; see
`fuzz/README.md`.
