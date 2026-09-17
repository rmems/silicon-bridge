// SPDX-License-Identifier: MIT OR Apache-2.0
//! # silicon-bridge
//!
//! SNN-to-FPGA deployment pipeline for FPGA-backed neuromorphic hardware.
//!
//! This crate provides:
//! - **Q8.8 fixed-point parameter export** (`FixedPointEncode`, `ParameterExport`,
//!   `CheckedParameterExport`, `MemFileWriter`, `ExportConfig`) for [silicon-hdl](https://github.com/rmems/silicon-hdl)
//!   `WeightRam` / `NeuronParamRam` via Vivado `$readmemh`. The default public
//!   path is [`ExportConfig::generic`] / [`FpgaParameterExporter::write_generic`]
//!   (`generic-dense-q88`). Prefer that over [`ParameterExport::export`] /
//!   [`MemFileWriter::write_mem_files`] (legacy Spikenaut-v2). Required
//!   readout: [`ExportConfig::generic_with_required_readout`].
//! - **UART codecs** (`DenseQ88Layout`, `encode_stimuli`,
//!   `decode_response`) that do not depend on `serialport`. SiliconBridge v3.0
//!   is an example 16-channel layout with golden-byte evidence matching
//!   current Basys 3 firmware, not a claim that any board speaks the frame.
//! - **Optional UART I/O** (`uart` feature) for blocking spike exchange on a
//!   caller-selected port
//! - **Vivado report parsing** for CI/CD gating on WNS, TNS, and LUT utilization
//!
//! Licensed under either of MIT or Apache-2.0 at your option.
//!
//! ## Installation
//!
//! This crate is **not on crates.io or docs.rs**. `[package].version` is
//! `0.3.0`; an authorized `cargo publish` is held until the Basys 3 host
//! path is proven (GitHub #84). Until then, depend on git or a path:
//!
//! ```toml
//! silicon-bridge = { git = "https://github.com/rmems/silicon-bridge" }
//! ```
//!
//! Optional UART I/O:
//!
//! ```toml
//! silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", features = ["uart"] }
//! ```
//!
//! After crates.io actually serves a version (intended first release: 0.3.0):
//!
//! ```toml
//! silicon-bridge = "0.3.0"
//! ```
//!
//! `rust-version` is `1.88.0` (language floor). See the crate README and
//! `docs/release-readiness.md` for the authorized `cargo publish` checklist
//! and the distinction between that floor, the 1.85.0 dependency graph, and
//! CI `stable`. This rustdoc does **not** claim crates.io or docs.rs are
//! already live.
//!
//! ## Reference hardware
//!
//! The default public path is board-agnostic Q8.8 `.mem` export
//! (`write_generic`). The **named reference companion path** is Digilent Basys 3
//! (Xilinx Artix-7 `XC7A35T-1CPG236C`, Vivado part `xc7a35tcpg236-1`) plus
//! [silicon-hdl](https://github.com/rmems/silicon-hdl)
//! `spikenaut_soc_basys3_top` (`Basys3_Top.sv`), with
//! `constraints/basys3.xdc` and `basys3_soc.xdc`. silicon-hdl also ships
//! `artix7_trainer.xdc`; that pinout is **not** the claimed reference demo.
//! Prior physical evidence is an LED heartbeat smoke (silicon-hdl #68), not
//! a silicon-bridge UART host session (that is silicon-bridge #84).
//!
//! Offline examples (`examples/generic_mem_export.rs`,
//! `examples/spikenaut_profile_export.rs`, `examples/dense_codec.rs`) use only
//! this public surface. They do not open serial ports or send live stimuli.
//!
//! ## Q8.8 conventions
//!
//! Q8.8 always means “value × 256 packed into a 16-bit word”, truncated toward
//! zero. This crate keeps **three** interpretations of that word. Per-block
//! signedness is selected with [`FpgaParameterExporter::set_encoding`] and
//! recorded on [`FpgaMetadata::encodings`] — never inferred from a filename
//! or from storing the word as `i16`.
//!
//! | Aspect | Signed parameter (`.mem`) | Unsigned parameter | Legacy UART clamp |
//! |---|---|---|---|
//! | Encode with | [`encode_q88_signed_full`] / [`FixedPointEncode::encode_q88`] | [`encode_q88_unsigned`] | [`encode_q88_signed`] |
//! | Decode with | [`q88_signed_to_f32`] | [`q88_to_f32`] | [`q88_signed_to_f32`] |
//! | Range | [`Q88_SIGNED_MIN`]`..=`[`Q88_SIGNED_MAX`] (`-128..=127.99609375`) | `0..=`[`Q88_UNSIGNED_MAX`] (`0..=255.99609375`) | [`STIMULUS_Q88_MIN`]`..=`[`STIMULUS_Q88_MAX`] (`-127.99..=127.99`) |
//! | Raw output | `i16::MIN..=i16::MAX` (`8000`..=`7FFF`) | `u16` `0000`..=`FFFF` | `-32765..=32765` (`8003`..=`7FFD`) |
//! | Serialized as | ASCII hex (`{:04X}` of the 16-bit pattern) | same hex of the unsigned pattern | raw binary, big-endian |
//! | Consumed by | silicon-hdl `WeightRam` / `NeuronParamRam` | in-memory unsigned consumers | SiliconBridge v3.0 UART frame |
//! | Use it for | hidden + readout weights; hardware `.mem` | thresholds/decay when explicitly unsigned | host stimuli, RX membrane potentials |
//!
//! silicon-hdl reads every `.mem` image as signed two's complement: `0xFF00`
//! is `-1.0`, not `65280` (silicon-hdl GH#73).
//! [`MemFileWriter::write_mem_files`] therefore refuses
//! [`Q88Encoding::Unsigned`] (`ExportError::UnsignedHardwareEncoding`):
//! unsigned words above 127.996 stored as `i16` are read as negatives
//! (`200.0` → `C800` → `-56.0`). Thresholds and decay **may** be selected
//! unsigned for existing in-memory consumers; hidden and readout weights
//! default to signed so inhibitory values survive.
//!
//! [`encode_q88_signed`] is the UART helper only. Its ±127.99 clamp is a wire
//! protocol artifact: `-128.0` encodes as `8003` there and as `8000` on the
//! parameter path. Encoding truncates toward zero and maps `NaN` to raw `0`
//! on the saturating helpers; the checked export path rejects non-finite
//! inputs and, under [`RangePolicy::Reject`], overflow.
//!
//! ## Provenance
//!
//! Extracted from Eagle-Lander, the author's own private neuromorphic GPU
//! supervisor (closed-source). The default public path is framework-agnostic
//! dense Q8.8 export. The named reference companion board is Digilent Basys 3 plus
//! silicon-hdl; Spikenaut export layouts remain opt-in profiles.
//!
//! ## Quick Start
//!
//! ```rust
//! use silicon_bridge::{FpgaParameterExporter, q88_signed_to_f32};
//!
//! let mut exporter = FpgaParameterExporter::new();
//! exporter.set_thresholds(vec![1.0; 16]);
//! // Row 0 is Dale-inhibitory: negative weights survive as two's complement.
//! let mut weights = vec![vec![0.5; 16]; 16];
//! weights[0] = vec![-1.0; 16];
//! exporter.set_weights(weights);
//! exporter.set_decay_rates(vec![0.85; 16]);
//!
//! let params = exporter.try_export().expect("rectangular, finite, in-range");
//! assert_eq!(params.weights[0], -256); // written to .mem as `FF00`
//! assert_eq!(q88_signed_to_f32(params.weights[0]), -1.0);
//! assert!(params.metadata.version.is_empty()); // no Spikenaut tag
//! println!("Memory usage: {:.2} KB", params.metadata.memory_usage_kb);
//! // Disk writes: `write_generic` / `ExportConfig::generic` for a
//! // framework-agnostic bundle. `write_mem_files` only when the historical
//! // Spikenaut-v2 layout is required. `ParameterExport::export` remains the
//! // documented legacy wrapper (flatten + saturate + Spikenaut-v2 stamp).
//! ```
//!
//! Prefer [`CheckedParameterExport::try_export`] when the bundle must be a
//! valid FPGA image. Under the default [`RangePolicy::Reject`], it rejects
//! empty, ragged, mismatched, non-finite, and out-of-range values instead of
//! flattening or saturating them. [`RangePolicy::Saturate`] is not applied
//! by `try_export` (it would drop the clamp list); use
//! [`FpgaParameterExporter::try_export_with_report`] to clamp and inspect a
//! [`SaturationReport`].
//!
//! The default checked path omits `parameters.json` layout tags, timestamps,
//! and declared latency. Disk writes use [`FpgaParameterExporter::write_generic`]
//! (`generic-dense-q88`). See `examples/generic_mem_export.rs`.
//!
//! ## FPGA Bridge (requires `uart` feature)
//!
//! ```toml
//! [dependencies]
//! silicon-bridge = { git = "https://github.com/rmems/silicon-bridge", features = ["uart"] }
//! ```
//!
//! Prefer [`FpgaBridge::open_with_config`] or [`FpgaBridge::builder`] with an
//! explicit port, baud rate, and per-I/O timeout. [`FpgaBridge::new`] remains
//! as a legacy probe helper and is not the recommended path. Opening a named
//! port does not require `libudev`; Linux enumeration via
//! [`list_serial_ports`] typically does.
//!
//! Frame encode/decode is [`encode_stimuli`] / [`decode_response`] and does
//! not need this feature. [`DenseQ88Layout::silicon_bridge_v3`] is the
//! 16-channel example layout with golden-byte evidence matching current
//! Basys 3 firmware, not a claim that any board speaks this frame. Other
//! layouts need matching FPGA firmware. Changing host channel counts does
//! not reconfigure a board.

// `forbid(unsafe_code)` enforces an AGENTS.md *constraint* — "do not add
// `unsafe` code without explicit safety justification" — mechanically. Nothing
// in the crate is unsafe today, so the strictest form costs nothing; lifting it
// for a justified block is a deliberate edit, which is the point.
#![forbid(unsafe_code)]
// `deny(missing_docs)` is a deliberate tightening, not enforcement of an
// existing hard rule: AGENTS.md lists "public items get `///` doc comments"
// under conventions, and a convention did not stop `FpgaMetadata` and its six
// fields from landing undocumented. This crate emits `.mem` files that get baked
// into a bitstream, so an undocumented public item is a hardware-facing
// ambiguity worth a compile error.
#![deny(missing_docs)]

mod fpga_codec;
mod fpga_export;
mod fpga_metrics;

#[cfg(feature = "uart")]
mod fpga_bridge;

// Re-export public API
pub use fpga_export::{
    BlockEncodings, BlockShape, BundleShapes, CheckedParameterExport, EXPORT_FORMAT_VERSION,
    ExportConfig, ExportError, ExportFileLayout, ExportProfile, ExportReport, FilenameError,
    FixedPointEncode, FpgaMetadata, FpgaParameterExporter, FpgaParameters,
    GENERIC_DENSE_PROFILE_ID, GENERIC_DENSE_SCHEMA_VERSION, MatrixFlattening, MemFileWriter,
    MetadataTimestampError, NonFiniteKind, OverflowPolicy, OverwritePolicy, PRODUCER_CRATE,
    ParameterBlock, ParameterExport, ParameterLocation, ParameterShapeError, Q88_SIGNED_MAX,
    Q88_SIGNED_MIN, Q88_UNSIGNED_MAX, Q88Encoding, QFormat, RangePolicy, ReadoutShape,
    RoundingMode, SPIKENAUT_LEGACY_TARGET_LATENCY_US, SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID,
    SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION, SPIKENAUT_V2_LEGACY_PROFILE_ID, STIMULUS_Q88_MAX,
    STIMULUS_Q88_MIN, SaturationEvent, SaturationReport, TimestampPolicy, encode_q88_signed,
    encode_q88_signed_full, encode_q88_unsigned, format_q88_hex, q88_signed_to_f32, q88_to_f32,
};

pub use fpga_metrics::FpgaMetrics;

pub use fpga_codec::{
    CodecError, DENSE_Q88_SYNC, DenseQ88Layout, MAX_DENSE_CHANNELS, SILICON_BRIDGE_V3_CHANNELS,
    SILICON_BRIDGE_V3_RX_LEN, SILICON_BRIDGE_V3_TX_LEN, StimulusResponse, decode_response,
    encode_stimuli, encode_stimuli_legacy_v3,
};

#[cfg(feature = "uart")]
pub use fpga_bridge::{
    DEFAULT_BAUD_RATE, DEFAULT_IO_TIMEOUT, ExchangeError, FpgaBridge, FpgaBridgeBuilder,
    SerialConfig, SerialConfigError, SerialConfigureOp, SerialError, find_fpga_ports,
    is_fpga_port_name, list_serial_ports,
};
