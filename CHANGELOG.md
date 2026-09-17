# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Reference hardware** in README and `docs/consumer.md` (#83): Digilent
  Basys 3 / Xilinx Artix-7 `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`) as the
  named companion path with silicon-hdl `spikenaut_soc_basys3_top`,
  `basys3.xdc` / `basys3_soc.xdc`, and SiliconBridge v3.0 golden bytes
  that match Basys 3 firmware (not “any board”). Default public export
  remains board-agnostic Q8.8 `.mem`. silicon-hdl’s Artix-7 trainer XDC
  is documented as **not** the claimed reference demo. Prior board
  evidence is the silicon-hdl LED heartbeat smoke only; a silicon-bridge
  UART host session is the 0.3.0 publish gate (#84).

### Changed

- crates.io badge and install copy no longer imply a live registry crate.
  Git/path is the current install; `silicon-bridge = "0.3.0"` is shown
  only as the post-publish line. `cargo publish` of 0.3.0 is held until
  #84.

## [0.3.0] - 2026-09-17

First crates.io-intended release. In-tree version and install snippets are
`0.3.0` (UART milestone numbering; Toward-0.1.0 / v0.2.0 are closed). Registry
lookup for `silicon-bridge` returned 404 at packaging time — an authorized
human `cargo publish` is still required after merge.

### Added

- Default public export path is **generic-dense-q88** (#78):
  `ExportConfig::default()` equals `generic()`, and
  `FpgaParameterExporter::write_generic` writes that profile. Spikenaut
  remains explicit opt-in (`legacy_spikenaut_v2`,
  `spikenaut_signed_output_v1`, `MemFileWriter::write_mem_files`).
  `ExportConfig::generic_with_required_readout` is the recommended name
  for a required signed `K×N` readout; `spikenaut_signed_output_v1` is
  the compatibility alias (same on-disk profile id).
- One-page “bring your own floats” guidance in the README and
  `docs/consumer.md` (thresholds / weights / decay / optional readout →
  checked `.mem`).
- Dense export profiles (#50): `ExportConfig` / `write_with_config` with a
  neutral `generic-dense-q88` profile, an explicit
  `spikenaut-v2-legacy` compatibility profile, and a distinct
  `spikenaut-signed-output-v1` contract that requires a signed `K×N`
  readout. Schema version, producer crate version, and profile / model
  identity are separate metadata fields. The generic path omits timestamps
  and `target_latency_us` unless the caller supplies them (declared
  targets only, never measured latency), refuses overwrite without
  `allow_replace`, and returns an `ExportReport` instead of printing.
  Filenames are a small validated layout (unique relative basenames; no
  absolute paths or `..` traversal). Only dense row-major flattening is
  implemented. See [docs/export-profiles.md](docs/export-profiles.md).
- Independent Rust↔HDL golden export contracts (#53): committed
  `tests/golden/` fixtures (signed/unsigned Q8.8 tables, a non-square 4×6
  layer, and a synthetic 16-neuron Spikenaut-shaped bundle with signed
  readout) plus `tests/golden_export_contract.rs`. Expected words are
  hand-specified, not produced by the encoder. A vendored silicon-hdl
  `WeightRam` at `d45163f` and `tests/golden/hdl/run.sh` provide optional
  `$readmemh` simulation evidence (Icarus; not board-measured parity).
  The exporter K×N readout layout vs silicon-hdl `OutputLayer` N×K
  addressing is recorded, not silently rewritten. Independent UART
  request/response golden bytes for SiliconBridge v3 and simulated
  dense 8 / 32 / 8×10 profiles live in `tests/golden/uart/` (binary
  frames; not `.mem` hex).
- Framework-agnostic consumer examples and a public-release readiness
  guide (#54): `examples/generic_mem_export.rs` (4×6 checked export,
  deterministic metadata, no Spikenaut identity),
  `examples/spikenaut_profile_export.rs` (synthetic 16-neuron + signed
  readout under the `Spikenaut-v2` layout tag), and
  `examples/dense_codec.rs` (host codec only; no live UART).
  `FpgaParameterExporter::set_format_version` /
  `set_timestamp` / `use_wall_clock_timestamp` make JSON reproducible and
  let generic bundles drop the historical layout name. `set_timestamp`
  rejects non-RFC-3339 and non-UTC strings. README/rustdoc distinguish
  unpublished git/path installs from a future crates.io version (registry
  404 as of 2026-09-15); those install lines are rewritten to
  `silicon-bridge = "0.3.0"` on this candidate commit **before** `cargo publish`
  because the tarball is immutable.
  See `docs/consumer.md` and `docs/release-readiness.md`.
  `scripts/smoke-packaged-consumer.sh` builds an out-of-tree crate against
  `cargo package` output; that is not registry proof. The package allow-list
  ships `docs/consumer.md` and `docs/release-readiness.md` (not `docs/logo.png`)
  so README links in the unpacked crate resolve.
- Property tests and libFuzzer targets for Q8.8 and UART frame
  boundaries (Linear RM-1352): exhaustive `i16`/`u16` round-trips,
  proptest over non-finite and boundary-adjacent `f32`s, encode→decode
  round trips, truncation at every golden-frame byte, garbage
  prefix/suffix, unused spike-mask bits, and chunk-split decoder
  stability. `fuzz/` compiles in CI (`cargo fuzz build`); long campaigns
  stay opt-in. Seed corpus is the committed golden fixtures. Targets do
  not open a serial port.
- Explicit UART serial configuration (#51): `SerialConfig` (nonzero baud,
  finite nonzero per-I/O timeout), `FpgaBridge::open_with_config`,
  `FpgaBridge::builder`, `FpgaBridge::from_port` /
  `from_port_with_config` for an already-open transport, and
  `list_serial_ports` which returns enumerator errors instead of an empty
  list. Defaults stay 115200 baud / 100 ms (`DEFAULT_BAUD_RATE`,
  `DEFAULT_IO_TIMEOUT`). The explicit open path accepts any caller-selected
  device name and does not filter through `is_fpga_port_name`. Typed
  `SerialError` / `SerialConfigError` carry port and operation context;
  constructors do not print to stdout or send stimulus frames.
  `is_transport_open` is distinct from the `ping` latch (`is_active`).
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
  unreleased cycle (see Fixed, below), so it ships 0.3.0 wired to nothing.
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

- **MSRV:** `rust-version = "1.88.0"`. Validated on
  `rustc 1.88.0 (6b00bc388 2025-06-23)` and
  `rustc 1.98.1 (48a229cea 2026-09-01)` for this candidate. Edition 2024
  / `getrandom` 0.4 (via `tempfile`) need 1.85.0; this crate also uses
  `if`/`let` chains (1.88.0). CI still uses GitHub Actions `stable`.
  Cargo refuses toolchains older than 1.88.0.
- Checked `try_export` defaults to the generic path (#78): empty layout
  tag (no `Spikenaut-v2`), omitted timestamp, omitted `target_latency_us`.
  `ParameterExport::export` and `write_mem_files` remain the explicit
  Spikenaut-v2 legacy wrappers (historical tag, wall-clock stamp, declared
  35 µs). Rustdoc now steers new callers to `write_generic` /
  `ExportConfig::generic` (no `#[deprecated]` attribute: in-tree legacy
  tests and examples stay warning-clean under `-D warnings`).
- UART docs frame SiliconBridge v3.0 / Basys3 / `dense_*` as **example
  layouts with golden-byte evidence**, not “this crate only works with
  Basys3”. Changing host channel counts still does not reconfigure an
  FPGA.
- `write_with_config` (and therefore `write_generic` / `write_mem_files`)
  stages the complete bundle under a `.silicon-bridge-staging-*`
  subdirectory of `output_dir`, then promotes each planned file into
  place. `OverwritePolicy::Prohibit` uses an exclusive hard link /
  `create_new` so a dest that appears after the pre-check — including a
  dangling symlink, which `Path::exists` misses — is refused rather than
  replaced by Unix `rename`. It also deletes files already promoted if a
  later promote fails. `OverwritePolicy::Replace` is still a per-file
  rename, not a multi-file atomic swap (Windows may `unlink` the
  destination first).
- `docs/export-profiles.md` is now in the package `include` allow-list so
  the README profile migration note resolves from an unpacked crate.
  `AGENTS.md` / `CLAUDE.md` / `REVIEW.md` stay out of the tarball.
- `MemFileWriter::write_mem_files` is the legacy Spikenaut-v2 path: it
  still replaces existing files and records `version: "Spikenaut-v2"` plus
  a declared 35 µs target, but returns `ExportReport` instead of `()` and
  no longer prints a deployment summary. `FpgaMetadata::target_latency_us`
  is `Option<f32>` so generic JSON can omit it. Empty `version` /
  `timestamp` are skipped on serialize.
- Post-transfer hygiene: live docs, CI badge, package `homepage`, and MIT
  copyright point at [`rmems/silicon-bridge`](https://github.com/rmems/silicon-bridge)
  after return from Limen-Neural (#27). Wiki is enabled under `rmems`; durable
  docs stay in-repo. Sibling links that still live under Limen-Neural
  (`neuromod`, `nir-rs`) are left intact.
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
- `find_fpga_ports` returns `Result<Vec<SerialPortInfo>, SerialError>` instead
  of swallowing enumerator failures as `Vec::new()` (#51). `FpgaBridge::new`,
  `open`, and the former `Box<dyn Error>` constructors now return `SerialError`.
  `new` is documented as a legacy probe helper and no longer prints on
  connect. Opening a named port is the recommended path.
- License switched from GPL-3.0-or-later to dual MIT/Apache-2.0 (#6).
- Documented the Q8.8 conventions with a side-by-side table in the crate,
  module, and README docs, plus tests covering the clamp boundaries (#23).
  The table originally described the `.mem` path as unsigned; #60 recorded a
  single signed convention; #49 splits signed parameter, unsigned parameter,
  and legacy UART clamp (see Changed, above).
- `[package].rust-version` is `1.88.0` for this 0.3.0 candidate (language
  floor: edition 2024 plus `if`/`let` chains). The dependency graph would
  compile on 1.85.0. Cargo refuses toolchains older than 1.88.0. Toolchains
  older than 1.85 still fail while parsing `edition = "2024"` before
  `rust-version` is consulted. CI uses GitHub Actions `stable`.
- The published crate is now an `include` allow-list. `AGENTS.md`, `CLAUDE.md`,
  `REVIEW.md`, `.codacy.yml`, `.gitignore`, and `.github/` are no longer shipped
  to crates.io; the tarball drops from 19 files / 99.5 KiB to 12 / 81.5 KiB.
  `docs/` stays unpublished, as it was under the previous `exclude`.
- The crate root now carries `#![forbid(unsafe_code)]` and
  `#![deny(missing_docs)]`, and `FpgaMetadata` and its fields gained the `///`
  comments they were missing.
