// SPDX-License-Identifier: MIT OR Apache-2.0
//! FPGA parameter export for [silicon-hdl](https://github.com/rmems/silicon-hdl).
//!
//! Converts trained SNN floats to signed two's-complement **Q8.8** (`i16`)
//! vectors and writes Vivado `$readmemh` `.mem` files consumable by `WeightRam`
//! and `NeuronParamRam` in the silicon-hdl core library.
//!
//! ## Traits
//!
//! Hardware-facing crates should depend on the traits here rather than the
//! concrete [`FpgaParameterExporter`] type when possible:
//!
//! - [`FixedPointEncode`] — `f32` → Q8.8 `i16`
//! - [`ParameterExport`] — produce [`FpgaParameters`] (infallible, legacy)
//! - [`CheckedParameterExport`] — produce [`FpgaParameters`] or a typed
//!   [`ParameterShapeError`]
//! - [`MemFileWriter`] — write `.mem` + metadata JSON
//!
//! ## Three Q8.8 encodings
//!
//! Q8.8 always means `value × 256` packed into a 16-bit word, truncated toward
//! zero. This crate keeps **three** interpretations of that word. Signedness is
//! selected per [`ParameterBlock`] via [`FpgaParameterExporter::set_encoding`]
//! and recorded on [`FpgaMetadata::encodings`] — never inferred from a
//! filename or from storing the word as `i16`.
//!
//! | Aspect | Signed parameter (`.mem`) | Unsigned parameter | Legacy UART clamp |
//! |---|---|---|---|
//! | Encode with | [`encode_q88_signed_full`] | [`encode_q88_unsigned`] | [`encode_q88_signed`] |
//! | Decode with | [`q88_signed_to_f32`] | [`q88_to_f32`] | [`q88_signed_to_f32`] |
//! | Range | [`Q88_SIGNED_MIN`]`..=`[`Q88_SIGNED_MAX`] (`-128..=127.99609375`) | `0..=`[`Q88_UNSIGNED_MAX`] (`0..=255.99609375`) | [`STIMULUS_Q88_MIN`]`..=`[`STIMULUS_Q88_MAX`] (`-127.99..=127.99`) |
//! | Raw output | `i16::MIN..=i16::MAX` (`8000`..=`7FFF`) | `u16` `0000`..=`FFFF` | `-32765..=32765` (`8003`..=`7FFD`) |
//! | Serialized as | ASCII hex, one `{:04X}` word of the 16-bit pattern | same hex of the unsigned pattern | raw binary, big-endian |
//! | Use it for | hidden + readout weights; hardware `.mem` images | in-memory unsigned consumers; thresholds/decay when explicitly selected | UART host stimuli only |
//!
//! silicon-hdl RAM reads every `.mem` image as **signed** two's complement
//! (`0xFF00` is `-1.0`, not `65280`). `LifNeuron` / `LifNeuronArray` and
//! `OutputLayer` `$signed`-compare at runtime, so a Dale-inhibitory weight
//! subtracts from the membrane. See silicon-hdl
//! `spikenaut-core-sv/mem/README.md`, “Signedness contract (GH#73)”.
//! [`MemFileWriter::write_mem_files`] therefore refuses
//! [`Q88Encoding::Unsigned`].
//!
//! [`encode_q88_signed`] is the UART helper. Its ±127.99 clamp is a wire
//! protocol artifact, **not** the parameter-export contract: `-128.0` encodes
//! as `8003` on the UART path and as `8000` on the parameter path. Do not
//! reuse that helper for `.mem` images.
//!
//! ## Export profiles
//!
//! The default public disk path is **generic-dense-q88**:
//! [`FpgaParameterExporter::write_generic`] or
//! [`FpgaParameterExporter::write_with_config`] with [`ExportConfig::generic`]
//! (also [`ExportConfig::default`]). That path writes no Spikenaut tag, no
//! timestamp unless supplied, and no `target_latency_us` unless declared.
//!
//! [`MemFileWriter::write_mem_files`] is the **explicit** legacy Spikenaut-v2
//! compatibility writer: documented filenames, a wall-clock timestamp, and a
//! declared 35 µs target (not a measurement). Required-readout bundles use
//! [`ExportConfig::generic_with_required_readout`]
//! ([`ExportConfig::spikenaut_signed_output_v1`] is the Spikenaut-named
//! alias). That contract is a distinct profile/schema, not a silent
//! redefinition of `Spikenaut-v2`. [`ExportConfig::silicon_hdl_v3`] is the
//! pinned reference HDL profile that also emits an `N×K` readout image for
//! silicon-hdl `OutputLayer`. See [`ExportConfig`].

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

#[path = "export_profile.rs"]
mod export_profile;

pub use export_profile::{
    BlockShape, BundleShapes, CompatibilityDimensions, CompatibilityFiles, CompatibilityMetadata,
    ContractReference, ExportConfig, ExportContract, ExportFileLayout, ExportProfile, ExportReport,
    GENERIC_DENSE_PROFILE_ID, GENERIC_DENSE_SCHEMA_VERSION, OverwritePolicy, PRODUCER_CRATE,
    ReadoutContract, ReadoutShape, SILICON_HDL_V3_CONTRACT_ID, SILICON_HDL_V3_HIDDEN_NEURONS,
    SILICON_HDL_V3_INPUT_CHANNELS, SILICON_HDL_V3_OUTPUT_CLASSES, SILICON_HDL_V3_PROFILE_ID,
    SILICON_HDL_V3_READOUT_FILENAME, SILICON_HDL_V3_SCHEMA_VERSION,
    SILICON_HDL_V3_SUPPORTED_REVISION, SPIKENAUT_LEGACY_TARGET_LATENCY_US,
    SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID, SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION,
    SPIKENAUT_V2_LEGACY_PROFILE_ID, TimestampPolicy, UartContractMetadata,
};

/// Metadata / layout tag for the Q8.8 `.mem` bundle shared with silicon-hdl.
///
/// Historical name from the Spikenaut deployment pipeline; kept for the
/// [`ExportProfile::LegacySpikenautV2`] compatibility profile. Generic and
/// corrected signed-output profiles use distinct schema strings — they do
/// not silently redefine this tag.
pub const EXPORT_FORMAT_VERSION: &str = "Spikenaut-v2";

/// Dense matrix flattening recorded in export metadata.
///
/// Only dense row-major is implemented. Other layouts are rejected by
/// [`MatrixFlattening::parse_name`] rather than exposed as unimplemented
/// options. This is the addressing `addr = row * width + col`, not CSR, and
/// not UART byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MatrixFlattening {
    /// Dense row-major: `addr = row * width + col`.
    RowMajorDense,
}

impl MatrixFlattening {
    /// Parse a flattening name. Only `row_major` / `row_major_dense` succeed.
    pub fn parse_name(name: &str) -> Result<Self, ExportError> {
        match name {
            "row_major" | "row_major_dense" => Ok(Self::RowMajorDense),
            other => Err(ExportError::UnsupportedFlattening {
                requested: other.to_string(),
            }),
        }
    }
}

/// Quantization rounding recorded in export metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RoundingMode {
    /// `trunc(value × 256)` toward zero. The Q8.8 encoder in this crate.
    TruncateTowardZero,
}

/// Overflow handling recorded in export metadata.
///
/// Mirrors [`RangePolicy`] for the on-disk manifest. The generic profile
/// records whatever policy the checked export actually used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverflowPolicy {
    /// Out-of-range finite values were rejected.
    Reject,
    /// Out-of-range finite values were clamped (only with a saturation report).
    Saturate,
}

/// Stored Q-format of each `.mem` word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QFormat {
    /// Total bits of the stored word.
    pub total_bits: u8,
    /// Fractional bits (scale `2^fractional_bits`).
    pub fractional_bits: u8,
}

impl QFormat {
    /// 16-bit Q8.8 (`value × 256` packed into `i16`).
    pub const Q8_8: Self = Self {
        total_bits: 16,
        fractional_bits: 8,
    };
}

/// Why a configured output filename was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilenameError {
    /// Empty string.
    Empty,
    /// Absolute path (Unix `/…` or equivalent).
    Absolute,
    /// `..` component or `..` substring that would escape `output_dir`.
    ParentTraversal,
    /// More than a single path component (directory separators).
    NotABasename,
    /// Characters outside the portable `[A-Za-z0-9._-]` set.
    InvalidCharacters,
}

impl fmt::Display for FilenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "filename is empty"),
            Self::Absolute => write!(f, "filename is an absolute path"),
            Self::ParentTraversal => {
                write!(
                    f,
                    "filename contains '..' traversal outside the output directory"
                )
            }
            Self::NotABasename => {
                write!(f, "filename is not a single relative basename")
            }
            Self::InvalidCharacters => {
                write!(f, "filename contains characters outside [A-Za-z0-9._-]")
            }
        }
    }
}

/// Encode host floating-point values as **full-range** signed Q8.8 (`i16`).
///
/// Q8.8 maps `value × 256` into a 16-bit word. This trait is the
/// parameter-export encoder: [`encode_q88_signed_full`], range
/// [`Q88_SIGNED_MIN`]`..=`[`Q88_SIGNED_MAX`]. It is **not** the UART helper
/// [`encode_q88_signed`], whose narrower ±127.99 clamp would map `-128.0` to
/// `8003` instead of `8000`.
///
/// Per-block signed/unsigned selection lives on
/// [`FpgaParameterExporter::set_encoding`]. The unsigned pair
/// [`encode_q88_unsigned`] / [`q88_to_f32`] cannot express an inhibitory
/// weight — see the crate-root “Q8.8 conventions” table.
pub trait FixedPointEncode {
    /// Convert one `f32` to full-range signed Q8.8 fixed-point.
    ///
    /// Inputs below [`Q88_SIGNED_MIN`] saturate to `i16::MIN` (`8000`) and
    /// inputs above [`Q88_SIGNED_MAX`] saturate to `i16::MAX` (`7FFF`); `NaN`
    /// encodes as `0`. Negatives survive as two's complement, so `-1.0`
    /// encodes as `-256` and is written to a `.mem` file as `FF00`.
    fn encode_q88(&self, value: f32) -> i16;
}

/// Export SNN parameters as an FPGA-facing Q8.8 parameter bundle.
///
/// **New callers:** use [`CheckedParameterExport::try_export`] for an
/// in-memory image and [`FpgaParameterExporter::write_generic`] /
/// [`ExportConfig::generic`] for the default disk path. This trait is the
/// documented **legacy** in-memory wrapper (flatten, saturate, stamp
/// `Spikenaut-v2`). It does not write files.
///
/// The resulting [`FpgaParameters`] align with silicon-hdl RAM contents
/// (`WeightRam`, `NeuronParamRam`).
pub trait ParameterExport {
    /// Build the full Q8.8 parameter set and metadata.
    ///
    /// Weight rows are flattened row-major, and the width is reported as
    /// `FpgaMetadata::num_channels` from the first row alone. This method is
    /// **infallible**: a ragged matrix still flattens, neuron counts are not
    /// cross-checked, and `NaN` / out-of-range values saturate through the
    /// encoder. That is the documented legacy contract.
    ///
    /// Prefer [`CheckedParameterExport::try_export`] or
    /// [`FpgaParameterExporter::validate`] before producing an FPGA image, or
    /// write through [`FpgaParameterExporter::write_generic`] (default
    /// generic-dense-q88 path). [`MemFileWriter::write_mem_files`] is the
    /// explicit Spikenaut-v2 compatibility writer. This method is **not**
    /// marked `#[deprecated]` so in-tree legacy tests and examples stay
    /// warning-clean under `-D warnings`.
    fn export(&self) -> FpgaParameters;
}

/// Checked SNN parameter export that rejects a malformed bundle.
///
/// Unlike [`ParameterExport::export`], this path never produces a
/// valid-looking parameter image from empty, ragged, dimension-mismatched,
/// non-finite, or (under the default [`RangePolicy::Reject`]) out-of-range
/// inputs. [`RangePolicy::Saturate`] is rejected here so a
/// [`SaturationReport`] cannot be dropped; use
/// [`FpgaParameterExporter::try_export_with_report`] to clamp and inspect.
pub trait CheckedParameterExport {
    /// Typed validation failure. Implementors should use
    /// [`ParameterShapeError`] rather than a parallel error enum.
    type Error;

    /// Validate the complete bundle and encode it.
    ///
    /// Out-of-range rejection depends on [`RangePolicy::Reject`] (the
    /// default). [`RangePolicy::Saturate`] must go through
    /// [`FpgaParameterExporter::try_export_with_report`].
    fn try_export(&self) -> Result<FpgaParameters, Self::Error>;
}

/// Write Q8.8 parameter vectors as Vivado `$readmemh` `.mem` files.
///
/// **New callers:** [`FpgaParameterExporter::write_generic`] /
/// [`ExportConfig::generic`]. This trait is the documented **legacy**
/// Spikenaut-v2 disk writer.
pub trait MemFileWriter {
    /// Filesystem / I/O failures, plus a parameter bundle that cannot be
    /// represented as a flat `.mem` file.
    type Error;

    /// Write the **legacy Spikenaut-v2** bundle under `output_dir`.
    ///
    /// Prefer [`FpgaParameterExporter::write_generic`] for new disk writes.
    /// Call this method only when a consumer keys on `version: "Spikenaut-v2"`,
    /// overwrite-in-place, a wall-clock timestamp, and the declared 35 µs
    /// target. Not marked `#[deprecated]` so in-tree compatibility tests
    /// stay warning-clean under `-D warnings`.
    ///
    /// Documented filenames are `parameters.mem`, `parameters_weights.mem`,
    /// `parameters_decay.mem`, optional `parameters_output_weights.mem`, and
    /// `parameters.json`. Those names do not by themselves imply HDL
    /// compatibility.
    ///
    /// **Overwrite:** this compatibility path **replaces** existing files in
    /// `output_dir` (historical behaviour). The default generic profile
    /// ([`FpgaParameterExporter::write_generic`] /
    /// [`ExportConfig::generic`]) refuses replacement unless
    /// [`ExportConfig::allow_replace`] is set.
    ///
    /// Implementations must validate the complete bundle **before** creating
    /// or truncating any output file. A validation failure leaves existing
    /// files untouched. The shared writer stages the complete bundle in a
    /// temporary subdirectory, then renames each file into place (see
    /// [`FpgaParameterExporter::write_with_config`]). The writer returns an
    /// [`ExportReport`] and does **not** print to stdout.
    fn write_mem_files(&self, output_dir: impl AsRef<Path>) -> Result<ExportReport, Self::Error>;
}

/// Default FPGA parameter exporter for dense Q8.8 `.mem` images.
///
/// Exports learned SNN parameters in Q8.8 fixed-point format. The default
/// public disk path is [`Self::write_generic`] (`generic-dense-q88`).
/// [`MemFileWriter::write_mem_files`] is the explicit Spikenaut-v2
/// compatibility writer (historical tag, wall-clock timestamp, declared
/// 35 µs target, overwrite-in-place).
pub struct FpgaParameterExporter {
    thresholds: Vec<f32>,
    weights: Vec<Vec<f32>>,
    decay_rates: Vec<f32>,
    output_weights: Option<Vec<Vec<f32>>>,
    threshold_encoding: Q88Encoding,
    weight_encoding: Q88Encoding,
    decay_encoding: Q88Encoding,
    readout_encoding: Q88Encoding,
    range_policy: RangePolicy,
    /// Layout tag written into [`FpgaMetadata::version`]. Empty by default
    /// so the checked path does not inherit `Spikenaut-v2`.
    format_version: String,
    /// Timestamp recorded by the checked path. Defaults to omit.
    timestamp: TimestampPolicy,
}

/// Which parameter bank a validation error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterBlock {
    /// Per-neuron fire thresholds (`N` values).
    Thresholds,
    /// Hidden-layer weight matrix (`N` rows × `M` inputs).
    Weights,
    /// Per-neuron decay rates (`N` values).
    DecayRates,
    /// Optional readout / output-layer weights (`K` rows × `N` hidden neurons).
    Readout,
}

impl fmt::Display for ParameterBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thresholds => write!(f, "thresholds"),
            Self::Weights => write!(f, "weights"),
            Self::DecayRates => write!(f, "decay_rates"),
            Self::Readout => write!(f, "output_weights"),
        }
    }
}

/// Location of a scalar inside a parameter block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterLocation {
    /// Parameter bank that contains the value.
    pub block: ParameterBlock,
    /// Neuron index, or matrix row.
    pub neuron: usize,
    /// Channel / column for a matrix; `None` for a per-neuron vector.
    pub channel: Option<usize>,
}

impl fmt::Display for ParameterLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.channel {
            Some(channel) => write!(f, "{}[{}, {}]", self.block, self.neuron, channel),
            None => write!(f, "{}[{}]", self.block, self.neuron),
        }
    }
}

/// Why a value is not a finite `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonFiniteKind {
    /// `f32::NAN` (any payload).
    Nan,
    /// `+∞`.
    PosInfinity,
    /// `-∞`.
    NegInfinity,
}

impl fmt::Display for NonFiniteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nan => write!(f, "NaN"),
            Self::PosInfinity => write!(f, "+inf"),
            Self::NegInfinity => write!(f, "-inf"),
        }
    }
}

/// Q8.8 numeric interpretation used for representability checks and encoding.
///
/// Selected **per [`ParameterBlock`]** with
/// [`FpgaParameterExporter::set_encoding`], not as an ambiguous global meaning
/// of `u16` / `i16`. The choice is recorded on [`FpgaMetadata::encodings`] so a
/// consuming profile or `parameters.json` reader does not infer signedness
/// from a filename or Rust storage type.
///
/// Hardware `.mem` images require [`Q88Encoding::Signed`]
/// ([`ExportError::UnsignedHardwareEncoding`]). Unsigned remains available for
/// in-memory consumers of thresholds/decay (and any other block) that still
/// want the `0..=255.99609375` magnitude range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Q88Encoding {
    /// Full-range signed two's-complement Q8.8, matching
    /// [`encode_q88_signed_full`].
    ///
    /// Representable without saturation:
    /// [`Q88_SIGNED_MIN`]`..=`[`Q88_SIGNED_MAX`] (`-128..=127.99609375`).
    /// Distinct from the UART helper [`encode_q88_signed`], which clamps at
    /// ±127.99.
    #[default]
    Signed,
    /// Unsigned-magnitude Q8.8, matching [`encode_q88_unsigned`].
    ///
    /// Representable without saturation: `0.0..=`[`Q88_UNSIGNED_MAX`].
    ///
    /// Encoded words are stored in [`FpgaParameters`] as `i16` via `u16 as
    /// i16` so the 16-bit pattern is preserved (`200.0` → `0xC800`, which is
    /// `-14336` as a signed integer). silicon-hdl RAM interprets every `.mem`
    /// word as signed two's-complement, so that pattern is `-56.0`, not
    /// `200.0`. [`MemFileWriter::write_mem_files`] therefore refuses this
    /// variant ([`ExportError::UnsignedHardwareEncoding`]).
    Unsigned,
}

impl Q88Encoding {
    /// Inclusive bounds the checked path treats as representable for `self`.
    pub fn representable_range(self) -> (f32, f32) {
        match self {
            Self::Signed => (Q88_SIGNED_MIN, Q88_SIGNED_MAX),
            Self::Unsigned => (0.0, Q88_UNSIGNED_MAX),
        }
    }
}

impl fmt::Display for Q88Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signed => write!(f, "signed Q8.8"),
            Self::Unsigned => write!(f, "unsigned Q8.8"),
        }
    }
}

/// Per-block Q8.8 signedness recorded in the export manifest.
///
/// This is the contract a consuming profile reads. A raw `i16` / `u16`
/// container does not itself imply signed or unsigned numerical semantics.
///
/// [`Default`] (and `#[serde(default)]` on [`FpgaMetadata::encodings`]) is
/// all-signed thresholds/weights/decay with `output_weights: None`. `None`
/// means **no readout matrix**, not unsigned. Deserializing a legacy
/// `parameters.json` that has top-level `output_weights` but no encoding
/// metadata records `Some(Q88Encoding::Signed)` so the manifest matches the
/// data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEncodings {
    /// Encoding for the threshold vector.
    pub thresholds: Q88Encoding,
    /// Encoding for the hidden weight matrix.
    pub weights: Q88Encoding,
    /// Encoding for the decay-rate vector.
    pub decay_rates: Q88Encoding,
    /// Encoding for the optional readout matrix.
    ///
    /// `None` when no readout was exported. Omitted from JSON when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_weights: Option<Q88Encoding>,
}

impl Default for BlockEncodings {
    /// All-signed hidden blocks, no readout (`output_weights: None`).
    fn default() -> Self {
        Self {
            thresholds: Q88Encoding::Signed,
            weights: Q88Encoding::Signed,
            decay_rates: Q88Encoding::Signed,
            output_weights: None,
        }
    }
}

impl BlockEncodings {
    /// Encoding recorded for `block`.
    ///
    /// Readout is [`Q88Encoding::Signed`] when the matrix was absent, matching
    /// the exporter default rather than inventing an unsigned interpretation.
    pub fn for_block(self, block: ParameterBlock) -> Q88Encoding {
        match block {
            ParameterBlock::Thresholds => self.thresholds,
            ParameterBlock::Weights => self.weights,
            ParameterBlock::DecayRates => self.decay_rates,
            ParameterBlock::Readout => self.output_weights.unwrap_or(Q88Encoding::Signed),
        }
    }
}

/// What the checked path does with a finite value outside the encoding range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RangePolicy {
    /// Reject the bundle. This is the default; silent saturation is not a
    /// safe FPGA-image default.
    #[default]
    Reject,
    /// Clamp to the encoding bounds and record every clamp in a
    /// [`SaturationReport`]. Non-finite inputs are still errors.
    ///
    /// Only [`FpgaParameterExporter::try_export_with_report`] applies this
    /// policy. [`CheckedParameterExport::try_export`] and
    /// [`MemFileWriter::write_mem_files`] return
    /// [`ParameterShapeError::SaturationRequiresReport`] so the clamp list
    /// cannot be silently dropped.
    Saturate,
}

/// Failure from [`FpgaParameterExporter::set_timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataTimestampError {
    /// The string is not RFC 3339.
    InvalidRfc3339,
    /// RFC 3339 parsed, but the offset is not UTC (`Z` or `±00:00`).
    NotUtc,
}

impl fmt::Display for MetadataTimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRfc3339 => write!(f, "timestamp is not RFC 3339"),
            Self::NotUtc => write!(
                f,
                "timestamp must be UTC (RFC 3339 with Z or a zero offset)"
            ),
        }
    }
}

impl std::error::Error for MetadataTimestampError {}

/// One value that [`RangePolicy::Saturate`] clamped before encoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaturationEvent {
    /// Where the original value sat in the bundle.
    pub location: ParameterLocation,
    /// Caller-supplied float, before clamping.
    pub original: f32,
    /// Value actually encoded, after clamping to the encoding bounds.
    pub saturated_to: f32,
    /// Encoding whose bounds were applied.
    pub encoding: Q88Encoding,
}

/// Observable record of every clamp performed under [`RangePolicy::Saturate`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaturationReport {
    /// Clamped values, in scan order (thresholds, then weights row-major,
    /// then decay, then readout).
    pub events: Vec<SaturationEvent>,
}

impl SaturationReport {
    /// Whether any value was clamped.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Number of clamped values.
    pub fn len(&self) -> usize {
        self.events.len()
    }
}

/// A parameter set that cannot be exported as a valid FPGA image.
///
/// Returned by [`FpgaParameterExporter::validate`] and
/// [`CheckedParameterExport::try_export`]. [`MemFileWriter::write_mem_files`]
/// wraps it in [`ExportError::InvalidParameters`].
///
/// Extended in place from the original rectangularity-only type (PR #41 /
/// `RaggedWeights`). New variants cover empty dimensions, neuron-count
/// mismatches, optional readout shape, non-finite values, representability,
/// and report-less saturation. The enum stays `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum ParameterShapeError {
    /// No neurons: thresholds, decay, and hidden weights are all empty.
    EmptyLayer,
    /// A required block is empty (zero length, or a weight matrix with `M = 0`).
    EmptyBlock {
        /// Which block was empty.
        block: ParameterBlock,
    },
    /// The weight rows disagree in length, so the flattened buffer and
    /// `FpgaMetadata::num_channels` describe different matrices.
    RaggedWeights {
        /// Width taken from the first row, and reported as `num_channels`.
        expected: usize,
        /// Index of the first row that does not have that width.
        row: usize,
        /// Length of that row.
        len: usize,
    },
    /// Thresholds, decay, or hidden-weight *row count* disagrees with `N`.
    ///
    /// `N` is the threshold count. Hidden weights must have `N` rows; decay
    /// must have `N` values. There is no hard-coded neuron count.
    DimensionMismatch {
        /// Block whose length disagrees with `N`.
        block: ParameterBlock,
        /// Expected length (`N`, from the threshold vector).
        expected: usize,
        /// Length actually supplied.
        actual: usize,
    },
    /// Optional readout rows disagree in length.
    RaggedReadout {
        /// Width taken from the first readout row (must equal `N`).
        expected: usize,
        /// Index of the first row that does not have that width.
        row: usize,
        /// Length of that row.
        len: usize,
    },
    /// Optional readout is present but is not `K` rows by `N` hidden columns.
    ReadoutShape {
        /// Hidden-neuron count `N`, required as the column count.
        expected_cols: usize,
        /// Column count of the (rectangular) readout matrix.
        actual_cols: usize,
        /// Number of readout rows `K`.
        rows: usize,
    },
    /// `NaN` or infinities are rejected before quantization on the checked path.
    NonFinite {
        /// Block, row, and column of the offending value.
        location: ParameterLocation,
        /// Whether the value was NaN, `+inf`, or `-inf`.
        kind: NonFiniteKind,
    },
    /// Finite value outside the selected encoding's representable range.
    OutOfRange {
        /// Block, row, and column of the offending value.
        location: ParameterLocation,
        /// The finite input that would saturate.
        value: f32,
        /// Encoding whose bounds were applied.
        encoding: Q88Encoding,
        /// Inclusive lower bound of that encoding.
        min: f32,
        /// Inclusive upper bound of that encoding.
        max: f32,
    },
    /// [`RangePolicy::Saturate`] was selected on a report-less export path.
    ///
    /// [`CheckedParameterExport::try_export`] and
    /// [`MemFileWriter::write_mem_files`] would otherwise encode the clamped
    /// values and drop the [`SaturationReport`]. Call
    /// [`FpgaParameterExporter::try_export_with_report`] instead.
    SaturationRequiresReport,
}

impl fmt::Display for ParameterShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLayer => write!(
                f,
                "parameter layer is empty: need N>0 thresholds, N decay rates, \
                 and an N×M (M>0) weight matrix"
            ),
            Self::EmptyBlock { block } => {
                write!(
                    f,
                    "{block} is empty; a dense layer cannot have a zero dimension"
                )
            }
            Self::RaggedWeights { expected, row, len } => write!(
                f,
                "weight matrix is not rectangular: row 0 has {expected} channels \
                 but row {row} has {len}; a flattened .mem file cannot describe it"
            ),
            Self::DimensionMismatch {
                block,
                expected,
                actual,
            } => write!(
                f,
                "{block} length is {actual}, expected {expected} (the neuron count N \
                 taken from thresholds)"
            ),
            Self::RaggedReadout { expected, row, len } => write!(
                f,
                "readout matrix is not rectangular: row 0 has {expected} columns \
                 but row {row} has {len}"
            ),
            Self::ReadoutShape {
                expected_cols,
                actual_cols,
                rows,
            } => write!(
                f,
                "readout is {rows}×{actual_cols}, expected K×{expected_cols} \
                 (K output rows by N hidden-neuron columns)"
            ),
            Self::NonFinite { location, kind } => {
                write!(
                    f,
                    "{location} is {kind}; non-finite values cannot be quantized"
                )
            }
            Self::OutOfRange {
                location,
                value,
                encoding,
                min,
                max,
            } => write!(
                f,
                "{location} value {value} is outside {encoding} range {min}..={max}"
            ),
            Self::SaturationRequiresReport => write!(
                f,
                "RangePolicy::Saturate cannot be used with try_export because \
                 saturation events would be discarded; call try_export_with_report \
                 instead"
            ),
        }
    }
}

impl std::error::Error for ParameterShapeError {}

/// Failure from the checked `.mem` writer.
///
/// Validation failures are [`ExportError::InvalidParameters`]. Unsigned
/// encoding on the hardware path is [`ExportError::UnsignedHardwareEncoding`].
/// I/O and JSON failures preserve their underlying cause via
/// [`std::error::Error::source`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ExportError {
    /// The parameter bundle failed [`FpgaParameterExporter::validate`] /
    /// [`CheckedParameterExport::try_export`].
    InvalidParameters(ParameterShapeError),
    /// Directory creation or file write failed.
    Io {
        /// Path that was being created or written.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// `parameters.json` could not be serialized.
    Serialize {
        /// Path that would have received the JSON.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// A block was configured as [`Q88Encoding::Unsigned`] on the hardware
    /// `.mem` writer.
    ///
    /// silicon-hdl RAM is signed two's-complement Q8.8. An unsigned word
    /// above 127.996 is stored as an `i16` bit pattern that the FPGA reads
    /// as a negative (for example unsigned `200.0` → `C800` → `-56.0`).
    UnsignedHardwareEncoding {
        /// Block configured as unsigned-magnitude Q8.8.
        block: ParameterBlock,
    },
    /// A configured output filename is not a safe relative basename.
    UnsafeFilename {
        /// Rejected name.
        name: String,
        /// Why the name was rejected.
        reason: FilenameError,
    },
    /// A custom export contract was built with a blank profile or schema id.
    EmptyContractIdentifier {
        /// Identifier field that must be non-empty.
        field: &'static str,
    },
    /// Two blocks were assigned the same output filename.
    DuplicateFilename {
        /// Repeated basename.
        name: String,
    },
    /// A pinned compatibility profile refused caller-supplied filenames.
    ImmutableFileLayout {
        /// Profile whose filenames are part of its compatibility contract.
        profile: String,
    },
    /// The generic / signed-output path refused to replace an existing file.
    OverwriteRefused {
        /// Path that already exists.
        path: PathBuf,
    },
    /// A matrix flattening other than dense row-major was requested.
    UnsupportedFlattening {
        /// Requested layout name.
        requested: String,
    },
    /// A profile that requires a `K×N` readout was used without one.
    MissingRequiredReadout {
        /// Profile that requires the readout matrix.
        profile: String,
    },
    /// A readout matrix is present but [`ExportFileLayout::output_weights`] is
    /// `None`, so JSON would record the block without a matching `.mem` file.
    MissingReadoutFilename,
    /// [`ExportConfig::with_declared_target_latency_us`] was given NaN or inf.
    NonFiniteDeclaredLatency,
    /// A pinned compatibility profile was selected for unsupported dimensions.
    UnsupportedCompatibilityShape {
        /// Profile that rejected the shape.
        profile: String,
        /// Dimensions supported by that profile.
        expected: CompatibilityDimensions,
        /// Dimensions supplied by the export.
        actual: CompatibilityDimensions,
    },
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParameters(err) => write!(f, "{err}"),
            Self::Io { path, source } => {
                write!(f, "I/O error writing {}: {source}", path.display())
            }
            Self::Serialize { path, source } => {
                write!(f, "failed to serialize {}: {source}", path.display())
            }
            Self::UnsignedHardwareEncoding { block } => write!(
                f,
                "hardware .mem export requires signed Q8.8; {block} is unsigned \
                 (values above 127.996 would be interpreted as negatives by \
                 silicon-hdl RAM)"
            ),
            Self::UnsafeFilename { name, reason } => {
                write!(f, "unsafe output filename {name:?}: {reason}")
            }
            Self::DuplicateFilename { name } => {
                write!(f, "output filename {name:?} is used by more than one block")
            }
            Self::EmptyContractIdentifier { field } => {
                write!(f, "custom export contract {field} must not be empty")
            }
            Self::ImmutableFileLayout { profile } => write!(
                f,
                "profile {profile} has a fixed file layout; choose a generic profile to customize filenames"
            ),
            Self::OverwriteRefused { path } => write!(
                f,
                "refusing to replace existing file {} without ExportConfig::allow_replace",
                path.display()
            ),
            Self::UnsupportedFlattening { requested } => write!(
                f,
                "unsupported matrix flattening {requested:?}; only dense row-major \
                 (`row_major` / `row_major_dense`) is implemented"
            ),
            Self::MissingRequiredReadout { profile } => {
                write!(f, "profile {profile} requires a K×N readout matrix")
            }
            Self::MissingReadoutFilename => write!(
                f,
                "readout matrix is present but ExportFileLayout::output_weights is None; \
                 refusing to write JSON without a matching .mem file"
            ),
            Self::NonFiniteDeclaredLatency => write!(
                f,
                "declared target_latency_us must be finite (not NaN or inf)"
            ),
            Self::UnsupportedCompatibilityShape {
                profile,
                expected,
                actual,
            } => write!(
                f,
                "profile {profile} supports {} inputs, {} hidden neurons, and {} outputs; \
                 got {} inputs, {} hidden neurons, and {} outputs",
                expected.input_channels,
                expected.hidden_neurons,
                expected.output_classes,
                actual.input_channels,
                actual.hidden_neurons,
                actual.output_classes
            ),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidParameters(err) => Some(err),
            Self::Io { source, .. } => Some(source),
            Self::Serialize { source, .. } => Some(source),
            Self::UnsignedHardwareEncoding { .. }
            | Self::UnsafeFilename { .. }
            | Self::DuplicateFilename { .. }
            | Self::EmptyContractIdentifier { .. }
            | Self::ImmutableFileLayout { .. }
            | Self::OverwriteRefused { .. }
            | Self::UnsupportedFlattening { .. }
            | Self::MissingRequiredReadout { .. }
            | Self::MissingReadoutFilename
            | Self::NonFiniteDeclaredLatency
            | Self::UnsupportedCompatibilityShape { .. } => None,
        }
    }
}

impl From<ParameterShapeError> for ExportError {
    fn from(err: ParameterShapeError) -> Self {
        Self::InvalidParameters(err)
    }
}

/// FPGA-compatible parameter format.
///
/// Vectors hold raw 16-bit Q8.8 words stored as `i16`. Hardware `.mem` images
/// are signed two's-complement. An in-memory [`Q88Encoding::Unsigned`] encode
/// stores the unsigned bit pattern via `as i16`; values above 127.996
/// therefore appear negative if interpreted as signed.
/// [`MemFileWriter::write_mem_files`] refuses that encoding so FPGA RAM never
/// sees the reinterpretation.
#[derive(Debug, Clone, Serialize)]
pub struct FpgaParameters {
    /// Neuron thresholds as Q8.8 raw words (`i16` bit patterns).
    pub thresholds: Vec<i16>,
    /// Weight matrix [neurons x channels] as Q8.8 raw words (`i16` bit patterns).
    pub weights: Vec<i16>,
    /// Decay rates as Q8.8 raw words (`i16` bit patterns).
    pub decay_rates: Vec<i16>,
    /// Optional readout / output-layer weights, flattened row-major as `K×N`.
    ///
    /// `None` when the exporter has no readout block. Omitted from JSON when
    /// absent so existing `parameters.json` fixtures keep loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_weights: Option<Vec<i16>>,
    /// Metadata about the parameter set
    pub metadata: FpgaMetadata,
}

/// Layout and provenance of an exported parameter bundle.
///
/// Serialized alongside the Q8.8 vectors as `parameters.json`, so a `.mem` set
/// on disk can be matched back to the shape it was generated for.
///
/// [`Default`] is empty provenance strings, zero counts, and
/// [`BlockEncodings::default`] (all signed, no readout). Downstream struct
/// literals can keep compiling after additive fields by writing
/// `..Default::default()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FpgaMetadata {
    /// Historical layout tag. Empty on the default checked path. The
    /// legacy [`ParameterExport::export`] wrapper and
    /// [`MemFileWriter::write_mem_files`] still stamp
    /// [`EXPORT_FORMAT_VERSION`] (`Spikenaut-v2`). Omitted from generic /
    /// signed-output JSON (empty string). Distinct from
    /// [`Self::schema_version`], [`Self::producer_version`], and
    /// [`Self::profile`].
    ///
    /// Not a validated invariant of the type itself: `FpgaMetadata` is public
    /// and `Deserialize`, so a value built by hand or read from an
    /// externally-supplied `parameters.json` can carry any string here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Export timestamp. Empty when omitted (checked / generic default).
    /// Legacy `ParameterExport::export` and `write_mem_files` use
    /// `chrono::Utc::now()`; the generic path copies a caller-supplied
    /// string or omits the field so repeats stay byte-identical.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timestamp: String,
    /// Number of neurons, taken from the threshold count.
    ///
    /// Not an invariant of this public struct: [`ParameterExport::export`]
    /// still copies the threshold count without cross-checking the weight
    /// rows. The checked path ([`CheckedParameterExport::try_export`]) requires
    /// `N` thresholds, `N` decay rates, and `N` weight rows.
    pub num_neurons: usize,
    /// Weight-matrix width, taken from the first weight row (`0` when there
    /// are no weights).
    ///
    /// Together with `num_neurons` this describes how `FpgaParameters::weights`
    /// is addressed: `row * num_channels + channel`.
    ///
    /// That addressing holds only for a rectangular weight matrix. Rows of
    /// unequal length are flattened unchanged, so the pair describes a matrix
    /// the buffer does not contain, and indexing misreads or overruns from the
    /// first short row onward.
    pub num_channels: usize,
    /// Declared per-tick latency target in microseconds, when present.
    ///
    /// Never a measured latency. The generic profile omits this unless the
    /// caller supplies a declared target. Legacy Spikenaut-v2 writes record
    /// [`SPIKENAUT_LEGACY_TARGET_LATENCY_US`] (`35.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_latency_us: Option<f32>,
    /// Total size of the three Q8.8 vectors in kibibytes, at 2 bytes per
    /// parameter.
    ///
    /// Counts parameters only — it excludes `$readmemh` ASCII overhead and any
    /// padding the target RAM applies.
    pub memory_usage_kb: f32,
    /// Per-block signed/unsigned interpretation used to produce the Q8.8
    /// words in this bundle.
    ///
    /// Signedness is never inferred from a filename or from storing words as
    /// `i16`. Older `parameters.json` without this field deserializes as
    /// [`BlockEncodings::default`] (all signed, no readout). A legacy file
    /// that includes top-level `output_weights` is patched on deserialize to
    /// `encodings.output_weights = Some(Signed)`.
    #[serde(default)]
    pub encodings: BlockEncodings,
    /// Profile identity (`generic-dense-q88`, `spikenaut-v2-legacy`,
    /// `spikenaut-signed-output-v1`). Distinct from schema and crate version.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub profile: String,
    /// Schema of this metadata document. Distinct from [`Self::profile`] and
    /// [`Self::producer_version`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema_version: String,
    /// Crate that produced the bundle (`silicon-bridge`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub producer_crate: String,
    /// `CARGO_PKG_VERSION` of the producer crate.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub producer_version: String,
    /// Caller-supplied model identity. Distinct from profile and schema.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_id: String,
    /// Matrix flattening. Only [`MatrixFlattening::RowMajorDense`] is written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flattening: Option<MatrixFlattening>,
    /// Quantization rounding used to produce the stored words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounding: Option<RoundingMode>,
    /// Overflow policy used by the checked export that produced this bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow_policy: Option<OverflowPolicy>,
    /// Total and fractional bit widths of each stored word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q_format: Option<QFormat>,
    /// Readout matrix shape `K×N` when a readout block is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout_shape: Option<ReadoutShape>,
    /// Per-block dimensions, signedness, and bit widths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<BundleShapes>,
    /// Optional machine-readable profile compatibility contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<CompatibilityMetadata>,
}

impl<'de> Deserialize<'de> for FpgaParameters {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            thresholds: Vec<i16>,
            weights: Vec<i16>,
            decay_rates: Vec<i16>,
            #[serde(default)]
            output_weights: Option<Vec<i16>>,
            metadata: FpgaMetadata,
        }

        let mut raw = Raw::deserialize(deserializer)?;
        // `BlockEncodings::default().output_weights` is None ("no readout").
        // Legacy files can carry the matrix without encoding metadata; record
        // signed so the manifest matches the data and reserializes correctly.
        if raw.output_weights.is_some() && raw.metadata.encodings.output_weights.is_none() {
            raw.metadata.encodings.output_weights = Some(Q88Encoding::Signed);
        }
        Ok(Self {
            thresholds: raw.thresholds,
            weights: raw.weights,
            decay_rates: raw.decay_rates,
            output_weights: raw.output_weights,
            metadata: raw.metadata,
        })
    }
}

impl FpgaParameterExporter {
    /// Create new exporter with default parameters
    pub fn new() -> Self {
        Self {
            thresholds: Vec::new(),
            weights: Vec::new(),
            decay_rates: Vec::new(),
            output_weights: None,
            threshold_encoding: Q88Encoding::Signed,
            weight_encoding: Q88Encoding::Signed,
            decay_encoding: Q88Encoding::Signed,
            readout_encoding: Q88Encoding::Signed,
            range_policy: RangePolicy::Reject,
            format_version: String::new(),
            timestamp: TimestampPolicy::Omit,
        }
    }

    /// Set neuron thresholds
    pub fn set_thresholds(&mut self, thresholds: Vec<f32>) {
        self.thresholds = thresholds;
    }

    /// Set weight matrix [neurons x channels]
    pub fn set_weights(&mut self, weights: Vec<Vec<f32>>) {
        self.weights = weights;
    }

    /// Set decay rates
    pub fn set_decay_rates(&mut self, decay_rates: Vec<f32>) {
        self.decay_rates = decay_rates;
    }

    /// Set the optional readout / output-weight matrix `[K outputs × N hidden]`.
    ///
    /// Absence is valid: call [`Self::clear_output_weights`] or never set
    /// this field. When present, the checked path requires a rectangular
    /// `K×N` matrix, not `N` rows by assumption.
    pub fn set_output_weights(&mut self, output_weights: Vec<Vec<f32>>) {
        self.output_weights = Some(output_weights);
    }

    /// Drop the optional readout block. Models without an output layer stay valid.
    pub fn clear_output_weights(&mut self) {
        self.output_weights = None;
    }

    /// Select signed or unsigned encoding for `block`.
    ///
    /// Defaults are all [`Q88Encoding::Signed`] — the silicon-hdl `.mem`
    /// convention, and the only encoding [`MemFileWriter::write_mem_files`]
    /// will write. Thresholds and decay **may** be switched to
    /// [`Q88Encoding::Unsigned`] for in-memory consumers that still want the
    /// `0..=255.99609375` magnitude range; hidden and readout weights support
    /// signed encoding (the default) so inhibitory values survive.
    ///
    /// Changing a block to unsigned changes the checked-path range and
    /// in-memory encoder. [`MemFileWriter::write_mem_files`] still requires
    /// signed encoding and returns [`ExportError::UnsignedHardwareEncoding`].
    /// It does not change [`ParameterExport::export`], which always uses
    /// [`encode_q88_signed_full`].
    pub fn set_encoding(&mut self, block: ParameterBlock, encoding: Q88Encoding) {
        match block {
            ParameterBlock::Thresholds => self.threshold_encoding = encoding,
            ParameterBlock::Weights => self.weight_encoding = encoding,
            ParameterBlock::DecayRates => self.decay_encoding = encoding,
            ParameterBlock::Readout => self.readout_encoding = encoding,
        }
    }

    /// Encoding currently selected for `block`.
    ///
    /// This is the explicit interpretation a consuming profile should read,
    /// not a property of the `i16` storage type.
    pub fn encoding(&self, block: ParameterBlock) -> Q88Encoding {
        match block {
            ParameterBlock::Thresholds => self.threshold_encoding,
            ParameterBlock::Weights => self.weight_encoding,
            ParameterBlock::DecayRates => self.decay_encoding,
            ParameterBlock::Readout => self.readout_encoding,
        }
    }

    /// Choose whether out-of-range values are rejected or clamped with a report.
    ///
    /// The default is [`RangePolicy::Reject`]. [`RangePolicy::Saturate`] is
    /// applied only by [`Self::try_export_with_report`].
    /// [`CheckedParameterExport::try_export`] and
    /// [`MemFileWriter::write_mem_files`] return
    /// [`ParameterShapeError::SaturationRequiresReport`] so a
    /// [`SaturationReport`] cannot be dropped. The legacy infallible
    /// [`ParameterExport::export`] still saturates without a report.
    pub fn set_range_policy(&mut self, policy: RangePolicy) {
        self.range_policy = policy;
    }

    /// Set the layout tag written into [`FpgaMetadata::version`].
    ///
    /// The default is empty: the checked path does **not** inherit
    /// [`EXPORT_FORMAT_VERSION`] (`Spikenaut-v2`). That string is a
    /// historical layout identifier, not a second on-disk format. Pass it
    /// only for Spikenaut compatibility. Generic consumers typically leave
    /// this unset; disk writes use [`ExportConfig`] profile / schema fields
    /// instead.
    pub fn set_format_version(&mut self, version: impl Into<String>) {
        self.format_version = version.into();
    }

    /// Set the timestamp written into [`FpgaMetadata::timestamp`].
    ///
    /// `timestamp` must be RFC 3339 with a UTC offset (`Z` or `±00:00`).
    /// The caller’s spelling is stored unchanged so repeated exports stay
    /// byte-identical. Non-UTC offsets and unparsable strings return
    /// [`MetadataTimestampError`]. The default is to **omit** the timestamp.
    /// Call [`Self::use_wall_clock_timestamp`] to record `chrono::Utc::now`
    /// at export time (legacy behaviour).
    pub fn set_timestamp(
        &mut self,
        timestamp: impl AsRef<str>,
    ) -> Result<(), MetadataTimestampError> {
        let timestamp = timestamp.as_ref();
        let parsed = chrono::DateTime::parse_from_rfc3339(timestamp)
            .map_err(|_| MetadataTimestampError::InvalidRfc3339)?;
        if parsed.offset().local_minus_utc() != 0 {
            return Err(MetadataTimestampError::NotUtc);
        }
        self.timestamp = TimestampPolicy::Supplied(timestamp.to_owned());
        Ok(())
    }

    /// Record a wall-clock timestamp (`chrono::Utc::now` at export time).
    ///
    /// This is opt-in. The default checked path omits timestamps so repeats
    /// stay byte-identical.
    pub fn use_wall_clock_timestamp(&mut self) {
        self.timestamp = TimestampPolicy::Now;
    }

    fn metadata_version(&self) -> String {
        self.format_version.clone()
    }

    fn metadata_timestamp(&self) -> String {
        match &self.timestamp {
            TimestampPolicy::Omit => String::new(),
            TimestampPolicy::Supplied(value) => value.clone(),
            TimestampPolicy::Now => chrono::Utc::now().to_rfc3339(),
        }
    }

    fn block_encodings(&self, include_readout: bool) -> BlockEncodings {
        BlockEncodings {
            thresholds: self.threshold_encoding,
            weights: self.weight_encoding,
            decay_rates: self.decay_encoding,
            output_weights: include_readout.then_some(self.readout_encoding),
        }
    }

    /// Convert `f32` to signed Q8.8 fixed-point format.
    ///
    /// Prefer [`FixedPointEncode::encode_q88`] when coding against the trait.
    pub fn to_q88(&self, value: f32) -> i16 {
        self.encode_q88(value)
    }

    /// Export parameters to an in-memory Q8.8 bundle.
    ///
    /// Prefer [`CheckedParameterExport::try_export`] for an FPGA image and
    /// [`Self::write_generic`] for the default disk path. This inherent
    /// wrapper is the documented **legacy** flatten-and-saturate path
    /// (same as [`ParameterExport::export`]).
    pub fn export(&self) -> FpgaParameters {
        ParameterExport::export(self)
    }

    /// Export parameters to `.mem` files for silicon-hdl / Vivado `$readmemh`.
    ///
    /// This is an alias of [`MemFileWriter::write_mem_files`]: the checked
    /// **legacy Spikenaut-v2** writer. Prefer [`Self::write_generic`] for new
    /// callers. This path uses [`ExportConfig::legacy_spikenaut_v2`], replaces
    /// existing files, and does not create or truncate files if validation
    /// fails.
    pub fn export_to_mem_files<P: AsRef<Path>>(
        &self,
        output_dir: P,
    ) -> Result<ExportReport, ExportError> {
        self.write_mem_files(output_dir)
    }

    /// Create an exporter pre-populated with given parameters.
    pub fn from_params(
        thresholds: Vec<f32>,
        weights: Vec<Vec<f32>>,
        decay_rates: Vec<f32>,
    ) -> Self {
        Self {
            thresholds,
            weights,
            decay_rates,
            output_weights: None,
            threshold_encoding: Q88Encoding::Signed,
            weight_encoding: Q88Encoding::Signed,
            decay_encoding: Q88Encoding::Signed,
            readout_encoding: Q88Encoding::Signed,
            range_policy: RangePolicy::Reject,
            format_version: String::new(),
            timestamp: TimestampPolicy::Omit,
        }
    }

    /// Validate the complete dense-layer bundle for the checked export path.
    ///
    /// For an `N`-neuron, `M`-input layer this requires `N` thresholds, `N`
    /// decay rates, and an `N×M` rectangular weight matrix with `N > 0` and
    /// `M > 0`. Non-square layers (`N ≠ M`) are valid; nothing here is
    /// hard-coded to 16.
    ///
    /// If a readout block is set, it is checked separately as `K×N`. Absence
    /// of a readout is valid. `NaN` and infinities are rejected before
    /// quantization. Finite values outside the block's [`Q88Encoding`] range
    /// are rejected (saturation is a [`RangePolicy`] on
    /// [`Self::try_export_with_report`], not a silent `validate` success, and
    /// not a report-less [`Self::try_export`]).
    ///
    /// [`ParameterExport::export`] stays infallible and still flattens a
    /// ragged matrix — this method is the gate for a hardware image.
    ///
    /// ```rust
    /// use silicon_bridge::{FpgaParameterExporter, ParameterShapeError};
    ///
    /// let square = FpgaParameterExporter::from_params(
    ///     vec![1.0, 1.0],
    ///     vec![vec![0.5, 0.5], vec![0.5, 0.5]],
    ///     vec![0.9, 0.9],
    /// );
    /// assert!(square.validate().is_ok());
    ///
    /// let ragged = FpgaParameterExporter::from_params(
    ///     vec![1.0, 1.0],
    ///     vec![vec![0.5, 0.5], vec![0.5]],
    ///     vec![0.9, 0.9],
    /// );
    /// assert_eq!(
    ///     ragged.validate(),
    ///     Err(ParameterShapeError::RaggedWeights {
    ///         expected: 2,
    ///         row: 1,
    ///         len: 1,
    ///     })
    /// );
    /// ```
    pub fn validate(&self) -> Result<(), ParameterShapeError> {
        self.validate_shape()?;
        self.scan_numeric(true, &mut SaturationReport::default())?;
        Ok(())
    }

    /// Validate and encode, returning every value [`RangePolicy::Saturate`] clamped.
    ///
    /// With the default [`RangePolicy::Reject`], the report is empty on
    /// success. Non-finite inputs are always errors, including under
    /// saturation. This is the only checked path that applies
    /// [`RangePolicy::Saturate`]; [`Self::try_export`] rejects that policy.
    pub fn try_export_with_report(
        &self,
    ) -> Result<(FpgaParameters, SaturationReport), ParameterShapeError> {
        self.validate_shape()?;
        let check_range = matches!(self.range_policy, RangePolicy::Reject);
        let mut report = SaturationReport::default();
        let params = self.encode_checked(check_range, &mut report)?;
        Ok((params, report))
    }

    /// Validate the complete bundle and encode it.
    ///
    /// This is the checked entry point. Prefer it over [`Self::export`] /
    /// [`ParameterExport::export`] when producing an FPGA image. The infallible
    /// methods remain for callers that need the historical flatten-and-clamp
    /// behaviour; they are not a safe default.
    ///
    /// Metadata defaults match **generic-dense-q88**: no Spikenaut layout tag,
    /// no timestamp, no declared latency. Disk writes should use
    /// [`Self::write_generic`]. [`MemFileWriter::write_mem_files`] is the
    /// explicit Spikenaut-v2 compatibility writer.
    ///
    /// Out-of-range values are rejected under the default
    /// [`RangePolicy::Reject`]. [`RangePolicy::Saturate`] returns
    /// [`ParameterShapeError::SaturationRequiresReport`] so the clamp list
    /// cannot be dropped; use [`Self::try_export_with_report`].
    pub fn try_export(&self) -> Result<FpgaParameters, ParameterShapeError> {
        if matches!(self.range_policy, RangePolicy::Saturate) {
            return Err(ParameterShapeError::SaturationRequiresReport);
        }
        self.try_export_with_report().map(|(params, _)| params)
    }

    /// First block configured as [`Q88Encoding::Unsigned`] that would be
    /// written into a hardware `.mem` image.
    fn unsigned_hardware_block(&self) -> Option<ParameterBlock> {
        const REQUIRED: [ParameterBlock; 3] = [
            ParameterBlock::Thresholds,
            ParameterBlock::Weights,
            ParameterBlock::DecayRates,
        ];
        for block in REQUIRED {
            if self.encoding(block) == Q88Encoding::Unsigned {
                return Some(block);
            }
        }
        if self.output_weights.is_some() && self.readout_encoding == Q88Encoding::Unsigned {
            Some(ParameterBlock::Readout)
        } else {
            None
        }
    }

    fn validate_shape(&self) -> Result<(), ParameterShapeError> {
        let weight_width = matrix_width(&self.weights, MatrixKind::Hidden)?;
        let readout_width = match &self.output_weights {
            Some(matrix) => Some(matrix_width(matrix, MatrixKind::Readout)?),
            None => None,
        };

        let n_thr = self.thresholds.len();
        let n_dec = self.decay_rates.len();
        let n_wt = self.weights.len();

        if n_thr == 0 && n_dec == 0 && n_wt == 0 {
            return Err(ParameterShapeError::EmptyLayer);
        }

        if n_thr == 0 {
            return Err(ParameterShapeError::EmptyBlock {
                block: ParameterBlock::Thresholds,
            });
        }

        if n_dec != n_thr {
            return Err(ParameterShapeError::DimensionMismatch {
                block: ParameterBlock::DecayRates,
                expected: n_thr,
                actual: n_dec,
            });
        }

        if n_wt != n_thr {
            return Err(ParameterShapeError::DimensionMismatch {
                block: ParameterBlock::Weights,
                expected: n_thr,
                actual: n_wt,
            });
        }

        let m = weight_width.unwrap_or(0);
        if m == 0 {
            return Err(ParameterShapeError::EmptyBlock {
                block: ParameterBlock::Weights,
            });
        }

        if let Some(matrix) = &self.output_weights {
            if matrix.is_empty() {
                return Err(ParameterShapeError::EmptyBlock {
                    block: ParameterBlock::Readout,
                });
            }
            let cols = readout_width.flatten().unwrap_or(0);
            if cols != n_thr {
                return Err(ParameterShapeError::ReadoutShape {
                    expected_cols: n_thr,
                    actual_cols: cols,
                    rows: matrix.len(),
                });
            }
        }

        Ok(())
    }

    fn scan_numeric(
        &self,
        check_range: bool,
        report: &mut SaturationReport,
    ) -> Result<(), ParameterShapeError> {
        for (neuron, &value) in self.thresholds.iter().enumerate() {
            self.check_value(
                location(ParameterBlock::Thresholds, neuron, None),
                value,
                check_range,
                report,
            )?;
        }
        for (neuron, row) in self.weights.iter().enumerate() {
            for (channel, &value) in row.iter().enumerate() {
                self.check_value(
                    location(ParameterBlock::Weights, neuron, Some(channel)),
                    value,
                    check_range,
                    report,
                )?;
            }
        }
        for (neuron, &value) in self.decay_rates.iter().enumerate() {
            self.check_value(
                location(ParameterBlock::DecayRates, neuron, None),
                value,
                check_range,
                report,
            )?;
        }
        if let Some(matrix) = &self.output_weights {
            for (neuron, row) in matrix.iter().enumerate() {
                for (channel, &value) in row.iter().enumerate() {
                    self.check_value(
                        location(ParameterBlock::Readout, neuron, Some(channel)),
                        value,
                        check_range,
                        report,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn check_value(
        &self,
        loc: ParameterLocation,
        value: f32,
        check_range: bool,
        report: &mut SaturationReport,
    ) -> Result<f32, ParameterShapeError> {
        if let Some(kind) = non_finite_kind(value) {
            return Err(ParameterShapeError::NonFinite {
                location: loc,
                kind,
            });
        }

        let encoding = self.encoding(loc.block);
        let (min, max) = encoding.representable_range();
        if value < min || value > max {
            if check_range {
                return Err(ParameterShapeError::OutOfRange {
                    location: loc,
                    value,
                    encoding,
                    min,
                    max,
                });
            }
            let saturated_to = value.clamp(min, max);
            report.events.push(SaturationEvent {
                location: loc,
                original: value,
                saturated_to,
                encoding,
            });
            return Ok(saturated_to);
        }
        Ok(value)
    }

    fn encode_checked(
        &self,
        check_range: bool,
        report: &mut SaturationReport,
    ) -> Result<FpgaParameters, ParameterShapeError> {
        let thresholds = self.encode_vector(
            ParameterBlock::Thresholds,
            &self.thresholds,
            check_range,
            report,
        )?;
        let mut weights = Vec::with_capacity(self.weights.iter().map(|row| row.len()).sum());
        for (neuron, row) in self.weights.iter().enumerate() {
            for (channel, &value) in row.iter().enumerate() {
                let prepared = self.check_value(
                    location(ParameterBlock::Weights, neuron, Some(channel)),
                    value,
                    check_range,
                    report,
                )?;
                weights.push(encode_for(self.encoding(ParameterBlock::Weights), prepared));
            }
        }
        let decay_rates = self.encode_vector(
            ParameterBlock::DecayRates,
            &self.decay_rates,
            check_range,
            report,
        )?;
        let output_weights = match &self.output_weights {
            None => None,
            Some(matrix) => {
                let mut encoded = Vec::with_capacity(matrix.iter().map(|row| row.len()).sum());
                for (neuron, row) in matrix.iter().enumerate() {
                    for (channel, &value) in row.iter().enumerate() {
                        let prepared = self.check_value(
                            location(ParameterBlock::Readout, neuron, Some(channel)),
                            value,
                            check_range,
                            report,
                        )?;
                        encoded.push(encode_for(self.encoding(ParameterBlock::Readout), prepared));
                    }
                }
                Some(encoded)
            }
        };

        Ok(self.bundle(thresholds, weights, decay_rates, output_weights))
    }

    fn encode_vector(
        &self,
        block: ParameterBlock,
        values: &[f32],
        check_range: bool,
        report: &mut SaturationReport,
    ) -> Result<Vec<i16>, ParameterShapeError> {
        let mut encoded = Vec::with_capacity(values.len());
        for (neuron, &value) in values.iter().enumerate() {
            let prepared =
                self.check_value(location(block, neuron, None), value, check_range, report)?;
            encoded.push(encode_for(self.encoding(block), prepared));
        }
        Ok(encoded)
    }

    fn bundle(
        &self,
        thresholds: Vec<i16>,
        weights: Vec<i16>,
        decay_rates: Vec<i16>,
        output_weights: Option<Vec<i16>>,
    ) -> FpgaParameters {
        let metadata = FpgaMetadata {
            version: self.metadata_version(),
            timestamp: self.metadata_timestamp(),
            num_neurons: self.thresholds.len(),
            num_channels: if self.weights.is_empty() {
                0
            } else {
                self.weights[0].len()
            },
            target_latency_us: None,
            memory_usage_kb: self.calculate_memory_usage(),
            encodings: self.block_encodings(output_weights.is_some()),
            ..FpgaMetadata::default()
        };

        FpgaParameters {
            thresholds,
            weights,
            decay_rates,
            output_weights,
            metadata,
        }
    }

    fn calculate_memory_usage(&self) -> f32 {
        let readout = self
            .output_weights
            .as_ref()
            .map(|matrix| matrix.iter().map(|row| row.len()).sum::<usize>())
            .unwrap_or(0);
        let total_params = self.thresholds.len()
            + self.weights.iter().map(|row| row.len()).sum::<usize>()
            + self.decay_rates.len()
            + readout;

        // Each parameter is 2 bytes (i16) in Q8.8 format
        (total_params * 2) as f32 / 1024.0
    }

    /// Write one `$readmemh` image: the raw 16-bit two's-complement pattern of
    /// each word, uppercase, one `{:04X}` per line.
    fn write_mem_file(
        path: impl AsRef<Path>,
        values: &[i16],
        overwrite: OverwritePolicy,
    ) -> Result<(), ExportError> {
        let path = path.as_ref();
        let mut file = Self::create_output_file(path, overwrite)?;
        for value in values {
            // Rust formats a signed integer's hex as its two's-complement
            // pattern (no `-` prefix), so `-256` already prints as `FF00`. The
            // `as u16` cast is what pins the field to 16 bits regardless of the
            // element type, which is the width `$readmemh` expects.
            writeln!(file, "{:04X}", *value as u16).map_err(|source| ExportError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    fn write_json_file(
        path: impl AsRef<Path>,
        json: &str,
        overwrite: OverwritePolicy,
    ) -> Result<(), ExportError> {
        let path = path.as_ref();
        let mut file = Self::create_output_file(path, overwrite)?;
        file.write_all(json.as_bytes())
            .map_err(|source| ExportError::Io {
                path: path.to_path_buf(),
                source,
            })
    }

    /// Open `path` for writing. [`OverwritePolicy::Prohibit`] uses
    /// `create_new` so a concurrent file cannot be truncated.
    fn create_output_file(
        path: &Path,
        overwrite: OverwritePolicy,
    ) -> Result<fs::File, ExportError> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true);
        match overwrite {
            OverwritePolicy::Prohibit => {
                opts.create_new(true);
            }
            OverwritePolicy::Replace => {
                opts.create(true).truncate(true);
            }
        }
        opts.open(path)
            .map_err(|source| match (overwrite, source.kind()) {
                (OverwritePolicy::Prohibit, ErrorKind::AlreadyExists) => {
                    ExportError::OverwriteRefused {
                        path: path.to_path_buf(),
                    }
                }
                _ => ExportError::Io {
                    path: path.to_path_buf(),
                    source,
                },
            })
    }

    /// Remove `path` if it exists. `NotFound` is success so a first export
    /// without a readout is not an error.
    fn remove_mem_file_if_present(path: PathBuf) -> Result<(), ExportError> {
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ExportError::Io { path, source }),
        }
    }
}

fn location(block: ParameterBlock, neuron: usize, channel: Option<usize>) -> ParameterLocation {
    ParameterLocation {
        block,
        neuron,
        channel,
    }
}

fn non_finite_kind(value: f32) -> Option<NonFiniteKind> {
    if value.is_nan() {
        Some(NonFiniteKind::Nan)
    } else if value.is_infinite() {
        if value.is_sign_positive() {
            Some(NonFiniteKind::PosInfinity)
        } else {
            Some(NonFiniteKind::NegInfinity)
        }
    } else {
        None
    }
}

fn encode_for(encoding: Q88Encoding, value: f32) -> i16 {
    match encoding {
        Q88Encoding::Signed => encode_q88_signed_full(value),
        // Bit-pattern storage: `u16 as i16` keeps the 16-bit word
        // (`200.0` → `C800` → `i16` `-14336`). That `i16` is **not** a signed
        // Q8.8 value — signed FPGA RAM would read it as `-56.0`.
        // `write_mem_files` refuses `Q88Encoding::Unsigned` so this pattern
        // never reaches silicon-hdl.
        Q88Encoding::Unsigned => encode_q88_unsigned(value) as i16,
    }
}

#[derive(Clone, Copy)]
enum MatrixKind {
    Hidden,
    Readout,
}

fn matrix_width(
    matrix: &[Vec<f32>],
    kind: MatrixKind,
) -> Result<Option<usize>, ParameterShapeError> {
    let mut rows = matrix.iter().enumerate();
    let Some((_, first_row)) = rows.next() else {
        return Ok(None);
    };
    let expected = first_row.len();
    for (row, values) in rows {
        if values.len() != expected {
            return Err(match kind {
                MatrixKind::Hidden => ParameterShapeError::RaggedWeights {
                    expected,
                    row,
                    len: values.len(),
                },
                MatrixKind::Readout => ParameterShapeError::RaggedReadout {
                    expected,
                    row,
                    len: values.len(),
                },
            });
        }
    }
    Ok(Some(expected))
}

impl FixedPointEncode for FpgaParameterExporter {
    fn encode_q88(&self, value: f32) -> i16 {
        // ENCODE SITE (signed Q8.8, full i16 range) — `.mem` / synthesis path.
        // Distinct from encode_q88_signed, the UART helper with the ±127.99
        // clamp. Encoding here with encode_q88_unsigned would flatten every
        // Dale-inhibitory weight to 0x0000.
        encode_q88_signed_full(value)
    }
}

impl ParameterExport for FpgaParameterExporter {
    fn export(&self) -> FpgaParameters {
        let thresholds_q88: Vec<i16> = self
            .thresholds
            .iter()
            .map(|&v| self.encode_q88(v))
            .collect();

        let weights_q88: Vec<i16> = self
            .weights
            .iter()
            .flat_map(|row| row.iter())
            .map(|&v| self.encode_q88(v))
            .collect();

        let decay_rates_q88: Vec<i16> = self
            .decay_rates
            .iter()
            .map(|&v| self.encode_q88(v))
            .collect();

        let version = if self.format_version.is_empty() {
            EXPORT_FORMAT_VERSION.to_string()
        } else {
            self.format_version.clone()
        };
        let timestamp = match &self.timestamp {
            TimestampPolicy::Supplied(value) => value.clone(),
            TimestampPolicy::Omit | TimestampPolicy::Now => chrono::Utc::now().to_rfc3339(),
        };

        let metadata = FpgaMetadata {
            version,
            timestamp,
            num_neurons: self.thresholds.len(),
            num_channels: if self.weights.is_empty() {
                0
            } else {
                self.weights[0].len()
            },
            target_latency_us: Some(SPIKENAUT_LEGACY_TARGET_LATENCY_US),
            memory_usage_kb: self.calculate_memory_usage(),
            // Legacy path always encodes signed full-range, regardless of
            // `set_encoding`. Record that fact rather than the unused knobs.
            encodings: BlockEncodings {
                output_weights: self.output_weights.is_some().then_some(Q88Encoding::Signed),
                ..BlockEncodings::default()
            },
            ..FpgaMetadata::default()
        };

        FpgaParameters {
            thresholds: thresholds_q88,
            weights: weights_q88,
            decay_rates: decay_rates_q88,
            output_weights: self.output_weights.as_ref().map(|matrix| {
                matrix
                    .iter()
                    .flat_map(|row| row.iter())
                    .map(|&v| self.encode_q88(v))
                    .collect()
            }),
            metadata,
        }
    }
}

impl CheckedParameterExport for FpgaParameterExporter {
    type Error = ParameterShapeError;

    fn try_export(&self) -> Result<FpgaParameters, Self::Error> {
        FpgaParameterExporter::try_export(self)
    }
}

impl MemFileWriter for FpgaParameterExporter {
    type Error = ExportError;

    fn write_mem_files(&self, output_dir: impl AsRef<Path>) -> Result<ExportReport, Self::Error> {
        self.write_with_config(output_dir, &ExportConfig::legacy_spikenaut_v2())
    }
}

impl Default for FpgaParameterExporter {
    fn default() -> Self {
        Self::new()
    }
}

/// Inclusive lower bound of **signed parameter-export** Q8.8 (`i16::MIN / 256`).
///
/// Distinct from [`STIMULUS_Q88_MIN`]: the UART helper saturates at `-127.99`
/// (`8003`), while a parameter word of `-128.0` is `8000`.
pub const Q88_SIGNED_MIN: f32 = -128.0;

/// Inclusive upper bound of **signed parameter-export** Q8.8 (`i16::MAX / 256`).
///
/// `32767 / 256 = 127.99609375`. Distinct from [`STIMULUS_Q88_MAX`] (`127.99`
/// → `7FFD`); the parameter maximum is `7FFF`.
pub const Q88_SIGNED_MAX: f32 = 32767.0 / 256.0;

/// Lower clamp bound for the **legacy UART** signed Q8.8 helper.
///
/// Values below this saturate to raw `-32765` (`8003`) in
/// [`encode_q88_signed`]. This is **not** the parameter-export range — see
/// [`Q88_SIGNED_MIN`]. silicon-hdl's `scripts/q88.py` mirrors this exact UART
/// bound; changing it desynchronises the stimulus path.
pub const STIMULUS_Q88_MIN: f32 = -127.99;

/// Upper clamp bound for the **legacy UART** signed Q8.8 helper.
///
/// Values above this saturate to raw `32765` (`7FFD`) in
/// [`encode_q88_signed`]. See [`STIMULUS_Q88_MIN`] and [`Q88_SIGNED_MAX`].
pub const STIMULUS_Q88_MAX: f32 = 127.99;

/// Inclusive upper bound of unsigned-magnitude Q8.8 (`65535 / 256`).
///
/// Used by the checked path when a block is [`Q88Encoding::Unsigned`].
pub const Q88_UNSIGNED_MAX: f32 = 65535.0 / 256.0;

/// Encode an `f32` as **unsigned-magnitude** Q8.8 (`u16`).
///
/// `raw = value × 256`, truncated toward zero, scaled result clamped to
/// `0..=65535`. `NaN` encodes as `0`.
///
/// # This is not the hardware convention
///
/// Nothing that talks to silicon-hdl may use this function for a hardware
/// `.mem` image. Parameter images are signed two's-complement Q8.8
/// (silicon-hdl GH#73) — use [`encode_q88_signed_full`]. UART stimuli use
/// [`encode_q88_signed`]. Encoding a parameter bank here flattens every
/// inhibitory weight to `0x0000`, which is silent: the resulting `.mem` file
/// is well-formed and loads cleanly.
///
/// It remains public for callers that genuinely want an unsigned magnitude in
/// the wider `0.0..=255.99609375` range, paired with [`q88_to_f32`].
///
/// ```rust
/// use silicon_bridge::{encode_q88_unsigned, q88_to_f32};
///
/// assert_eq!(encode_q88_unsigned(1.0), 256);
/// assert_eq!(encode_q88_unsigned(-1.0), 0); // negatives are lost here
/// assert_eq!(encode_q88_unsigned(1000.0), 65535); // saturates
/// assert_eq!(q88_to_f32(encode_q88_unsigned(0.5)), 0.5);
/// ```
pub fn encode_q88_unsigned(value: f32) -> u16 {
    // ENCODE SITE (unsigned Q8.8) — clamp on the *scaled* value, so the
    // representable input range is 0.0..=255.99609375 (65535 / 256).
    // `f32::clamp` propagates NaN, so map it to 0 before the cast.
    if value.is_nan() {
        return 0;
    }
    let scaled = value * 256.0;
    scaled.clamp(0.0, 65535.0) as u16
}

/// Encode an `f32` as **legacy UART** signed Q8.8 (`i16`, two's complement).
///
/// This is the host-stimulus helper. `raw = value × 256`, truncated toward
/// zero, with the *unscaled* input clamped to
/// [`STIMULUS_Q88_MIN`]`..=`[`STIMULUS_Q88_MAX`]. `NaN` encodes as `0`. The
/// UART wire format is big-endian (`to_be_bytes()`).
///
/// **Not the parameter-export contract.** `.mem` images use
/// [`encode_q88_signed_full`]: `-128.0` is `8000` there and `8003` here.
/// Do not silently repurpose this helper for parameter banks.
///
/// This is not interchangeable with [`encode_q88_unsigned`].
///
/// ```rust
/// use silicon_bridge::{encode_q88_signed, q88_signed_to_f32};
///
/// assert_eq!(encode_q88_signed(1.0), 256);
/// assert_eq!(encode_q88_signed(-1.0), -256); // negatives survive here
/// assert_eq!(encode_q88_signed(-1.0).to_be_bytes(), [0xFF, 0x00]);
/// assert_eq!(q88_signed_to_f32(encode_q88_signed(-0.5)), -0.5);
/// assert_eq!(encode_q88_signed(-128.0), -32765); // UART clamp, not 0x8000
/// ```
pub fn encode_q88_signed(value: f32) -> i16 {
    // ENCODE SITE (signed Q8.8) — UART / host-stimulus path only.
    // Clamp happens on the *unscaled* value so saturation lands on raw
    // ±32765 (±127.99 × 256, truncated) rather than the i16 limits.
    // `f32::clamp` propagates NaN, so map it to 0 before the i16 cast.
    if value.is_nan() {
        return 0;
    }
    (value.clamp(STIMULUS_Q88_MIN, STIMULUS_Q88_MAX) * 256.0) as i16
}

/// Encode an `f32` as **full-range signed parameter** Q8.8 (`i16`).
///
/// 16-bit two's complement, 8 fractional bits, scale 256, representable
/// range [`Q88_SIGNED_MIN`]`..=`[`Q88_SIGNED_MAX`]
/// (`[-128, 127.99609375]` → `8000..=7FFF`). Truncation toward zero.
/// Values outside that range saturate to `i16::MIN` / `i16::MAX`; `NaN`
/// encodes as `0`. The checked export path rejects overflow instead of
/// saturating unless [`RangePolicy::Saturate`] is selected.
///
/// This is the encoder [`FixedPointEncode`], [`format_q88_hex`], and the
/// checked `.mem` writer use. It is **not** [`encode_q88_signed`].
///
/// ```rust
/// use silicon_bridge::{encode_q88_signed_full, format_q88_hex, q88_signed_to_f32};
///
/// assert_eq!(format_q88_hex(-1.0), "FF00");
/// assert_eq!(format_q88_hex(-0.5), "FF80");
/// assert_eq!(format_q88_hex(0.0), "0000");
/// assert_eq!(format_q88_hex(0.5), "0080");
/// assert_eq!(format_q88_hex(1.0), "0100");
/// assert_eq!(format_q88_hex(-128.0), "8000");
/// assert_eq!(format_q88_hex(127.99609375), "7FFF");
/// assert_eq!(q88_signed_to_f32(encode_q88_signed_full(-128.0)), -128.0);
/// ```
pub fn encode_q88_signed_full(value: f32) -> i16 {
    // ENCODE SITE (signed Q8.8, full i16 range) — parameter / `.mem` path.
    // Float-to-int `as` truncates toward zero and saturates at the i16
    // limits, so -128.0 → 0x8000 and 127.99609375 → 0x7FFF.
    if value.is_nan() {
        return 0;
    }
    (value * 256.0) as i16
}

/// Decode a **signed** Q8.8 (`i16`) word back to `f32`.
///
/// Counterpart of both [`encode_q88_signed_full`] (parameter words) and
/// [`encode_q88_signed`] (UART words). Every `i16` is valid
/// (`-32768..=32767` → `-128.0..=127.99609375`).
pub fn q88_signed_to_f32(raw: i16) -> f32 {
    raw as f32 / 256.0
}

/// Format an `f32` as one **parameter** `.mem` word: uppercase `{:04X}` hex of
/// its full-range signed Q8.8 two's-complement bit pattern.
///
/// Uses [`encode_q88_signed_full`], not the UART helper.
///
/// ```rust
/// use silicon_bridge::format_q88_hex;
///
/// assert_eq!(format_q88_hex(1.0), "0100");
/// assert_eq!(format_q88_hex(-1.0), "FF00"); // a Dale-inhibitory weight
/// assert_eq!(format_q88_hex(-128.0), "8000");
/// assert_eq!(format_q88_hex(127.99609375), "7FFF");
/// ```
pub fn format_q88_hex(value: f32) -> String {
    format!("{:04X}", encode_q88_signed_full(value) as u16)
}

/// Convert **unsigned-magnitude** Q8.8 back to `f32`.
///
/// Counterpart of [`encode_q88_unsigned`], and like it, not the hardware
/// signed-parameter convention. Decode `.mem` words with
/// [`q88_signed_to_f32`]; reading a signed word here turns every negative
/// into a large positive (`0xFF00` → `255.0` instead of `-1.0`). The same
/// hex `FFFF` is unsigned `255.99609375` and signed `-1/256`.
pub fn q88_to_f32(q88_value: u16) -> f32 {
    q88_value as f32 / 256.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_q88_conversion() {
        let exporter = FpgaParameterExporter::new();

        // Test basic conversions
        assert_eq!(exporter.to_q88(0.0), 0);
        assert_eq!(exporter.to_q88(1.0), 256);
        assert_eq!(exporter.to_q88(127.0), 32512);
        assert_eq!(exporter.to_q88(-1.0), -256);
        assert_eq!(exporter.to_q88(-127.0), -32512);

        // Test precision
        assert_eq!(q88_signed_to_f32(256), 1.0);
        assert_eq!(q88_signed_to_f32(0), 0.0);
        assert_eq!(q88_signed_to_f32(32512), 127.0);
        assert_eq!(q88_signed_to_f32(-256), -1.0);
    }

    #[test]
    fn test_parameter_export() {
        let mut exporter = FpgaParameterExporter::new();

        // Set test parameters
        exporter.set_thresholds(vec![1.0, 0.8, 1.2]);
        exporter.set_weights(vec![
            vec![0.5, 1.0, 0.3],
            vec![0.7, 0.9, 1.1],
            vec![0.4, 0.6, 0.8],
        ]);
        exporter.set_decay_rates(vec![0.85, 0.9, 0.8]);

        // Export to FPGA format
        let params = exporter.export();

        // Verify conversion
        assert_eq!(params.thresholds.len(), 3);
        assert_eq!(params.weights.len(), 9); // 3x3
        assert_eq!(params.decay_rates.len(), 3);
        assert_eq!(params.metadata.num_neurons, 3);
        assert_eq!(params.metadata.num_channels, 3);

        // Verify converted Q8.8 values are correct
        assert_eq!(params.thresholds, vec![256, 204, 307]);
        assert_eq!(
            params.weights,
            vec![128, 256, 76, 179, 230, 281, 102, 153, 204]
        );
        assert_eq!(params.decay_rates, vec![217, 230, 204]);
    }

    #[test]
    fn test_memory_calculation() {
        let mut exporter = FpgaParameterExporter::new();

        // Set parameters for 16 neurons, 16 channels
        exporter.set_thresholds(vec![1.0; 16]);
        exporter.set_weights(vec![vec![0.5; 16]; 16]);
        exporter.set_decay_rates(vec![0.85; 16]);

        let params = exporter.export();

        // Expected memory: (16 + 256 + 16) * 2 bytes = 576 bytes = 0.5625 KB
        assert!((params.metadata.memory_usage_kb - 0.5625).abs() < 0.01);
    }

    #[test]
    fn test_trait_surface() {
        let exporter = FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5]], vec![0.9]);

        // Trait methods (not only inherent methods) are the stable hardware surface.
        assert_eq!(FixedPointEncode::encode_q88(&exporter, 1.0), 256);
        let params = ParameterExport::export(&exporter);
        assert_eq!(params.metadata.version, EXPORT_FORMAT_VERSION);
        assert_eq!(params.thresholds, vec![256]);
        assert_eq!(params.weights, vec![128]);
        assert_eq!(params.decay_rates, vec![230]);
        assert!(params.metadata.target_latency_us.is_some());
    }

    #[test]
    fn checked_export_defaults_are_generic_not_spikenaut() {
        let exporter = FpgaParameterExporter::from_params(vec![1.0], vec![vec![-1.0]], vec![0.5]);
        let params = exporter.try_export().expect("checked");
        assert!(
            params.metadata.version.is_empty(),
            "default checked path must not stamp Spikenaut-v2"
        );
        assert!(!params.metadata.version.contains("Spikenaut"));
        assert!(params.metadata.timestamp.is_empty());
        assert!(params.metadata.target_latency_us.is_none());
        assert_eq!(params.weights[0], -256);

        let dir = tempfile::tempdir().unwrap();
        let report = exporter.write_generic(dir.path()).expect("generic write");
        assert_eq!(report.profile, GENERIC_DENSE_PROFILE_ID);
        let json = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(!json.contains("Spikenaut"));
    }

    #[test]
    fn format_version_override_is_not_forced_to_spikenaut() {
        let mut exporter =
            FpgaParameterExporter::from_params(vec![1.0], vec![vec![-1.0]], vec![0.5]);
        exporter.set_format_version("generic-dense-q88");
        exporter
            .set_timestamp("1970-01-01T00:00:00Z")
            .expect("rfc3339 utc");

        let params = exporter.try_export().expect("checked");
        assert_eq!(params.metadata.version, "generic-dense-q88");
        assert_eq!(params.metadata.timestamp, "1970-01-01T00:00:00Z");
        assert_eq!(params.weights[0], -256);
        assert!(
            !params.metadata.version.contains("Spikenaut"),
            "generic layout tag must not inherit Spikenaut identity"
        );

        let again = exporter.try_export().expect("repeat");
        assert_eq!(params.metadata.timestamp, again.metadata.timestamp);
        assert_eq!(params.metadata.version, again.metadata.version);
    }

    #[test]
    fn wall_clock_timestamp_can_be_restored() {
        let mut exporter =
            FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5]], vec![0.5]);
        exporter
            .set_timestamp("1970-01-01T00:00:00Z")
            .expect("rfc3339 utc");
        exporter.use_wall_clock_timestamp();
        let params = exporter.try_export().expect("checked");
        assert_ne!(params.metadata.timestamp, "1970-01-01T00:00:00Z");
        assert!(!params.metadata.timestamp.is_empty());
    }

    #[test]
    fn set_timestamp_rejects_empty_invalid_and_non_utc() {
        let mut exporter =
            FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5]], vec![0.5]);
        assert_eq!(
            exporter.set_timestamp(""),
            Err(MetadataTimestampError::InvalidRfc3339)
        );
        assert_eq!(
            exporter.set_timestamp("not-a-timestamp"),
            Err(MetadataTimestampError::InvalidRfc3339)
        );
        assert_eq!(
            exporter.set_timestamp("2020-01-01T00:00:00-05:00"),
            Err(MetadataTimestampError::NotUtc)
        );
        exporter
            .set_timestamp("1970-01-01T00:00:00+00:00")
            .expect("explicit zero offset is UTC");
        let params = exporter.try_export().expect("checked");
        assert_eq!(params.metadata.timestamp, "1970-01-01T00:00:00+00:00");
    }

    /// Small fixture whose floats are exact Q8.8 multiples of 1/256.
    fn mem_writer_fixture() -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(
            vec![1.0, 0.75],
            vec![vec![0.5, 2.0], vec![0.25, 1.5]],
            vec![0.5, 0.75],
        )
    }

    fn read_mem_lines(path: impl AsRef<Path>) -> Vec<String> {
        fs::read_to_string(path)
            .expect("mem file should be readable")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn assert_uppercase_hex_words(lines: &[String]) {
        for line in lines {
            assert_eq!(line.len(), 4, "expected XXXX hex word, got {line:?}");
            assert!(
                line.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
                "expected uppercase XXXX hex word, got {line:?}"
            );
        }
    }

    /// Read `.mem` words back the way the RTL does: parse the 16-bit pattern,
    /// then reinterpret it as signed two's complement.
    fn parse_mem_words(lines: &[String]) -> Vec<i16> {
        lines
            .iter()
            .map(|line| u16::from_str_radix(line, 16).expect("mem line should be valid hex") as i16)
            .collect()
    }

    #[test]
    fn test_write_mem_files_via_trait_emits_expected_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exporter = mem_writer_fixture();

        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("write_mem_files");

        let mut names: Vec<String> = fs::read_dir(dir.path())
            .expect("output dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "parameters.json",
                "parameters.mem",
                "parameters_decay.mem",
                "parameters_weights.mem",
            ]
        );

        let thresholds = read_mem_lines(dir.path().join("parameters.mem"));
        let weights = read_mem_lines(dir.path().join("parameters_weights.mem"));
        let decay_rates = read_mem_lines(dir.path().join("parameters_decay.mem"));

        assert_uppercase_hex_words(&thresholds);
        assert_uppercase_hex_words(&weights);
        assert_uppercase_hex_words(&decay_rates);

        // 1.0 -> 0x0100, 0.75 -> 0x00C0 (letter digit proves uppercase)
        assert_eq!(thresholds, ["0100", "00C0"]);
        // Flattened row-major: 0.5, 2.0, 0.25, 1.5
        assert_eq!(weights, ["0080", "0200", "0040", "0180"]);
        assert_eq!(decay_rates, ["0080", "00C0"]);
    }

    #[test]
    fn test_mem_files_round_trip_to_parameter_export() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exporter = mem_writer_fixture();

        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("write_mem_files");

        let expected = ParameterExport::export(&exporter);
        let thresholds = parse_mem_words(&read_mem_lines(dir.path().join("parameters.mem")));
        let weights = parse_mem_words(&read_mem_lines(dir.path().join("parameters_weights.mem")));
        let decay_rates = parse_mem_words(&read_mem_lines(dir.path().join("parameters_decay.mem")));

        assert_eq!(thresholds, expected.thresholds);
        assert_eq!(weights, expected.weights);
        assert_eq!(decay_rates, expected.decay_rates);

        let decoded: Vec<f32> = thresholds.iter().copied().map(q88_signed_to_f32).collect();
        for (got, want) in decoded.iter().zip([1.0_f32, 0.75]) {
            assert!(
                (got - want).abs() < 1e-6,
                "Q8.8 round-trip drifted: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn test_metadata_json_round_trips_to_fpga_parameters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exporter = mem_writer_fixture();

        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("write_mem_files");

        let json = fs::read_to_string(dir.path().join("parameters.json")).expect("parameters.json");
        let round_tripped: FpgaParameters =
            serde_json::from_str(&json).expect("parameters.json should deserialize");

        let expected = ParameterExport::export(&exporter);
        assert_eq!(round_tripped.thresholds, expected.thresholds);
        assert_eq!(round_tripped.weights, expected.weights);
        assert_eq!(round_tripped.decay_rates, expected.decay_rates);

        assert_eq!(round_tripped.metadata.version, EXPORT_FORMAT_VERSION);
        assert_eq!(round_tripped.metadata.num_neurons, 2);
        assert_eq!(round_tripped.metadata.num_channels, 2);
        assert_eq!(round_tripped.metadata.encodings, BlockEncodings::default());
        assert_eq!(
            round_tripped
                .metadata
                .encodings
                .for_block(ParameterBlock::Weights),
            Q88Encoding::Signed
        );
        assert!(!round_tripped.metadata.timestamp.is_empty());
        assert!(
            (round_tripped.metadata.target_latency_us.unwrap_or_default()
                - SPIKENAUT_LEGACY_TARGET_LATENCY_US)
                .abs()
                < 1e-6
        );
        // (2 thresholds + 4 weights + 2 decay) * 2 bytes = 16 bytes
        assert!((round_tripped.metadata.memory_usage_kb - 16.0 / 1024.0).abs() < 1e-6);
    }

    /// Dale-inhibitory fixture: every weight row mixes excitatory and
    /// inhibitory values, the shape a real E/I bank has.
    fn dale_fixture() -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(
            vec![1.0, 0.75],
            vec![vec![0.5, -1.0], vec![-0.00390625, 1.5]],
            vec![0.5, 0.75],
        )
    }

    /// The regression this whole module exists for.
    ///
    /// `write_mem_files` used to encode through `encode_q88_unsigned`, which
    /// clamps negatives to `0`. A Dale-inhibitory weight of `-1.0` was written
    /// as `0000` — a well-formed word that loads cleanly and silently removes
    /// the inhibition. silicon-hdl reads `.mem` images as signed two's
    /// complement (GH#73), so `-1.0` must land on disk as `FF00` and read back
    /// as `-1.0`.
    #[test]
    fn negative_weights_survive_the_mem_round_trip_as_twos_complement() {
        let dir = tempfile::tempdir().expect("tempdir");

        MemFileWriter::write_mem_files(&dale_fixture(), dir.path()).expect("write_mem_files");

        let lines = read_mem_lines(dir.path().join("parameters_weights.mem"));

        // On disk: two's-complement patterns, not clamped-to-zero magnitudes.
        assert_eq!(lines, ["0080", "FF00", "FFFF", "0180"]);
        assert_uppercase_hex_words(&lines);
        assert!(
            !lines.iter().any(|word| word == "0000"),
            "an inhibitory weight was flattened to 0x0000: {lines:?}"
        );

        // Read back the way the RTL does, then decode.
        let raw = parse_mem_words(&lines);
        assert_eq!(raw, [128, -256, -1, 384]);

        let decoded: Vec<f32> = raw.iter().copied().map(q88_signed_to_f32).collect();
        assert_eq!(decoded, [0.5, -1.0, -0.00390625, 1.5]);
    }

    /// The same words the shipped exp-025 bank actually contains, so this
    /// crate's encoder is pinned against silicon-hdl's real images rather than
    /// only against itself. Patterns taken from
    /// `spikenaut-core-sv/mem/merged_v2_weights.mem`.
    #[test]
    fn encoder_reproduces_shipped_bank_bit_patterns() {
        for (value, word) in [
            (-1.0_f32, "FF00"),       // 0xFF00 — most common Dale-I weight
            (-1.0 / 256.0, "FFFF"),   // 0xFFFF — smallest negative step
            (-243.0 / 256.0, "FF0D"), // 0xFF0D
            (-227.0 / 256.0, "FF1D"), // 0xFF1D
            (-118.0 / 256.0, "FF8A"), // 0xFF8A
            (1.125, "0120"),          // 0x0120 — the README's worked example
        ] {
            assert_eq!(format_q88_hex(value), word, "for {value}");
            let raw = u16::from_str_radix(word, 16).expect("hex") as i16;
            assert_eq!(q88_signed_to_f32(raw), value, "decode for {word}");
        }
    }

    /// Pins *why* the encode site moved, so a future revert is a failing test
    /// rather than a silently wrong bitstream.
    #[test]
    fn the_unsigned_encoder_would_still_flatten_every_inhibitory_weight() {
        let params = ParameterExport::export(&dale_fixture());
        assert_eq!(params.weights, [128, -256, -1, 384]);

        let as_unsigned: Vec<u16> = [0.5_f32, -1.0, -0.00390625, 1.5]
            .into_iter()
            .map(encode_q88_unsigned)
            .collect();
        assert_eq!(
            as_unsigned,
            [128, 0, 0, 384],
            "the unsigned helper is unchanged — it is simply no longer wired \
             to the .mem path"
        );
    }

    /// A pre-fix `parameters.json` carried unsigned words up to `65535`. Those
    /// no longer fit the `i16` fields, and `EXPORT_FORMAT_VERSION` is
    /// deliberately unchanged, so the compatibility boundary is worth pinning:
    /// the break must be a loud deserialization error, never a value that
    /// wraps to a plausible-looking negative.
    #[test]
    fn pre_fix_unsigned_json_fails_loudly_rather_than_wrapping() {
        let legacy = r#"{
            "thresholds": [65280],
            "weights": [51200],
            "decay_rates": [217],
            "metadata": {
                "version": "Spikenaut-v2",
                "timestamp": "2026-01-01T00:00:00Z",
                "num_neurons": 1,
                "num_channels": 1,
                "target_latency_us": 35.0,
                "memory_usage_kb": 0.006
            }
        }"#;

        let err = serde_json::from_str::<FpgaParameters>(legacy)
            .expect_err("65280 must not deserialize into an i16 field");
        assert!(
            err.to_string().contains("invalid value") || err.to_string().contains("i16"),
            "expected an out-of-range error, got: {err}"
        );

        // Words that were already inside the signed range still load, so only
        // the genuinely ambiguous half of the old format is rejected.
        let in_range = legacy
            .replace("[65280]", "[256]")
            .replace("[51200]", "[128]");
        let parsed: FpgaParameters =
            serde_json::from_str(&in_range).expect("in-range legacy words still deserialize");
        assert_eq!(parsed.thresholds, [256]);
        assert_eq!(parsed.weights, [128]);
    }

    /// The metadata JSON carries the same signed words as the `.mem` files, so
    /// tooling reading either sees one set of values.
    #[test]
    fn metadata_json_carries_signed_words() {
        let dir = tempfile::tempdir().expect("tempdir");

        MemFileWriter::write_mem_files(&dale_fixture(), dir.path()).expect("write_mem_files");

        let json = fs::read_to_string(dir.path().join("parameters.json")).expect("parameters.json");
        let round_tripped: FpgaParameters =
            serde_json::from_str(&json).expect("parameters.json should deserialize");

        assert_eq!(round_tripped.weights, [128, -256, -1, 384]);
        assert!(
            json.contains("-256"),
            "the JSON should record -256, not 65280: {json}"
        );
    }
}

#[cfg(test)]
mod weight_shape_tests {
    use super::*;

    /// Rows of unequal length, the shape that used to flatten silently.
    fn ragged() -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(
            vec![1.0, 1.0, 1.0],
            vec![vec![0.5, 0.5], vec![0.5], vec![0.5, 0.5]],
            vec![0.9, 0.9, 0.9],
        )
    }

    /// `validate` now requires N thresholds, N decay rates, and N weight
    /// rows. The infallible [`ParameterExport::export`] still copies the
    /// threshold count into `num_neurons` without that check — pinned here so
    /// the legacy contract is a test, not a silent behaviour change.
    #[test]
    fn rectangular_rows_export_even_when_the_vector_lengths_disagree() {
        let mismatched = FpgaParameterExporter::from_params(
            vec![1.0; 16],
            vec![vec![0.5, 0.5]; 4],
            vec![0.9; 2],
        );

        assert_eq!(
            mismatched.validate(),
            Err(ParameterShapeError::DimensionMismatch {
                block: ParameterBlock::DecayRates,
                expected: 16,
                actual: 2,
            })
        );

        let params = ParameterExport::export(&mismatched);
        assert_eq!(params.metadata.num_neurons, 16, "from the threshold count");
        assert_eq!(params.weights.len(), 8, "only 4 rows of 2 were supplied");
    }

    #[test]
    fn rectangular_weights_validate() {
        let exporter = FpgaParameterExporter::from_params(
            vec![1.0, 1.0],
            vec![vec![0.5, 0.5, 0.5], vec![0.25, 0.25, 0.25]],
            vec![0.9, 0.9],
        );
        assert_eq!(exporter.validate(), Ok(()));
    }

    #[test]
    fn an_empty_exporter_is_rejected_on_the_checked_path() {
        assert_eq!(
            FpgaParameterExporter::new().validate(),
            Err(ParameterShapeError::EmptyLayer)
        );
        // Legacy export stays infallible and still produces an empty bundle.
        let params = ParameterExport::export(&FpgaParameterExporter::new());
        assert!(params.thresholds.is_empty());
        assert!(params.weights.is_empty());
        assert!(params.decay_rates.is_empty());
    }

    #[test]
    fn a_single_weight_row_validates() {
        let exporter =
            FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5, 0.5, 0.5]], vec![0.9]);
        assert_eq!(exporter.validate(), Ok(()));
    }

    #[test]
    fn validate_reports_the_first_ragged_row() {
        assert_eq!(
            ragged().validate(),
            Err(ParameterShapeError::RaggedWeights {
                expected: 2,
                row: 1,
                len: 1,
            })
        );
    }

    /// A short row shifts every later weight one slot toward the front, so
    /// `WeightRam` addressed as `row * num_channels + channel` reads neuron 2's
    /// first weight where neuron 1's second belongs. The buffer/metadata
    /// disagreement this asserts is exactly what `validate` now catches.
    #[test]
    fn export_flattens_a_ragged_matrix_into_a_buffer_metadata_cannot_describe() {
        let params = ParameterExport::export(&ragged());

        assert_eq!(params.metadata.num_neurons, 3);
        assert_eq!(params.metadata.num_channels, 2, "width read from row 0");
        assert_eq!(params.weights.len(), 5, "2 + 1 + 2 values were flattened");
        assert_ne!(
            params.weights.len(),
            params.metadata.num_neurons * params.metadata.num_channels,
            "a 3x2 read would run off the end of a 5-word buffer"
        );
    }

    #[test]
    fn write_mem_files_rejects_a_ragged_matrix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("export");

        let err = MemFileWriter::write_mem_files(&ragged(), &output)
            .expect_err("ragged weights should not produce .mem files");

        match err {
            ExportError::InvalidParameters(shape) => {
                assert_eq!(
                    shape,
                    ParameterShapeError::RaggedWeights {
                        expected: 2,
                        row: 1,
                        len: 1,
                    }
                );
            }
            other => panic!("expected InvalidParameters, got {other}"),
        }
        assert!(
            !output.exists(),
            "the output directory should not be created for an invalid shape"
        );
    }

    #[test]
    fn write_mem_files_still_accepts_a_rectangular_matrix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exporter = FpgaParameterExporter::from_params(
            vec![1.0, 0.75],
            vec![vec![0.5, 2.0], vec![0.25, 1.5]],
            vec![0.5, 0.75],
        );

        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("write_mem_files");
        assert!(dir.path().join("parameters_weights.mem").exists());
    }

    #[test]
    fn the_error_message_names_both_row_widths() {
        let message = ParameterShapeError::RaggedWeights {
            expected: 2,
            row: 1,
            len: 1,
        }
        .to_string();

        assert!(message.contains("row 0 has 2 channels"), "{message}");
        assert!(message.contains("row 1 has 1"), "{message}");
    }
}

#[cfg(test)]
mod q88_convention_tests {
    use super::*;

    const MAX_UNSIGNED_INPUT: f32 = 65535.0 / 256.0; // 255.99609375

    #[test]
    fn unsigned_encode_scales_by_256() {
        assert_eq!(encode_q88_unsigned(0.0), 0);
        assert_eq!(encode_q88_unsigned(1.0), 256);
        assert_eq!(encode_q88_unsigned(0.5), 128);
        assert_eq!(encode_q88_unsigned(1.0 / 256.0), 1);
        assert_eq!(encode_q88_unsigned(255.0), 65280);
    }

    #[test]
    fn unsigned_encode_truncates_toward_zero() {
        assert_eq!(encode_q88_unsigned(0.999), 255);
        assert_eq!(encode_q88_unsigned(1.9999), 511);
    }

    #[test]
    fn unsigned_encode_clamps_at_both_ends() {
        assert_eq!(encode_q88_unsigned(MAX_UNSIGNED_INPUT), 65535);
        assert_eq!(encode_q88_unsigned(256.0), 65535);
        assert_eq!(encode_q88_unsigned(1.0e6), 65535);
        assert_eq!(encode_q88_unsigned(f32::MAX), 65535);
        assert_eq!(encode_q88_unsigned(f32::INFINITY), 65535);

        assert_eq!(encode_q88_unsigned(0.0), 0);
        assert_eq!(encode_q88_unsigned(-1.0 / 256.0), 0);
        assert_eq!(encode_q88_unsigned(-1.0), 0);
        assert_eq!(encode_q88_unsigned(-127.99), 0);
        assert_eq!(encode_q88_unsigned(f32::MIN), 0);
        assert_eq!(encode_q88_unsigned(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn unsigned_encode_maps_nan_to_zero() {
        assert_eq!(encode_q88_unsigned(f32::NAN), 0);
    }

    #[test]
    fn unsigned_encode_round_trips_through_q88_to_f32() {
        for value in [0.0_f32, 0.00390625, 0.5, 1.0, 12.25, MAX_UNSIGNED_INPUT] {
            let raw = encode_q88_unsigned(value);
            assert_eq!(q88_to_f32(raw), value, "round trip failed for {value}");
        }
    }

    /// The exporter's encode site must be `encode_q88_signed_full`, not the
    /// UART helper or the unsigned magnitude helper. Reusing
    /// `encode_q88_signed` would map `-128.0` to `8003` instead of `8000`.
    #[test]
    fn trait_and_inherent_encoders_match_the_parameter_encoder() {
        let exporter = FpgaParameterExporter::new();
        for value in [-128.0_f32, -127.0, -5.0, -0.3, 0.0, 0.3, 1.0, 127.0, 300.0] {
            let expected = encode_q88_signed_full(value);
            assert_eq!(FixedPointEncode::encode_q88(&exporter, value), expected);
            assert_eq!(exporter.to_q88(value), expected);
        }
        assert_ne!(
            encode_q88_signed(-128.0),
            encode_q88_signed_full(-128.0),
            "UART clamp must not be the parameter encoder"
        );
    }

    #[test]
    fn mem_words_are_four_hex_digits_uppercase() {
        assert_eq!(format_q88_hex(0.0), "0000");
        assert_eq!(format_q88_hex(1.0), "0100");
        assert_eq!(format_q88_hex(STIMULUS_Q88_MAX), "7FFD");
        assert_eq!(format_q88_hex(-1.0), "FF00");
        assert_eq!(format_q88_hex(STIMULUS_Q88_MIN), "8003");
        assert_eq!(format_q88_hex(Q88_SIGNED_MIN), "8000");
        assert_eq!(format_q88_hex(Q88_SIGNED_MAX), "7FFF");
    }

    #[test]
    fn signed_encode_scales_by_256_in_both_directions() {
        assert_eq!(encode_q88_signed(0.0), 0);
        assert_eq!(encode_q88_signed(1.0), 256);
        assert_eq!(encode_q88_signed(-1.0), -256);
        assert_eq!(encode_q88_signed(0.5), 128);
        assert_eq!(encode_q88_signed(-0.5), -128);
        assert_eq!(encode_q88_signed(1.0 / 256.0), 1);
        assert_eq!(encode_q88_signed(-1.0 / 256.0), -1);
    }

    #[test]
    fn signed_encode_truncates_toward_zero() {
        assert_eq!(encode_q88_signed(0.999), 255);
        assert_eq!(encode_q88_signed(-0.999), -255);
    }

    #[test]
    fn signed_encode_clamps_at_both_ends() {
        assert_eq!(encode_q88_signed(STIMULUS_Q88_MAX), 32765);
        assert_eq!(encode_q88_signed(STIMULUS_Q88_MIN), -32765);
        assert_eq!(encode_q88_signed(128.0), 32765);
        assert_eq!(encode_q88_signed(-128.0), -32765);
        assert_eq!(encode_q88_signed(1.0e6), 32765);
        assert_eq!(encode_q88_signed(-1.0e6), -32765);
        assert_eq!(encode_q88_signed(f32::MAX), 32765);
        assert_eq!(encode_q88_signed(f32::MIN), -32765);
        assert_eq!(encode_q88_signed(f32::INFINITY), 32765);
        assert_eq!(encode_q88_signed(f32::NEG_INFINITY), -32765);
    }

    #[test]
    fn signed_encode_maps_nan_to_zero() {
        assert_eq!(encode_q88_signed(f32::NAN), 0);
    }

    #[test]
    fn signed_wire_words_are_big_endian() {
        assert_eq!(encode_q88_signed(1.0).to_be_bytes(), [0x01, 0x00]);
        assert_eq!(encode_q88_signed(-1.0).to_be_bytes(), [0xFF, 0x00]);
        assert_eq!(encode_q88_signed(-0.5).to_be_bytes(), [0xFF, 0x80]);
        assert_eq!(
            encode_q88_signed(STIMULUS_Q88_MAX).to_be_bytes(),
            [0x7F, 0xFD]
        );
    }

    #[test]
    fn signed_encode_round_trips_through_q88_signed_to_f32() {
        for value in [-127.0_f32, -12.25, -1.0, -0.00390625, 0.0, 0.5, 64.75] {
            let raw = encode_q88_signed(value);
            assert_eq!(
                q88_signed_to_f32(raw),
                value,
                "round trip failed for {value}"
            );
        }
    }

    #[test]
    fn signed_decoder_covers_the_full_i16_wire_range() {
        assert_eq!(q88_signed_to_f32(i16::MIN), -128.0);
        assert_eq!(q88_signed_to_f32(i16::MAX), 32767.0 / 256.0);
        assert!(encode_q88_signed(-128.0) > i16::MIN);
        assert!(encode_q88_signed(f32::MAX) < i16::MAX);
    }

    #[test]
    fn signed_and_unsigned_agree_only_on_the_shared_range() {
        for value in [0.0_f32, 0.5, 1.0, 64.25, 127.0] {
            assert_eq!(
                i32::from(encode_q88_signed(value)),
                i32::from(encode_q88_unsigned(value)),
                "conventions should agree for {value}"
            );
        }

        assert_eq!(encode_q88_signed(200.0), 32765);
        assert_eq!(encode_q88_unsigned(200.0), 51200);
    }

    #[test]
    fn using_the_wrong_convention_corrupts_negative_values() {
        assert_eq!(encode_q88_signed(-1.0), -256);
        assert_eq!(encode_q88_unsigned(-1.0), 0);

        let wire = encode_q88_signed(-1.0);
        assert_eq!(q88_signed_to_f32(wire), -1.0);
        assert_eq!(q88_to_f32(wire as u16), 255.0);
    }
}

#[cfg(test)]
mod checked_export_tests {
    use super::*;
    use std::error::Error;

    fn dense(n: usize, m: usize) -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(vec![1.0; n], vec![vec![0.5; m]; n], vec![0.9; n])
    }

    fn loc(block: ParameterBlock, neuron: usize, channel: Option<usize>) -> ParameterLocation {
        ParameterLocation {
            block,
            neuron,
            channel,
        }
    }

    fn assert_invalid(err: ExportError, expected: ParameterShapeError) {
        match err {
            ExportError::InvalidParameters(shape) => assert_eq!(shape, expected),
            other => panic!("expected InvalidParameters({expected:?}), got {other}"),
        }
    }

    #[test]
    fn valid_non_square_layer_exports() {
        // 3 neurons × 5 inputs — not 16, not square.
        let exporter = dense(3, 5);
        assert_eq!(exporter.validate(), Ok(()));

        let params = exporter.try_export().expect("non-square layer is valid");
        assert_eq!(params.thresholds.len(), 3);
        assert_eq!(params.decay_rates.len(), 3);
        assert_eq!(params.weights.len(), 15);
        assert_eq!(params.metadata.num_neurons, 3);
        assert_eq!(params.metadata.num_channels, 5);
        assert_eq!(params.output_weights, None);

        let via_trait = CheckedParameterExport::try_export(&exporter).expect("trait");
        assert_eq!(via_trait.thresholds, params.thresholds);
        assert_eq!(via_trait.weights, params.weights);
    }

    #[test]
    fn empty_layer_is_rejected() {
        assert_eq!(
            FpgaParameterExporter::new().validate(),
            Err(ParameterShapeError::EmptyLayer)
        );
        assert!(FpgaParameterExporter::new().try_export().is_err());
    }

    #[test]
    fn empty_weight_width_is_rejected() {
        let exporter = FpgaParameterExporter::from_params(
            vec![1.0, 1.0],
            vec![vec![], vec![]],
            vec![0.9, 0.9],
        );
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::EmptyBlock {
                block: ParameterBlock::Weights,
            })
        );
    }

    #[test]
    fn threshold_count_mismatch_names_the_block() {
        let exporter =
            FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5], vec![0.5]], vec![0.9]);
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::DimensionMismatch {
                block: ParameterBlock::Weights,
                expected: 1,
                actual: 2,
            })
        );
    }

    #[test]
    fn decay_count_mismatch_names_the_block() {
        let exporter = FpgaParameterExporter::from_params(
            vec![1.0, 1.0],
            vec![vec![0.5], vec![0.5]],
            vec![0.9],
        );
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::DimensionMismatch {
                block: ParameterBlock::DecayRates,
                expected: 2,
                actual: 1,
            })
        );
    }

    #[test]
    fn optional_readout_absence_is_valid() {
        let exporter = dense(2, 3);
        assert!(
            exporter
                .try_export()
                .expect("no readout")
                .output_weights
                .is_none()
        );
    }

    #[test]
    fn optional_readout_kx_n_is_valid() {
        let mut exporter = dense(3, 4);
        exporter.set_output_weights(vec![vec![0.1, 0.2, 0.3], vec![-0.4, 0.5, 0.6]]);
        let params = exporter.try_export().expect("2×3 readout on N=3");
        let readout = params.output_weights.expect("readout present");
        assert_eq!(readout.len(), 6);
        assert_eq!(readout[3], encode_q88_signed_full(-0.4));
    }

    #[test]
    fn readout_wrong_column_count_is_rejected() {
        let mut exporter = dense(3, 2);
        // 2 outputs × 2 columns, but N = 3.
        exporter.set_output_weights(vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::ReadoutShape {
                expected_cols: 3,
                actual_cols: 2,
                rows: 2,
            })
        );
    }

    #[test]
    fn ragged_readout_names_the_row() {
        let mut exporter = dense(3, 2);
        exporter.set_output_weights(vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5]]);
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::RaggedReadout {
                expected: 3,
                row: 1,
                len: 2,
            })
        );
    }

    #[test]
    fn empty_readout_block_is_rejected() {
        let mut exporter = dense(2, 2);
        exporter.set_output_weights(Vec::new());
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::EmptyBlock {
                block: ParameterBlock::Readout,
            })
        );
    }

    #[test]
    fn nan_is_rejected_before_quantization() {
        let mut exporter = dense(2, 2);
        exporter.set_weights(vec![vec![0.5, 0.5], vec![0.5, f32::NAN]]);
        assert_eq!(
            exporter.validate(),
            Err(ParameterShapeError::NonFinite {
                location: loc(ParameterBlock::Weights, 1, Some(1)),
                kind: NonFiniteKind::Nan,
            })
        );
        // Legacy encoder still maps NaN to 0.
        assert_eq!(encode_q88_signed(f32::NAN), 0);
    }

    #[test]
    fn infinities_are_rejected_before_quantization() {
        let mut pos = dense(1, 1);
        pos.set_thresholds(vec![f32::INFINITY]);
        assert_eq!(
            pos.validate(),
            Err(ParameterShapeError::NonFinite {
                location: loc(ParameterBlock::Thresholds, 0, None),
                kind: NonFiniteKind::PosInfinity,
            })
        );

        let mut neg = dense(1, 1);
        neg.set_decay_rates(vec![f32::NEG_INFINITY]);
        assert_eq!(
            neg.validate(),
            Err(ParameterShapeError::NonFinite {
                location: loc(ParameterBlock::DecayRates, 0, None),
                kind: NonFiniteKind::NegInfinity,
            })
        );
    }

    #[test]
    fn signed_range_boundaries_are_accepted_and_the_next_step_is_not() {
        let mut at_max = dense(1, 1);
        at_max.set_weights(vec![vec![Q88_SIGNED_MAX]]);
        let max_params = at_max.try_export().expect("signed max is representable");
        assert_eq!(max_params.weights[0], i16::MAX);
        assert_eq!(format!("{:04X}", max_params.weights[0] as u16), "7FFF");

        let mut at_min = dense(1, 1);
        at_min.set_weights(vec![vec![Q88_SIGNED_MIN]]);
        let min_params = at_min.try_export().expect("signed min is representable");
        assert_eq!(min_params.weights[0], i16::MIN);
        assert_eq!(format!("{:04X}", min_params.weights[0] as u16), "8000");

        let mut over = dense(1, 1);
        over.set_weights(vec![vec![128.0]]);
        match over.try_export() {
            Err(ParameterShapeError::OutOfRange {
                location,
                value,
                encoding,
                min,
                max,
            }) => {
                assert_eq!(location, loc(ParameterBlock::Weights, 0, Some(0)));
                assert_eq!(value, 128.0);
                assert_eq!(encoding, Q88Encoding::Signed);
                assert_eq!(min, Q88_SIGNED_MIN);
                assert_eq!(max, Q88_SIGNED_MAX);
            }
            other => panic!("expected OutOfRange, got {other:?}"),
        }

        let mut under = dense(1, 1);
        under.set_weights(vec![vec![-128.0 - 1.0 / 256.0]]);
        assert!(matches!(
            under.try_export(),
            Err(ParameterShapeError::OutOfRange {
                encoding: Q88Encoding::Signed,
                ..
            })
        ));
    }

    #[test]
    fn unsigned_range_boundaries_are_accepted_and_negatives_are_not() {
        let mut exporter = dense(1, 1);
        exporter.set_encoding(ParameterBlock::Weights, Q88Encoding::Unsigned);
        exporter.set_weights(vec![vec![Q88_UNSIGNED_MAX]]);
        assert!(exporter.try_export().is_ok());

        exporter.set_weights(vec![vec![200.0]]);
        let params = exporter.try_export().expect("200.0 is in unsigned range");
        assert_eq!(params.weights[0] as u16, encode_q88_unsigned(200.0));
        // Bit-pattern storage: unsigned 200.0 is 0xC800, which is negative
        // as i16. silicon-hdl RAM would decode that as -56.0, not 200.0 —
        // which is why `write_mem_files` refuses unsigned encoding.
        assert_eq!(params.weights[0], encode_q88_unsigned(200.0) as i16);
        assert!(params.weights[0] < 0);
        assert_eq!(q88_signed_to_f32(params.weights[0]), -56.0);

        exporter.set_weights(vec![vec![-0.00390625]]);
        match exporter.try_export() {
            Err(ParameterShapeError::OutOfRange {
                encoding: Q88Encoding::Unsigned,
                value,
                min,
                max,
                ..
            }) => {
                assert_eq!(value, -0.00390625);
                assert_eq!(min, 0.0);
                assert_eq!(max, Q88_UNSIGNED_MAX);
            }
            other => panic!("expected unsigned OutOfRange, got {other:?}"),
        }

        exporter.set_weights(vec![vec![256.0]]);
        assert!(matches!(
            exporter.try_export(),
            Err(ParameterShapeError::OutOfRange {
                encoding: Q88Encoding::Unsigned,
                ..
            })
        ));
    }

    #[test]
    fn saturation_is_explicit_and_observable() {
        let mut exporter = dense(1, 1);
        exporter.set_range_policy(RangePolicy::Saturate);
        exporter.set_weights(vec![vec![200.0]]);
        exporter.set_thresholds(vec![-200.0]);

        // validate() still rejects: saturation is not a silent validate success.
        assert!(matches!(
            exporter.validate(),
            Err(ParameterShapeError::OutOfRange { .. })
        ));

        let (params, report) = exporter
            .try_export_with_report()
            .expect("Saturate allows out-of-range");
        assert_eq!(report.len(), 2);
        assert_eq!(report.events[0].original, -200.0);
        assert_eq!(report.events[0].saturated_to, Q88_SIGNED_MIN);
        assert_eq!(report.events[1].original, 200.0);
        assert_eq!(report.events[1].saturated_to, Q88_SIGNED_MAX);
        assert_eq!(params.thresholds[0], encode_q88_signed_full(Q88_SIGNED_MIN));
        assert_eq!(params.weights[0], encode_q88_signed_full(Q88_SIGNED_MAX));
    }

    #[test]
    fn saturation_still_rejects_non_finite_values() {
        let mut exporter = dense(1, 1);
        exporter.set_range_policy(RangePolicy::Saturate);
        exporter.set_weights(vec![vec![f32::NAN]]);
        assert!(matches!(
            exporter.try_export_with_report(),
            Err(ParameterShapeError::NonFinite {
                kind: NonFiniteKind::Nan,
                ..
            })
        ));
    }

    #[test]
    fn try_export_rejects_saturate_so_the_report_cannot_be_dropped() {
        let mut exporter = dense(1, 1);
        exporter.set_range_policy(RangePolicy::Saturate);
        exporter.set_weights(vec![vec![200.0]]);

        assert!(matches!(
            exporter.try_export(),
            Err(ParameterShapeError::SaturationRequiresReport)
        ));
        assert!(matches!(
            CheckedParameterExport::try_export(&exporter),
            Err(ParameterShapeError::SaturationRequiresReport)
        ));

        let err = MemFileWriter::write_mem_files(&exporter, tempfile::tempdir().unwrap().path())
            .expect_err("hardware writer must not silently saturate");
        assert_invalid(err, ParameterShapeError::SaturationRequiresReport);
    }

    #[test]
    fn write_mem_files_rejects_unsigned_encoding() {
        let mut exporter = dense(1, 1);
        exporter.set_encoding(ParameterBlock::Weights, Q88Encoding::Unsigned);
        exporter.set_weights(vec![vec![200.0]]);

        // In-memory checked export still encodes the unsigned bit pattern.
        let params = exporter.try_export().expect("unsigned in-memory encode");
        assert_eq!(params.weights[0] as u16, encode_q88_unsigned(200.0));

        let dir = tempfile::tempdir().expect("tempdir");
        let err = MemFileWriter::write_mem_files(&exporter, dir.path())
            .expect_err("unsigned must not reach a hardware .mem image");
        match err {
            ExportError::UnsignedHardwareEncoding {
                block: ParameterBlock::Weights,
            } => {}
            other => panic!("expected UnsignedHardwareEncoding(Weights), got {other}"),
        }
        assert!(
            !dir.path().join("parameters_weights.mem").exists(),
            "unsigned encoding must not write a .mem image"
        );
    }

    #[test]
    fn unsigned_readout_encoding_is_ignored_when_readout_is_absent() {
        let mut exporter = dense(1, 1);
        exporter.set_encoding(ParameterBlock::Readout, Q88Encoding::Unsigned);
        MemFileWriter::write_mem_files(&exporter, tempfile::tempdir().unwrap().path())
            .expect("absent readout does not occupy a hardware bank");
    }

    #[test]
    fn malformed_bundle_does_not_create_or_truncate_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("export");
        let good = dense(2, 2);
        MemFileWriter::write_mem_files(&good, &output).expect("seed files");

        let weights_path = output.join("parameters_weights.mem");
        let json_path = output.join("parameters.json");
        let before_weights = fs::read(&weights_path).expect("weights");
        let before_json = fs::read(&json_path).expect("json");

        let mut bad = dense(2, 2);
        bad.set_decay_rates(vec![0.9]);
        let err = MemFileWriter::write_mem_files(&bad, &output)
            .expect_err("mismatch must not rewrite files");
        assert_invalid(
            err,
            ParameterShapeError::DimensionMismatch {
                block: ParameterBlock::DecayRates,
                expected: 2,
                actual: 1,
            },
        );

        assert_eq!(
            fs::read(&weights_path).expect("weights after"),
            before_weights
        );
        assert_eq!(fs::read(&json_path).expect("json after"), before_json);

        let missing_dir = dir.path().join("never-created");
        MemFileWriter::write_mem_files(&bad, &missing_dir).expect_err("shape error");
        assert!(
            !missing_dir.exists(),
            "validation failure must not create the output directory"
        );
    }

    #[test]
    fn io_error_preserves_the_filesystem_cause() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("not-a-directory");
        fs::write(&blocker, b"not a dir").expect("blocker file");

        let err = MemFileWriter::write_mem_files(&dense(1, 1), &blocker)
            .expect_err("cannot create_dir_all over a file");
        match &err {
            ExportError::Io { path, .. } => {
                assert_eq!(path, &blocker);
            }
            other => panic!("expected Io, got {other}"),
        }
        assert!(
            Error::source(&err).is_some(),
            "I/O cause must be preserved: {err}"
        );
    }

    #[test]
    fn error_messages_identify_block_and_location() {
        let nan = ParameterShapeError::NonFinite {
            location: loc(ParameterBlock::Weights, 1, Some(2)),
            kind: NonFiniteKind::Nan,
        };
        let message = nan.to_string();
        assert!(message.contains("weights[1, 2]"), "{message}");
        assert!(message.contains("NaN"), "{message}");

        let range = ParameterShapeError::OutOfRange {
            location: loc(ParameterBlock::Thresholds, 4, None),
            value: 200.0,
            encoding: Q88Encoding::Signed,
            min: Q88_SIGNED_MIN,
            max: Q88_SIGNED_MAX,
        };
        let message = range.to_string();
        assert!(message.contains("thresholds[4]"), "{message}");
        assert!(message.contains("200"), "{message}");
    }

    #[test]
    fn checked_writer_emits_readout_file_only_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut exporter = dense(2, 2);
        exporter.set_output_weights(vec![vec![0.25, 0.5]]);

        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("write");
        assert!(dir.path().join("parameters_output_weights.mem").exists());
        let lines: Vec<String> =
            fs::read_to_string(dir.path().join("parameters_output_weights.mem"))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect();
        assert_eq!(lines, ["0040", "0080"]);
    }

    #[test]
    fn stale_readout_file_is_removed_when_output_weights_are_cleared() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut exporter = dense(2, 2);
        exporter.set_output_weights(vec![vec![0.25, 0.5]]);
        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("first write with readout");

        let readout = dir.path().join("parameters_output_weights.mem");
        assert!(readout.exists());

        exporter.clear_output_weights();
        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("re-export without readout");

        assert!(
            !readout.exists(),
            "stale parameters_output_weights.mem must not survive a readout-less re-export"
        );
        let json = fs::read_to_string(dir.path().join("parameters.json")).expect("json");
        let params: FpgaParameters = serde_json::from_str(&json).expect("json");
        assert!(params.output_weights.is_none());
        assert!(!json.contains("output_weights"));
    }
}

#[cfg(test)]
mod signed_parameter_encoding_tests {
    use super::*;

    fn dense(n: usize, m: usize) -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(vec![1.0; n], vec![vec![0.5; m]; n], vec![0.9; n])
    }

    fn read_mem_lines(path: impl AsRef<Path>) -> Vec<String> {
        fs::read_to_string(path)
            .expect("mem file should be readable")
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn golden_signed_parameter_hex_mappings() {
        for (value, word) in [
            (-1.0_f32, "FF00"),
            (-0.5, "FF80"),
            (0.0, "0000"),
            (0.5, "0080"),
            (1.0, "0100"),
            (-128.0, "8000"),
            (Q88_SIGNED_MAX, "7FFF"),
        ] {
            assert_eq!(
                format_q88_hex(value),
                word,
                "signed parameter word for {value}"
            );
            assert_eq!(
                encode_q88_signed_full(value) as u16,
                u16::from_str_radix(word, 16).unwrap(),
                "raw pattern for {value}"
            );
        }
    }

    #[test]
    fn unsigned_max_is_ffff_and_differs_from_signed_ffff() {
        assert_eq!(encode_q88_unsigned(Q88_UNSIGNED_MAX), 0xFFFF);
        assert_eq!(
            format!("{:04X}", encode_q88_unsigned(Q88_UNSIGNED_MAX)),
            "FFFF"
        );
        assert_eq!(q88_to_f32(0xFFFF), Q88_UNSIGNED_MAX);

        // Same 16-bit pattern, opposite interpretations.
        assert_eq!(q88_signed_to_f32(0xFFFF_u16 as i16), -1.0 / 256.0);
        assert_eq!(encode_q88_signed_full(-1.0 / 256.0) as u16, 0xFFFF);
        assert_ne!(
            q88_to_f32(0xFFFF),
            q88_signed_to_f32(0xFFFF_u16 as i16),
            "FFFF is unsigned 255.99609375 and signed -1/256"
        );
    }

    #[test]
    fn uart_helper_still_clamps_short_of_the_parameter_extrema() {
        assert_eq!(format!("{:04X}", encode_q88_signed(-128.0) as u16), "8003");
        assert_eq!(
            format!("{:04X}", encode_q88_signed(Q88_SIGNED_MAX) as u16),
            "7FFD"
        );
        assert_eq!(encode_q88_signed(-128.0), -32765);
        assert_eq!(encode_q88_signed_full(-128.0), i16::MIN);
        assert_eq!(encode_q88_signed_full(Q88_SIGNED_MAX), i16::MAX);
    }

    #[test]
    fn truncation_reconstruction_error_is_less_than_one_quantum() {
        const QUANTUM: f32 = 1.0 / 256.0;
        let samples = [
            Q88_SIGNED_MIN,
            -64.123,
            -1.0,
            -0.999,
            -0.5,
            -QUANTUM / 2.0,
            0.0,
            QUANTUM / 3.0,
            0.5,
            1.0,
            12.345,
            127.0,
            Q88_SIGNED_MAX,
        ];
        for value in samples {
            let back = q88_signed_to_f32(encode_q88_signed_full(value));
            let err = (back - value).abs();
            assert!(
                err < QUANTUM,
                "reconstruction error {err} for {value} is not < 1/256 (got {back})"
            );
        }
    }

    #[test]
    fn negative_hidden_and_readout_weights_survive_signed_export() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut exporter = dense(2, 2);
        exporter.set_weights(vec![vec![0.5, -1.0], vec![-0.5, 1.0]]);
        exporter.set_output_weights(vec![vec![-1.0, 0.5], vec![1.0, -0.5]]);

        MemFileWriter::write_mem_files(&exporter, dir.path()).expect("signed hardware write");

        let hidden = read_mem_lines(dir.path().join("parameters_weights.mem"));
        let readout = read_mem_lines(dir.path().join("parameters_output_weights.mem"));
        assert_eq!(hidden, ["0080", "FF00", "FF80", "0100"]);
        assert_eq!(readout, ["FF00", "0080", "0100", "FF80"]);

        let json = fs::read_to_string(dir.path().join("parameters.json")).expect("json");
        let params: FpgaParameters = serde_json::from_str(&json).expect("json");
        assert_eq!(params.weights, [128, -256, -128, 256]);
        assert_eq!(
            params.output_weights.as_deref(),
            Some(&[-256, 128, 256, -128][..])
        );
        assert_eq!(q88_signed_to_f32(params.weights[1]), -1.0);
        assert_eq!(
            q88_signed_to_f32(params.output_weights.as_ref().unwrap()[0]),
            -1.0
        );
        assert_eq!(params.metadata.encodings.weights, Q88Encoding::Signed);
        assert_eq!(
            params.metadata.encodings.output_weights,
            Some(Q88Encoding::Signed)
        );
    }

    #[test]
    fn positive_legacy_fixtures_retain_their_words() {
        let exporter = FpgaParameterExporter::from_params(
            vec![1.0, 0.75],
            vec![vec![0.5, 2.0], vec![0.25, 1.5]],
            vec![0.5, 0.75],
        );
        let params = exporter.try_export().expect("legacy positive fixture");
        assert_eq!(params.thresholds, [256, 192]);
        assert_eq!(params.weights, [128, 512, 64, 384]);
        assert_eq!(params.decay_rates, [128, 192]);
    }

    #[test]
    fn per_block_encodings_are_recorded_in_the_manifest() {
        let mut exporter = dense(1, 1);
        exporter.set_encoding(ParameterBlock::Thresholds, Q88Encoding::Unsigned);
        exporter.set_encoding(ParameterBlock::DecayRates, Q88Encoding::Unsigned);
        exporter.set_encoding(ParameterBlock::Weights, Q88Encoding::Signed);
        exporter.set_output_weights(vec![vec![-1.0]]);
        exporter.set_encoding(ParameterBlock::Readout, Q88Encoding::Signed);

        assert_eq!(
            exporter.encoding(ParameterBlock::Thresholds),
            Q88Encoding::Unsigned
        );
        assert_eq!(
            exporter.encoding(ParameterBlock::Weights),
            Q88Encoding::Signed
        );

        let params = exporter.try_export().expect("mixed in-memory encodings");
        assert_eq!(params.metadata.encodings.thresholds, Q88Encoding::Unsigned);
        assert_eq!(params.metadata.encodings.weights, Q88Encoding::Signed);
        assert_eq!(params.metadata.encodings.decay_rates, Q88Encoding::Unsigned);
        assert_eq!(
            params.metadata.encodings.output_weights,
            Some(Q88Encoding::Signed)
        );
        assert_eq!(params.output_weights.as_deref(), Some(&[-256][..]));
    }

    #[test]
    fn unsigned_thresholds_and_decay_are_an_explicit_in_memory_mode() {
        let mut exporter = dense(1, 1);
        exporter.set_encoding(ParameterBlock::Thresholds, Q88Encoding::Unsigned);
        exporter.set_encoding(ParameterBlock::DecayRates, Q88Encoding::Unsigned);
        exporter.set_thresholds(vec![200.0]);
        exporter.set_decay_rates(vec![Q88_UNSIGNED_MAX]);

        let params = exporter.try_export().expect("unsigned in-memory blocks");
        assert_eq!(params.thresholds[0] as u16, encode_q88_unsigned(200.0));
        assert_eq!(params.decay_rates[0] as u16, 0xFFFF);
        assert_eq!(params.metadata.encodings.thresholds, Q88Encoding::Unsigned);
        assert_eq!(params.metadata.encodings.decay_rates, Q88Encoding::Unsigned);

        let err = MemFileWriter::write_mem_files(&exporter, tempfile::tempdir().unwrap().path())
            .expect_err("unsigned thresholds must not reach hardware .mem");
        assert!(matches!(
            err,
            ExportError::UnsignedHardwareEncoding {
                block: ParameterBlock::Thresholds,
            }
        ));
    }

    #[test]
    fn older_json_without_encodings_defaults_to_signed() {
        let legacy = r#"{
            "thresholds": [256],
            "weights": [128],
            "decay_rates": [230],
            "metadata": {
                "version": "Spikenaut-v2",
                "timestamp": "2026-01-01T00:00:00Z",
                "num_neurons": 1,
                "num_channels": 1,
                "target_latency_us": 35.0,
                "memory_usage_kb": 0.006
            }
        }"#;
        let parsed: FpgaParameters = serde_json::from_str(legacy).expect("legacy json");
        assert_eq!(parsed.metadata.encodings, BlockEncodings::default());
        assert_eq!(parsed.metadata.encodings.output_weights, None);
    }

    #[test]
    fn metadata_struct_literal_compiles_with_default_encodings() {
        let meta = FpgaMetadata {
            version: EXPORT_FORMAT_VERSION.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            num_neurons: 1,
            num_channels: 1,
            target_latency_us: Some(SPIKENAUT_LEGACY_TARGET_LATENCY_US),
            memory_usage_kb: 0.0,
            ..Default::default()
        };
        assert_eq!(meta.encodings, BlockEncodings::default());
        assert_eq!(meta.encodings.output_weights, None);
    }

    #[test]
    fn legacy_json_with_readout_defaults_encoding_to_signed() {
        let legacy = r#"{
            "thresholds": [256],
            "weights": [128],
            "decay_rates": [230],
            "output_weights": [-256],
            "metadata": {
                "version": "Spikenaut-v2",
                "timestamp": "2026-01-01T00:00:00Z",
                "num_neurons": 1,
                "num_channels": 1,
                "target_latency_us": 35.0,
                "memory_usage_kb": 0.006
            }
        }"#;
        let parsed: FpgaParameters = serde_json::from_str(legacy).expect("legacy readout json");
        assert_eq!(parsed.output_weights.as_deref(), Some(&[-256][..]));
        assert_eq!(
            parsed.metadata.encodings.output_weights,
            Some(Q88Encoding::Signed),
            "None would mean no readout, but the matrix is present"
        );

        let json = serde_json::to_string(&parsed).expect("reserialize");
        let round_tripped: FpgaParameters = serde_json::from_str(&json).expect("reserialized json");
        assert_eq!(
            round_tripped.metadata.encodings.output_weights,
            Some(Q88Encoding::Signed)
        );
        assert!(
            json.contains("\"output_weights\":\"signed\"")
                || json.contains("\"output_weights\": \"signed\""),
            "reserialized manifest must name the readout encoding: {json}"
        );
    }

    #[test]
    fn explicit_readout_encoding_is_not_overwritten_on_deserialize() {
        let json = r#"{
            "thresholds": [256],
            "weights": [128],
            "decay_rates": [230],
            "output_weights": [-256],
            "metadata": {
                "version": "Spikenaut-v2",
                "timestamp": "2026-01-01T00:00:00Z",
                "num_neurons": 1,
                "num_channels": 1,
                "target_latency_us": 35.0,
                "memory_usage_kb": 0.006,
                "encodings": {
                    "thresholds": "signed",
                    "weights": "signed",
                    "decay_rates": "signed",
                    "output_weights": "unsigned"
                }
            }
        }"#;
        let parsed: FpgaParameters = serde_json::from_str(json).expect("explicit unsigned readout");
        assert_eq!(
            parsed.metadata.encodings.output_weights,
            Some(Q88Encoding::Unsigned)
        );
    }

    #[test]
    fn parameter_encoder_saturates_at_full_i16_range() {
        assert_eq!(encode_q88_signed_full(128.0), i16::MAX);
        assert_eq!(encode_q88_signed_full(-129.0), i16::MIN);
        assert_eq!(encode_q88_signed_full(f32::INFINITY), i16::MAX);
        assert_eq!(encode_q88_signed_full(f32::NEG_INFINITY), i16::MIN);
        assert_eq!(encode_q88_signed_full(f32::NAN), 0);
    }
}
