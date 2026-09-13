<p align="center">
  <img src="docs/logo.png" width="220" alt="Spikenaut">
</p>

<h1 align="center">silicon-bridge</h1>
<p align="center">SNN-to-FPGA deployment pipeline: Q8.8 parameter export, .mem generation, and UART spike readback</p>

<p align="center">
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
  - `FixedPointEncode` — `f32` → Q8.8 (`u16`)
  - `ParameterExport` — build the FPGA parameter bundle
  - `MemFileWriter` — write `$readmemh` `.mem` files
- `FpgaParameterExporter` — default implementation of those traits
- `format_q88_hex` / `q88_to_f32` — Q8.8 helpers
- `FpgaBridge` — blocking UART host protocol for host–FPGA spike exchange,
  backed by `serialport` (`uart` feature); no async runtime is involved.
  `FpgaBridge::new()` probes only `/dev/ttyUSB0`, `/dev/ttyUSB1`, and
  `/dev/ttyUSB2` on Linux — not ttyACM, Windows COM ports, or arbitrary
  USB paths
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
use silicon_bridge::{FpgaParameterExporter, ParameterExport};

let mut exporter = FpgaParameterExporter::new();
exporter.set_thresholds(vec![0.6; 16]);
exporter.set_weights(vec![vec![0.5; 16]; 16]);
exporter.set_decay_rates(vec![0.9; 16]);

let params = ParameterExport::export(&exporter);
// → params.thresholds, .weights, .decay_rates are Vec<u16> (Q8.8 format)
// → ready for silicon-hdl WeightRam / NeuronParamRam via Vivado $readmemh
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

`FpgaBridge` is synchronous. `FpgaBridge::new()` opens the first of
`/dev/ttyUSB0`, `/dev/ttyUSB1`, `/dev/ttyUSB2` that accepts 115200 baud; it
does not probe ttyACM devices, Windows COM ports, or arbitrary USB paths.

`process_stimuli` writes the request frame and then `read_exact`s the reply.
The 100 ms value is the `serialport` **per-read** timeout, not a hard
wall-clock budget for the entire call: a partial reply can retry, so the
call can exceed 100 ms. Nothing in this API returns a `Future`, and no async
executor is required.

## Q8.8 Fixed-Point Format

Q8.8 always means “value × 256 packed into a 16-bit word”, but this crate
carries **two signedness conventions** that are not interchangeable:
parameters baked into the bitstream are unsigned; host stimuli sent over UART
are signed.

| Aspect | Parameter export (`.mem`) | Host stimuli (UART TX/RX) |
|---|---|---|
| Encode with | `FixedPointEncode::encode_q88` / `encode_q88_unsigned` | `encode_q88_signed` |
| Decode with | `q88_to_f32` | `q88_signed_to_f32` |
| Raw type | `u16` (unsigned) | `i16` (two's complement) |
| Width | 16 bits — 8 integer + 8 fractional | 16 bits — 8 integer + 8 fractional |
| Scaling | `raw = value × 256`, truncated toward zero | `raw = value × 256`, truncated toward zero |
| Encoder input clamp | `[0.0, 255.99609375]` (scaled clamp `0..=65535`) | `[-127.99, 127.99]` |
| Encoder raw output | `0..=65535` | `-32765..=32765` (saturates inside the `i16` limits) |
| Decoder accepts | any `u16`: `0..=65535` → `0.0..=255.99609375` | any `i16`: `-32768..=32767` → `-128.0..=127.99609375` |
| Serialized as | ASCII hex, one `{:04X}` word per line (`$readmemh`) | raw binary, big-endian (MSB first) |
| Use it for | weights, thresholds, decay rates | host stimuli, RX membrane potentials |

The export path **cannot represent negative values** — anything below `0.0`
clamps to raw `0`. Both encoders truncate toward zero and map `NaN` to raw `0`.
The signed decoder is wider than its encoder: the FPGA may send any `i16`, so
`0x8000` decodes to `-128.0` even though TX saturates at `±32765`.

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
| [neuromod](https://github.com/Limen-Neural/neuromod) | SNN dynamics / core runtime traits |

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
