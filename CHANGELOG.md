# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `FpgaParameterExporter::validate` and `ParameterShapeError` — reject a weight
  matrix whose rows disagree in length. `MemFileWriter::write_mem_files` now
  validates before touching the filesystem, so a ragged matrix returns an error
  instead of writing `.mem` files that load misaligned into `WeightRam`.
  `ParameterExport::export` stays infallible and still flattens; its rustdoc
  points at `validate`.
- `FpgaBridge::open` and `is_fpga_port_name` (`uart` feature) — open a serial
  port by name, and classify a port name across Linux, macOS, and Windows.
- `encode_q88_unsigned` — free-function unsigned Q8.8 encoder shared by
  `FixedPointEncode::encode_q88`, `.mem` export, and `format_q88_hex` (#23).
- `encode_q88_signed`, `q88_signed_to_f32`, `STIMULUS_Q88_MIN`, and
  `STIMULUS_Q88_MAX` — signed Q8.8 host-stimulus helpers used by the UART TX/RX
  path (#23).
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
  resulting file is well-formed and loads cleanly. The export path now shares
  `encode_q88_signed` with the UART path, so `-1.0` lands on disk as `FF00`.

  No shipped FPGA image was affected: silicon-hdl's `merged_v2` bank came from
  the Spikenaut-SNN Julia export, not from this writer. This was a latent trap,
  not a live corruption.

  **Breaking.** `FixedPointEncode::encode_q88` and `FpgaParameterExporter::to_q88`
  return `i16` instead of `u16`; `FpgaParameters::{thresholds, weights, decay_rates}`
  are `Vec<i16>`, so `parameters.json` now records `-256` where it recorded `65280`.
  `format_q88_hex` formats the signed pattern (`-1.0` → `"FF00"`, was `"0000"`).
  Parameter magnitudes now saturate at `±127.99` rather than reaching `255.99`,
  matching silicon-hdl's `scripts/q88.py` clamp bit-for-bit. `encode_q88_unsigned`
  and `q88_to_f32` are unchanged and still public, but are documented as an
  unsigned-magnitude pair that must not be used to build hardware images.
- `FpgaMetrics::parse_from_report` now skips the rule of dashes Vivado prints
  under the `WNS(ns)` column headers, so a verbatim timing summary parses
  instead of returning `None` (#21).

### Changed

- `find_fpga_ports` now matches macOS `cu.usb*` / `tty.usb*` nodes and Windows
  `COM<n>` ports as well as Linux `ttyUSB` / `ttyACM`; it previously filtered on
  a `ttyUSB` substring and so returned an empty list off Linux. `FpgaBridge::new`
  probes the discovered ports, falling back to `/dev/ttyUSB0..2` only when
  enumeration finds nothing.
- License switched from GPL-3.0-or-later to dual MIT/Apache-2.0 (#6).
- Documented the Q8.8 conventions with a side-by-side table in the crate,
  module, and README docs, plus tests covering the clamp boundaries (#23).
  The table originally described the `.mem` path as unsigned; it now records the
  single signed convention both paths share (see Fixed, above).
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
