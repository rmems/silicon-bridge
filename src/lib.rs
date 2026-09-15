// SPDX-License-Identifier: MIT OR Apache-2.0
//! # silicon-bridge
//!
//! SNN-to-FPGA deployment pipeline for FPGA-backed neuromorphic hardware.
//!
//! This crate provides:
//! - **Q8.8 fixed-point parameter export** (`FixedPointEncode`, `ParameterExport`,
//!   `CheckedParameterExport`, `MemFileWriter`) for [silicon-hdl](https://github.com/rmems/silicon-hdl)
//!   `WeightRam` / `NeuronParamRam` via Vivado `$readmemh`
//! - **FPGA spike readback** over UART using the SiliconBridge v3.0 protocol
//! - **Vivado report parsing** for CI/CD gating on WNS, TNS, and LUT utilization
//!
//! Licensed under either of MIT or Apache-2.0 at your option.
//!
//! ## Q8.8 conventions
//!
//! Q8.8 always means “value × 256 packed into a 16-bit word”. Everything this
//! crate hands to silicon-hdl — `.mem` parameter images *and* UART host
//! stimuli — uses **one** convention: signed two's complement. `0xFF00` is
//! `-1.0`, not `65280`.
//!
//! | Aspect | Parameter export (`.mem`) | Host stimuli (UART TX/RX) |
//! |---|---|---|
//! | Encode with | [`FixedPointEncode::encode_q88`] / [`encode_q88_signed`] | [`encode_q88_signed`] |
//! | Decode with | [`q88_signed_to_f32`] | [`q88_signed_to_f32`] |
//! | Raw type | `i16` (two's complement) | `i16` (two's complement) |
//! | Width | 16 bits — 8 integer + 8 fractional | 16 bits — 8 integer + 8 fractional |
//! | Scaling | `raw = value × 256`, truncated toward zero | `raw = value × 256`, truncated toward zero |
//! | Encoder input clamp | [`STIMULUS_Q88_MIN`]`..=`[`STIMULUS_Q88_MAX`] (`-127.99..=127.99`) | same |
//! | Encoder raw output | `-32765..=32765` (saturates inside the `i16` limits) | same |
//! | Decoder accepts | any `i16`: `-32768..=32767` → `-128.0..=127.99609375` | same |
//! | Serialized as | ASCII hex, one `{:04X}` word of the raw pattern per line (`$readmemh`) | raw binary, big-endian (MSB first) |
//! | Consumed by | silicon-hdl `WeightRam` / `NeuronParamRam` | SiliconBridge v3.0 UART frame |
//! | Use it for | weights, thresholds, decay rates | host stimuli, RX membrane potentials |
//!
//! The two columns differ only in how the word reaches the FPGA — hex text in
//! a file versus big-endian bytes on a wire. The encoder saturates at `±32765`
//! while the decoder is wider, so an FPGA word of `0x8000` decodes to `-128.0`.
//! Encoding truncates toward zero and maps `NaN` to raw `0`.
//!
//! ### Why signed, and the unsigned pair
//!
//! silicon-hdl reads every `.mem` image as signed: `LifNeuron` /
//! `LifNeuronArray` and `OutputLayer` all `$signed`-compare at runtime, so a
//! Dale-inhibitory weight subtracts from the membrane rather than adding a
//! large positive (silicon-hdl `spikenaut-core-sv/mem/README.md`, “Signedness
//! contract (GH#73)”). The clamp bounds above are mirrored bit-for-bit by
//! silicon-hdl's `scripts/q88.py`; moving them desynchronises the two.
//!
//! [`encode_q88_unsigned`] and [`q88_to_f32`] remain public as an
//! unsigned-magnitude pair over `0.0..=255.99609375`, but they are **not** the
//! hardware convention. [`MemFileWriter::write_mem_files`] refuses
//! [`Q88Encoding::Unsigned`] (`ExportError::UnsignedHardwareEncoding`) because
//! unsigned words above 127.996 stored as `i16` are read as negatives by
//! signed FPGA RAM (`200.0` → `C800` → `-56.0`). Encoding a parameter bank
//! through [`encode_q88_unsigned`] also flattens every negative weight to
//! `0x0000` — a well-formed word that loads cleanly and silently drops the
//! inhibition.
//!
//! ## Provenance
//!
//! Extracted from Eagle-Lander, the author's own private neuromorphic GPU supervisor
//! repository (closed-source). The FPGA export pipeline deployed trained SNN parameters
//! to Basys3 hardware in production before being open-sourced as a standalone crate.
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
//! let params = exporter.export();
//! assert_eq!(params.weights[0], -256); // written to .mem as `FF00`
//! assert_eq!(q88_signed_to_f32(params.weights[0]), -1.0);
//! println!("Memory usage: {:.2} KB", params.metadata.memory_usage_kb);
//! // Prefer `try_export` / `CheckedParameterExport` when the bundle must be a
//! // valid FPGA image. Under the default `RangePolicy::Reject`, it rejects
//! // empty, ragged, mismatched, non-finite, and out-of-range values instead
//! // of flattening or saturating them. `RangePolicy::Saturate` is not applied
//! // by `try_export` (it would drop the clamp list); use
//! // `try_export_with_report` to clamp and inspect a `SaturationReport`.
//! ```
//!
//! ## FPGA Bridge (requires `uart` feature)
//!
//! ```toml
//! [dependencies]
//! silicon-bridge = { version = "0.1", features = ["uart"] }
//! ```

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

mod fpga_export;
mod fpga_metrics;

#[cfg(feature = "uart")]
mod fpga_bridge;

// Re-export public API
// Re-export public API
pub use fpga_export::{
    CheckedParameterExport, EXPORT_FORMAT_VERSION, ExportError, FixedPointEncode, FpgaMetadata,
    FpgaParameterExporter, FpgaParameters, MemFileWriter, NonFiniteKind, ParameterBlock,
    ParameterExport, ParameterLocation, ParameterShapeError, Q88_UNSIGNED_MAX, Q88Encoding,
    RangePolicy, STIMULUS_Q88_MAX, STIMULUS_Q88_MIN, SaturationEvent, SaturationReport,
    encode_q88_signed, encode_q88_unsigned, format_q88_hex, q88_signed_to_f32, q88_to_f32,
};

pub use fpga_metrics::FpgaMetrics;

#[cfg(feature = "uart")]
pub use fpga_bridge::{FpgaBridge, find_fpga_ports, is_fpga_port_name};
