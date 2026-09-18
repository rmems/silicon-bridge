<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Export profiles — migration note

Issue [#50](https://github.com/rmems/silicon-bridge/issues/50). Dense Q8.8
export is no longer a single Spikenaut-branded writer. Predefined contracts
share the #48 checked validator and the #49 per-block encoding policy, and
callers can build a custom contract for their own HDL package. This is not a
plugin or tensor framework.

ASCII `.mem` files remain one uppercase 16-bit hex word per line. That
layout is independent of UART frame byte order. There is no endianness
knob for text hex.

## Profiles

| Profile | Identity | Schema | Readout | Timestamp | `target_latency_us` | Overwrite |
|---|---|---|---|---|---|---|
| Generic dense Q8.8 | `generic-dense-q88` | `silicon-bridge-dense-q88-v1` | optional | omitted unless supplied | omitted unless declared | refuse unless `allow_replace` |
| Custom contract | caller-supplied | caller-supplied | optional, required `K×N`, or required `K×N` plus HDL-native `N×K` | omitted unless supplied | omitted unless declared | refuse unless `allow_replace` |
| Required readout | `spikenaut-signed-output-v1` | `spikenaut-signed-output-v1` | **required** `K×N` | omitted unless supplied | omitted unless declared | refuse unless `allow_replace` |
| silicon-hdl v3 reference | `silicon-hdl-v3-compatible` | `silicon-hdl-v3-profile-v1` | **required** `K×N` plus HDL-native `N×K` | omitted unless supplied | omitted unless declared | refuse unless `allow_replace` |
| Legacy Spikenaut-v2 | `spikenaut-v2-legacy` | `Spikenaut-v2` (historical tag) | optional | wall-clock RFC 3339 | declared `35.0` (not measured) | replace (historical) |

Schema version, producer crate / crate version, and profile / model
identity are separate `FpgaMetadata` fields. The corrected signed-output
contract is **not** a silent redefinition of `Spikenaut-v2`.

Matching `parameters.mem` / `parameters_weights.mem` /
`parameters_decay.mem` / `parameters_output_weights.mem` names does **not**
imply silicon-hdl compatibility. The pinned silicon-hdl v3 profile is the
explicit path that also writes `hdl_readout_neuron_major.mem` and records
the Host/HDL contract metadata.

## What to call

```rust
use silicon_bridge::FpgaParameterExporter;

let exporter = FpgaParameterExporter::from_params(
    vec![1.0, 0.5, 1.5, 0.75],
    vec![vec![0.5; 6]; 4],
    vec![0.9; 4],
);

// Default public path: no Spikenaut tag, no invented timing, no stdout.
let report = exporter.write_generic("out")?;
// equivalent: exporter.write_with_config("out", &ExportConfig::generic())?;
```

- **New callers** (default): `write_generic` /
  `ExportConfig::generic()` / `ExportConfig::default()`. Prefer these over
  `ParameterExport::export` and `MemFileWriter::write_mem_files`.
- **Custom HDL / board contract**: build an `ExportContract::custom(...)`
  with a caller-owned profile id, schema id, file layout, and
  `ReadoutContract`. Pass it through `ExportConfig::from_contract(...)`. A
  custom contract writes the same dense Q8.8 file format without inheriting
  Spikenaut branding or the pinned silicon-hdl compatibility metadata.
- **Required signed `K×N` readout**:
  `ExportConfig::generic_with_required_readout()`. Missing readout is an
  error (`ExportError::MissingRequiredReadout`). On-disk profile id remains
  `spikenaut-signed-output-v1` — not a silent redefinition of `Spikenaut-v2`.
  `ExportConfig::spikenaut_signed_output_v1()` is the Spikenaut-named alias.
- **Pinned silicon-hdl v3 reference profile**:
  `ExportConfig::silicon_hdl_v3()`. This profile supports exactly 16 input
  channels, 16 hidden neurons, and 3 output classes. It preserves
  `parameters_output_weights.mem` as generic class-major `K×N` and emits
  `hdl_readout_neuron_major.mem` as the reference HDL `OutputLayer`
  neuron-major `N×K` image. See [host-hdl-contract.md](host-hdl-contract.md).
- **Existing Spikenaut-v2 tooling** that keys on `version: "Spikenaut-v2"`,
  the documented filenames, overwrite-in-place, and a declared 35 µs
  target: `MemFileWriter::write_mem_files` /
  `ExportConfig::legacy_spikenaut_v2()`.

`ParameterExport::export` still stamps the historical in-memory bundle
(`Spikenaut-v2`, wall-clock timestamp, declared 35 µs). It does not write
files and it does not print. `try_export` is the default checked path: it
omits the Spikenaut tag, timestamps, and declared latency unless the caller
opts in.

## Overwrite

`write_mem_files` **replaces** files in the output directory. That is
deliberate compatibility.

`ExportConfig::generic()` and `generic_with_required_readout()` **refuse** if
any configured target already exists. Pass `.allow_replace()` for
explicit consent. Filename and overwrite checks run before any
create/truncate.

The shared writer stages the complete bundle in a
`.silicon-bridge-staging-*` subdirectory of the output directory, then
promotes each planned file into place. `OverwritePolicy::Prohibit` uses
an exclusive hard link / `create_new` so a dest that appears after the
pre-check (including a dangling symlink) is refused rather than replaced.
It also removes files already promoted if a later promote fails.
`OverwritePolicy::Replace` is still per-file, not a multi-file atomic
swap (Windows may `unlink` the destination before rename).

Unsafe names (absolute paths, `..` traversal, nested paths, collisions)
are rejected before writing.

## Metadata field changes

- `FpgaMetadata::target_latency_us` is now `Option<f32>` and is omitted
  from generic JSON unless the caller supplies a **declared** target.
- Empty `version` / `timestamp` are omitted from JSON.
- Profiled writes record `profile`, `schema_version`, `producer_crate`,
  `producer_version`, flattening (`row_major_dense` only), rounding
  (`truncate_toward_zero`), overflow policy, Q-format bit widths, and
  per-block shapes.
- `ExportConfig::silicon_hdl_v3()` also records
  `metadata.compatibility.contract_id`, a generic
  `metadata.compatibility.reference` object for the pinned `silicon-hdl` revision,
  supported dimensions, emitted filenames, readout source/HDL layouts,
  reset/timestep assumptions, and SiliconBridge v3.0 UART frame sizes.
- `write_mem_files` returns `ExportReport` instead of `()` and no longer
  prints a summary. The report does not claim board compatibility or
  measured latency.

Same input + same `ExportConfig` (omit or pin the timestamp) produces
byte-identical `.mem` and JSON.
