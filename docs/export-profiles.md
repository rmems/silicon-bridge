<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Export profiles and migration

Dense Q8.8 export is no longer a single Spikenaut-branded writer. Three
contracts share the same parameter blocks (thresholds, hidden weights, decay,
optional readout) and the same signed Q8.8 encoder. They differ in schema
tag, filenames policy, timestamps, overwrite, and whether a readout is
required.

This is the migration note for GitHub
[#50](https://github.com/rmems/silicon-bridge/issues/50) / Linear RM-1075.

## Profiles

| Profile | Constructor | Schema (`FpgaMetadata::version`) | Readout | Timestamp | `target_latency_us` | Overwrite |
|---|---|---|---|---|---|---|
| Generic | `ExportConfig::generic()` | `silicon-bridge-dense-v1` | optional | omitted unless supplied | omitted unless declared | refuse |
| Spikenaut legacy | `ExportConfig::spikenaut_legacy()` | `Spikenaut-v2` | optional | wall clock unless supplied | 35 µs **declared** target | replace |
| Spikenaut signed-output | `ExportConfig::spikenaut_signed_output()` | `Spikenaut-signed-output-v1` | **required** K×N | omitted unless supplied | omitted unless declared | refuse |

`Spikenaut-v2` is **not** redefined. A corrected signed-readout contract uses
`Spikenaut-signed-output-v1`. Downstream that keys on `Spikenaut-v2` keeps
seeing that tag from `write_mem_files` / `try_export`.

Schema version, producer crate version (`CARGO_PKG_VERSION`), and
profile / model identity are separate JSON fields (`version`,
`producer_crate_version`, `profile`, optional `model_id`).

## What to call

New, framework-agnostic callers:

```rust
use silicon_bridge::{ExportConfig, FpgaParameterExporter};

let mut exporter = FpgaParameterExporter::from_params(
    vec![1.0; 4],
    vec![vec![0.5; 6]; 4],
    vec![0.9; 4],
);
let report = exporter.write_with_config("out", &ExportConfig::generic())?;
// report replaces the old stdout summary; the writer prints nothing.
```

Spikenaut deployment (documented names
`parameters.mem` / `parameters_weights.mem` / `parameters_decay.mem` /
optional `parameters_output_weights.mem` + `parameters.json`):

- `MemFileWriter::write_mem_files` — same names, overwrite allowed, 35 µs
  declared target, leftover readout file removed when absent.
- `ExportConfig::spikenaut_signed_output()` when a signed K×N readout is
  part of the contract. Missing readout is an error
  (`ParameterShapeError::ReadoutRequired`).

In-memory `ParameterExport::export` and `try_export` still produce
`Spikenaut-v2` metadata. Overlay a profile with
`try_export_with_config`.

## Overwrite and filenames

- Generic and signed-output refuse to replace existing destinations unless
  `ExportConfig::allow_overwrite()` is set. Validation happens before any
  truncate.
- Legacy `write_mem_files` still replaces, matching the historical writer.
- `ExportFileLayout::new` accepts only unique, relative basenames. Absolute
  paths, `..` traversal, and directory separators are rejected before write.
  ASCII case collisions are treated as duplicates (Windows-safe).

## Determinism

The same parameters + config (including timestamp, if any) produce
byte-identical `.mem` and JSON. Generic and signed-output omit timestamps by
default. Use `with_timestamp` when a stable stamp is required. Do not treat
`target_latency_us` as a measurement — it is recorded only when the caller
or the legacy Spikenaut profile supplies a declared target.

## `.mem` vs UART

`.mem` files are uppercase four-digit ASCII hex, one 16-bit word per line
(`MemWordFormat::AsciiHexU16`). UART frames are raw binary, big-endian.
There is no endianness switch for text hex. Matrix flattening is row-major
only; `MatrixLayout::parse("column_major")` is an error.

## JSON field changes (pre-1.0)

- `target_latency_us` is now `Option<f32>` and omitted when unset. Legacy
  JSON that still has `"target_latency_us": 35.0` deserializes as `Some(35.0)`.
- Empty timestamps are omitted. Profile-configured writes add `profile`,
  `producer_crate_version`, and `layout` (per-block dimensions, signedness,
  16/8 Q-format, rounding, overflow, flattening, filenames).
- `MemFileWriter::write_mem_files` returns `ExportReport` instead of `()`
  and no longer prints to stdout.
