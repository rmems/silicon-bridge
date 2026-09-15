# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- UART codecs split from transport (#52): `DenseQ88Layout`, `encode_stimuli`,
  `encode_stimuli_legacy_v3`, `decode_response`, and `StimulusResponse` live
  in a `serialport`-free module. SiliconBridge v3.0 is an explicit 16-channel
  profile; dense 8/16/32 (and non-multiple-of-8 mask) layouts are host codecs
  and need matching FPGA firmware. Checked encode rejects wrong-length and
  non-finite stimuli before any write. `FpgaBridge` gains `from_port`,
  `exchange` / `exchange_legacy` / `recover`; a failed I/O exchange sets
  `needs_recovery` and does not resend the stimulus. `process_stimuli` is
  the documented legacy pad/truncate wrapper and now returns `ExchangeError`.
- `FpgaParameterExporter::validate` and `ParameterShapeError` — reject a weight
  matrix whose rows disagree in length. `MemFileWriter::write_mem_files` now
  validates before touching the filesystem, so a ragged matrix returns an error
  instead of writing `.mem` files that load misaligned into `WeightRam`.
  `ParameterExport::export` stays infallible and still flattens; its rustdoc
  points at `validate`.
- Checked FPGA parameter export (#48): `CheckedParameterExport::try_export`,
  `FpgaParameterExporter::try_export_with_report`, and typed
  `ParameterShapeError` variants for empty layers, neuron-count mismatches,
  optional `K×N` readout shape, non-finite values, and out-of-range values.
  `MemFileWriter::write_mem_files` uses the checked path and returns
  `ExportError` (shape / I/O / JSON / unsigned hardware encoding) instead of
  `Box<dyn Error>`, preserving I/O causes. Out-of-range values are rejected
  under the default `RangePolicy::Reject`. `try_export` and
  `write_mem_files` refuse `RangePolicy::Saturate` so a `SaturationReport`
  cannot be dropped; use `try_export_with_report` to clamp and inspect.
  Hardware `.mem` writes refuse `Q88Encoding::Unsigned` because silicon-hdl
  RAM is signed (`200.0` as unsigned `C800` would read as `-56.0`).
  Re-exporting without a readout deletes a leftover
  `parameters_output_weights.mem`. The infallible `ParameterExport::export` is
  unchanged (flatten + saturate) and remains the documented legacy wrapper.
- Full-range signed parameter Q8.8 with explicit per-block encoding (#49):
  `encode_q88_signed_full`, `Q88_SIGNED_MIN` / `Q88_SIGNED_MAX`
  (`[-128, 127.99609375]` → `8000..=7FFF`), and `BlockEncodings` on
  `FpgaMetadata` so a consuming profile reads signedness from the manifest
  rather than inferring it from a filename or `i16` storage. Thresholds and
  decay may be selected `Q88Encoding::Unsigned` for in-memory consumers;
  hidden and readout weights default to signed. `encode_q88_signed` stays the
  UART helper with the ±127.99 clamp (`-128.0` → `8003` there, `8000` on the
  parameter path). Optional readout remains the #48 `K×N` matrix.
- `FpgaBridge::open` and `is_fpga_port_name` (`uart` feature) — open a serial
  port by name, and classify a port name across Linux, macOS, and Windows.
- `encode_q88_unsigned` — free-function unsigned-magnitude Q8.8 encoder (#23).
  It backed `FixedPointEncode::encode_q88`, `.mem` export, and `format_q88_hex`
  when it landed; all three moved to the signed encoder later in this same
  unreleased cycle (see Fixed, below), so it ships 0.1.0 wired to nothing.
- `encode_q88_signed`, `q88_signed_to_f32`, `STIMULUS_Q88_MIN`, and
  `STIMULUS_Q88_MAX` — signed Q8.8 helpers (#23). Introduced for the UART TX/RX
  path. After #60 they also backed `.mem` export; #49 moved `.mem` to
  `encode_q88_signed_full` (see Changed, below).
- `FpgaMetrics::tns_ns` field plus `parse_tns_from_report`, `parse_lut_utilization`,
  and `load_from_reports` — TNS from the timing summary data row and LUT
  utilization from `report_utilization` output. Absent values degrade to `0.0`
  instead of failing the parse, and `tns_ns` is `#[serde(default)]` so metrics
  serialized before it existed still deserialize (#21).
  New public field: code outside the crate that builds `FpgaMetrics` with a
  struct literal must add `tns_ns` (or `..Default::default()`). Serialized
  payloads and field-access code are unaffected.

### Fixed

- **`.mem` export is now signed two's-complement Q8.8.** `MemFileWriter::write_mem_files`
  encoded through `encode_q88_unsigned`, which clamps negatives to `0`, but
  silicon-hdl reads every `.mem` image as signed (`spikenaut-core-sv/mem/README.md`,
  "Signedness contract (GH#73)"): `LifNeuron` / `LifNeuronArray` and `OutputLayer`
  all `$signed`-compare at runtime. Exporting a Dale E/I bank through this crate
  therefore flattened every inhibitory weight to `0x0000` — silently, since the
  resulting file is well-formed and loads cleanly. The export path switched to
  signed encoding so `-1.0` lands on disk as `FF00`. #60 used
  `encode_q88_signed` (UART clamp); #49 uses `encode_q88_signed_full`.

  No shipped FPGA image was affected: silicon-hdl's `merged_v2` bank came from
  the Spikenaut-SNN Julia export, not from this writer. This was a latent trap,
  not a live corruption.

  **Breaking.** `FixedPointEncode::encode_q88` and `FpgaParameterExporter::to_q88`
  return `i16` instead of `u16`; `FpgaParameters::{thresholds, weights, decay_rates}`
  are `Vec<i16>`, so `parameters.json` now records `-256` where it recorded `65280`.
  `format_q88_hex` formats the signed pattern (`-1.0` → `"FF00"`, was `"0000"`).
  Parameter magnitudes saturate at `±127.99` on the UART helper; the later
  #49 change (see Changed) widens the parameter path to the full `i16` range.
  `encode_q88_unsigned` and `q88_to_f32` are unchanged and still public, but are
  documented as an unsigned-magnitude pair that must not be used to build
  hardware images.

  **Reading old `parameters.json` fails.** Any word above `32767` — every
  pre-fix threshold or weight over `127.99` — no longer deserializes into the
  `i16` fields, and `EXPORT_FORMAT_VERSION` is deliberately *not* bumped, so the
  payload changed under an unchanged `"Spikenaut-v2"` tag. The failure is a loud
  serde error rather than a silent misread, and the tag is documented as a
  layout identifier that downstream tooling keys on (bumping it breaks that
  tooling). Re-export rather than migrating old JSON in place. Signedness is
  now an explicit per-block field on `FpgaMetadata::encodings` (#49), not a
  bump of this layout tag.
- `FpgaMetrics::parse_from_report` now skips the rule of dashes Vivado prints
  under the `WNS(ns)` column headers, so a verbatim timing summary parses
  instead of returning `None` (#21).

### Changed

- Parameter `.mem` export uses full-range signed Q8.8 (`encode_q88_signed_full`)
  instead of the UART helper's ±127.99 clamp (#49). `-128.0` is `8000` on the
  parameter path and remains `8003` on UART (`encode_q88_signed`). Docs
  distinguish signed parameter, unsigned parameter, and legacy UART encodings.
  `FpgaMetadata` gains `encodings` (`BlockEncodings`, `#[serde(default)]` so
  older JSON still loads) and implements `Default` (all-signed encodings,
  empty provenance) so struct literals can use `..Default::default()`. A
  legacy `parameters.json` with top-level `output_weights` but no encoding
  metadata records the readout as signed.
- `find_fpga_ports` now matches macOS `cu.usb*` / `tty.usb*` nodes and Windows
  `COM<n>` ports as well as Linux `ttyUSB` / `ttyACM`; it previously filtered on
  a `ttyUSB` substring and so returned an empty list off Linux. `FpgaBridge::new`
  probes the discovered ports, falling back to `/dev/ttyUSB0..2` only when
  enumeration finds nothing.
- License switched from GPL-3.0-or-later to dual MIT/Apache-2.0 (#6).
- Documented the Q8.8 conventions with a side-by-side table in the crate,
  module, and README docs, plus tests covering the clamp boundaries (#23).
  The table originally described the `.mem` path as unsigned; #60 recorded a
  single signed convention; #49 splits signed parameter, unsigned parameter,
  and legacy UART clamp (see Changed, above).
- Declared `rust-version = "1.98.1"`, not `"1.85"`, the dependency-derived
  floor — edition 2024 and `getrandom` 0.4 (via the `tempfile` dev-dependency)
  both only require 1.85.0. This is a deliberate policy choice to track the
  toolchain validated at authorship rather than the minimum the graph needs, and
  it narrows compatibility versus a bare 1.85 declaration: Cargo will refuse
  to build this crate on any toolchain from 1.85.0 up to (but excluding)
  1.98.1, even though every such toolchain can actually compile it. A
  toolchain older than 1.85 still gets the pre-existing `edition = "2024"`
  parse error instead, because Cargo rejects that while parsing the manifest,
  before `rust-version` is consulted.
- The published crate is now an `include` allow-list. `AGENTS.md`, `CLAUDE.md`,
  `REVIEW.md`, `.codacy.yml`, `.gitignore`, and `.github/` are no longer shipped
  to crates.io; the tarball drops from 19 files / 99.5 KiB to 12 / 81.5 KiB.
  `docs/` stays unpublished, as it was under the previous `exclude`.
- The crate root now carries `#![forbid(unsafe_code)]` and
  `#![deny(missing_docs)]`, and `FpgaMetadata` and its fields gained the `///`
  comments they were missing.
