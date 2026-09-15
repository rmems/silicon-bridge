// SPDX-License-Identifier: MIT OR Apache-2.0
//! Export profiles: generic dense Q8.8 versus explicit Spikenaut contracts.
//!
//! The historical writer tagged every bundle `Spikenaut-v2`, stamped a wall
//! clock, and claimed a 35 µs target. Callers who are not deploying a
//! Spikenaut model should use [`ExportConfig::generic`] instead. Spikenaut
//! support stays available as versioned compatibility profiles:
//!
//! | Profile | Schema [`ExportConfig::schema_version`] | Readout | Timestamp | `target_latency_us` | Overwrite |
//! |---|---|---|---|---|---|
//! | [`ExportConfig::generic`] | [`GENERIC_EXPORT_SCHEMA_VERSION`] | optional | omitted unless supplied | omitted unless declared | refuse |
//! | [`ExportConfig::spikenaut_legacy`] | [`EXPORT_FORMAT_VERSION`] (`Spikenaut-v2`) | optional | wall clock unless supplied | 35 µs declared target | replace |
//! | [`ExportConfig::spikenaut_signed_output`] | [`SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION`] | **required** K×N | omitted unless supplied | omitted unless declared | refuse |
//!
//! [`EXPORT_FORMAT_VERSION`] is the historical layout tag. A corrected signed
//! readout contract must **not** reuse that string.
//!
//! ASCII `.mem` files are one uppercase 16-bit hex word per line
//! ([`MemWordFormat::AsciiHexU16`]). That is not UART frame endianness; there
//! is no little/big-endian switch for text hex.
//!
//! ## Migration
//!
//! - Prefer [`FpgaParameterExporter::write_with_config`] with
//!   [`ExportConfig::generic`] for new callers.
//! - [`MemFileWriter::write_mem_files`] remains the Spikenaut-v2
//!   compatibility path: same `parameters*.mem` names, overwrite allowed,
//!   declared 35 µs target. It no longer prints to stdout; it returns an
//!   [`ExportReport`].
//! - [`ParameterExport::export`] / [`CheckedParameterExport::try_export`]
//!   still produce in-memory `Spikenaut-v2` metadata. Overlay a profile with
//!   [`FpgaParameterExporter::try_export_with_config`].

use super::{
    EXPORT_FORMAT_VERSION, ExportError, FpgaMetadata, FpgaParameters, ParameterBlock, Q88Encoding,
    RangePolicy,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Neutral dense-export schema. Not a Spikenaut layout identifier.
pub const GENERIC_EXPORT_SCHEMA_VERSION: &str = "silicon-bridge-dense-v1";

/// Corrected Spikenaut contract with a required signed K×N readout.
///
/// Distinct from [`EXPORT_FORMAT_VERSION`] (`Spikenaut-v2`). Do not treat a
/// `Spikenaut-v2` file as this profile.
pub const SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION: &str = "Spikenaut-signed-output-v1";

/// Profile identity recorded on [`FpgaMetadata::profile`] for the generic path.
pub const GENERIC_PROFILE_ID: &str = "generic";

/// Profile identity for the historical Spikenaut-v2 writer contract.
pub const SPIKENAUT_LEGACY_PROFILE_ID: &str = "spikenaut-legacy";

/// Profile identity for the signed-readout Spikenaut contract.
pub const SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID: &str = "spikenaut-signed-output";

/// Q8.8 total word width in bits.
pub const Q88_TOTAL_BITS: u8 = 16;

/// Q8.8 fractional bits (scale 256).
pub const Q88_FRACTIONAL_BITS: u8 = 8;

/// Historical Spikenaut declared per-tick latency target, in microseconds.
///
/// This is a **declared** design target recorded by
/// [`ExportConfig::spikenaut_legacy`], not a measurement of an export or a
/// board.
pub const SPIKENAUT_DECLARED_TARGET_LATENCY_US: f32 = 35.0;

/// Which dense-export contract a write should follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportProfile {
    /// Framework-agnostic dense Q8.8 bundle. No Spikenaut branding.
    Generic,
    /// Historical `Spikenaut-v2` filenames and metadata.
    SpikenautLegacy,
    /// Signed-output Spikenaut contract; readout is required.
    SpikenautSignedOutput,
}

impl ExportProfile {
    /// Stable identity string stored on [`FpgaMetadata::profile`].
    pub fn id(self) -> &'static str {
        match self {
            Self::Generic => GENERIC_PROFILE_ID,
            Self::SpikenautLegacy => SPIKENAUT_LEGACY_PROFILE_ID,
            Self::SpikenautSignedOutput => SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID,
        }
    }

    /// Schema / layout tag stored on [`FpgaMetadata::version`].
    pub fn schema_version(self) -> &'static str {
        match self {
            Self::Generic => GENERIC_EXPORT_SCHEMA_VERSION,
            Self::SpikenautLegacy => EXPORT_FORMAT_VERSION,
            Self::SpikenautSignedOutput => SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION,
        }
    }

    /// Whether this profile requires a K×N readout matrix.
    pub fn requires_readout(self) -> bool {
        matches!(self, Self::SpikenautSignedOutput)
    }
}

impl fmt::Display for ExportProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Dense matrix flattening. Only row-major is implemented.
///
/// Column-major and sparse layouts are not options on this type. Parse a
/// caller-supplied name with [`MatrixLayout::parse`] — unsupported names are
/// rejected rather than silently rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MatrixLayout {
    /// `addr = row * width + col`. Hidden weights match silicon-hdl
    /// `LifNeuronArray`. Readout is K rows by N hidden columns.
    #[default]
    RowMajor,
}

impl MatrixLayout {
    /// Accept only the implemented row-major names.
    ///
    /// `# Errors`
    ///
    /// [`ExportError::UnsupportedMatrixLayout`] for any other string, including
    /// `column_major`.
    pub fn parse(name: &str) -> Result<Self, ExportError> {
        match name {
            "row_major" | "row-major" | "row_major_dense" => Ok(Self::RowMajor),
            other => Err(ExportError::UnsupportedMatrixLayout {
                requested: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for MatrixLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RowMajor => f.write_str("row_major"),
        }
    }
}

/// Quantization rounding recorded in the export contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RoundingMode {
    /// Truncate toward zero (`trunc(value × 256)`). The Q8.8 encoder.
    #[default]
    TruncateTowardZero,
}

impl fmt::Display for RoundingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncateTowardZero => f.write_str("truncate_toward_zero"),
        }
    }
}

/// Overflow policy recorded in the export contract.
///
/// Mirrors [`RangePolicy`] for the on-disk manifest. Hardware `.mem` writes
/// refuse saturation so a clamp list cannot be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverflowPolicy {
    /// Reject out-of-range finite values.
    #[default]
    Reject,
    /// Clamp to the encoding bounds (in-memory report path only).
    Saturate,
}

impl From<RangePolicy> for OverflowPolicy {
    fn from(policy: RangePolicy) -> Self {
        match policy {
            RangePolicy::Reject => Self::Reject,
            RangePolicy::Saturate => Self::Saturate,
        }
    }
}

impl fmt::Display for OverflowPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reject => f.write_str("reject"),
            Self::Saturate => f.write_str("saturate"),
        }
    }
}

/// How 16-bit Q8.8 words are written to a `.mem` file.
///
/// This is **not** UART endianness. UART frames are raw binary, big-endian.
/// `$readmemh` images are ASCII hex of the 16-bit pattern, one word per line.
/// There is no byte-swap option for that text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemWordFormat {
    /// Four uppercase hex digits of the 16-bit two's-complement pattern.
    #[default]
    AsciiHexU16,
}

impl fmt::Display for MemWordFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AsciiHexU16 => f.write_str("ascii_hex_u16"),
        }
    }
}

/// Dimensions and signedness of one dense parameter block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockShape {
    /// Row count: `N` for hidden vectors/matrices, `K` for readout.
    pub rows: usize,
    /// Column count: `1` for per-neuron vectors, `M` for hidden weights,
    /// `N` for readout.
    pub cols: usize,
    /// Explicit signedness. Not inferred from a filename or `i16` storage.
    pub encoding: Q88Encoding,
    /// Word width in bits. Q8.8 is [`Q88_TOTAL_BITS`].
    pub total_bits: u8,
    /// Fractional bits. Q8.8 is [`Q88_FRACTIONAL_BITS`].
    pub fractional_bits: u8,
    /// Matrix flattening. `None` for a vector (`cols == 1` with no 2-D layout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flattening: Option<MatrixLayout>,
}

impl BlockShape {
    /// Per-neuron vector of `len` values.
    pub fn vector(len: usize, encoding: Q88Encoding) -> Self {
        Self {
            rows: len,
            cols: 1,
            encoding,
            total_bits: Q88_TOTAL_BITS,
            fractional_bits: Q88_FRACTIONAL_BITS,
            flattening: None,
        }
    }

    /// Row-major dense matrix of `rows` × `cols`.
    pub fn matrix(rows: usize, cols: usize, encoding: Q88Encoding) -> Self {
        Self {
            rows,
            cols,
            encoding,
            total_bits: Q88_TOTAL_BITS,
            fractional_bits: Q88_FRACTIONAL_BITS,
            flattening: Some(MatrixLayout::RowMajor),
        }
    }
}

/// Per-block layout recorded beside the Q8.8 vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportLayout {
    /// Threshold vector (`N`).
    pub thresholds: BlockShape,
    /// Hidden weights (`N` × `M`), row-major.
    pub weights: BlockShape,
    /// Decay vector (`N`).
    pub decay_rates: BlockShape,
    /// Optional readout (`K` × `N`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_weights: Option<BlockShape>,
    /// Rounding used to produce the words.
    pub rounding: RoundingMode,
    /// Overflow policy used to produce the words.
    pub overflow: OverflowPolicy,
    /// Hidden and readout matrix flattening.
    pub flattening: MatrixLayout,
    /// `.mem` serialization. Not UART byte order.
    pub mem_word_format: MemWordFormat,
    /// Relative filenames written under the caller-selected directory.
    pub files: WrittenFiles,
}

/// Relative basenames written for one export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrittenFiles {
    /// Threshold `.mem` basename.
    pub thresholds: String,
    /// Hidden-weight `.mem` basename.
    pub weights: String,
    /// Decay `.mem` basename.
    pub decay: String,
    /// Readout `.mem` basename when a readout was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_weights: Option<String>,
    /// Metadata JSON basename.
    pub metadata: String,
}

/// Whether an existing file may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    /// Refuse if any destination already exists. Generic / signed-output default.
    Refuse,
    /// Replace destination files. Legacy [`super::MemFileWriter::write_mem_files`]
    /// behaviour.
    Replace,
}

/// Timestamp recorded on [`FpgaMetadata::timestamp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampSpec {
    /// Omit (empty string, skipped in JSON). Default for generic and
    /// signed-output profiles so repeats are byte-identical.
    Omit,
    /// Caller-supplied RFC 3339 (or any opaque string). Same input → same bytes.
    Explicit(String),
    /// `chrono::Utc::now()`. Legacy Spikenaut-v2 writer default.
    Now,
}

/// Why a configured output name is not a safe relative basename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilenameReason {
    /// Empty string.
    Empty,
    /// Absolute path (`/…`, Windows prefix, and similar).
    Absolute,
    /// `..` component or a path that would leave `output_dir`.
    Traversal,
    /// Not a single path component (separators, `.`, extra directories).
    NotABasename,
}

impl fmt::Display for FilenameReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("filename is empty"),
            Self::Absolute => f.write_str("absolute paths are not allowed"),
            Self::Traversal => {
                f.write_str("path traversal ('..') outside the output directory is not allowed")
            }
            Self::NotABasename => {
                f.write_str("filename must be a single relative basename with no directory")
            }
        }
    }
}

/// Configurable relative filenames for one dense export.
///
/// Construct with [`ExportFileLayout::spikenaut`] (the documented Spikenaut
/// deployment names) or [`ExportFileLayout::new`] after uniqueness / safety
/// checks. There is no plugin filename scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFileLayout {
    thresholds: String,
    weights: String,
    decay: String,
    output_weights: String,
    metadata: String,
}

impl ExportFileLayout {
    /// Documented Spikenaut deployment names:
    /// `parameters.mem`, `parameters_weights.mem`, `parameters_decay.mem`,
    /// `parameters_output_weights.mem`, and `parameters.json`.
    pub fn spikenaut() -> Self {
        Self {
            thresholds: "parameters.mem".into(),
            weights: "parameters_weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            output_weights: "parameters_output_weights.mem".into(),
            metadata: "parameters.json".into(),
        }
    }

    /// Same technical `$readmemh` names as [`Self::spikenaut`].
    ///
    /// The names are RAM-image filenames, not a Spikenaut metadata tag. The
    /// generic profile uses them so a 4×6 layer writes a familiar trio of
    /// `.mem` files without `Spikenaut-v2` in the JSON.
    pub fn generic() -> Self {
        Self::spikenaut()
    }

    /// Caller-selected relative basenames.
    ///
    /// `# Errors`
    ///
    /// [`ExportError::InvalidFilename`] or [`ExportError::DuplicateFilename`].
    pub fn new(
        thresholds: impl Into<String>,
        weights: impl Into<String>,
        decay: impl Into<String>,
        output_weights: impl Into<String>,
        metadata: impl Into<String>,
    ) -> Result<Self, ExportError> {
        let layout = Self {
            thresholds: thresholds.into(),
            weights: weights.into(),
            decay: decay.into(),
            output_weights: output_weights.into(),
            metadata: metadata.into(),
        };
        layout.validate()?;
        Ok(layout)
    }

    /// Threshold `.mem` basename.
    pub fn thresholds(&self) -> &str {
        &self.thresholds
    }

    /// Hidden-weight `.mem` basename.
    pub fn weights(&self) -> &str {
        &self.weights
    }

    /// Decay `.mem` basename.
    pub fn decay(&self) -> &str {
        &self.decay
    }

    /// Readout `.mem` basename (used only when a readout is present).
    pub fn output_weights(&self) -> &str {
        &self.output_weights
    }

    /// Metadata JSON basename.
    pub fn metadata(&self) -> &str {
        &self.metadata
    }

    fn all_names(&self) -> [&str; 5] {
        [
            self.thresholds.as_str(),
            self.weights.as_str(),
            self.decay.as_str(),
            self.output_weights.as_str(),
            self.metadata.as_str(),
        ]
    }

    /// Reject unsafe or colliding names before any file is created.
    pub fn validate(&self) -> Result<(), ExportError> {
        for name in self.all_names() {
            validate_basename(name)?;
        }
        let names = self.all_names();
        for (i, a) in names.iter().enumerate() {
            for b in names.iter().skip(i + 1) {
                if names_collide(a, b) {
                    return Err(ExportError::DuplicateFilename { name: (*a).into() });
                }
            }
        }
        Ok(())
    }
}

impl Default for ExportFileLayout {
    fn default() -> Self {
        Self::generic()
    }
}

/// Why two configured names cannot share an output path.
fn names_collide(a: &str, b: &str) -> bool {
    a == b || a.eq_ignore_ascii_case(b)
}

/// Reject absolute paths, `..` traversal, and anything that is not one basename.
pub(super) fn validate_basename(name: &str) -> Result<(), ExportError> {
    if name.is_empty() {
        return Err(ExportError::InvalidFilename {
            name: name.into(),
            reason: FilenameReason::Empty,
        });
    }
    if name.contains('\0') || name.contains('/') || name.contains('\\') {
        let reason = if name.starts_with('/') || looks_absolute(name) {
            FilenameReason::Absolute
        } else if name.contains("..") {
            FilenameReason::Traversal
        } else {
            FilenameReason::NotABasename
        };
        return Err(ExportError::InvalidFilename {
            name: name.into(),
            reason,
        });
    }

    let path = Path::new(name);
    if path.is_absolute() || looks_absolute(name) {
        return Err(ExportError::InvalidFilename {
            name: name.into(),
            reason: FilenameReason::Absolute,
        });
    }

    let mut components = path.components();
    let first = components.next();
    if components.next().is_some() {
        return Err(ExportError::InvalidFilename {
            name: name.into(),
            reason: FilenameReason::NotABasename,
        });
    }
    match first {
        Some(Component::Normal(_)) if name != "." && name != ".." => Ok(()),
        Some(Component::ParentDir) | Some(Component::CurDir) => Err(ExportError::InvalidFilename {
            name: name.into(),
            reason: FilenameReason::Traversal,
        }),
        Some(Component::RootDir) | Some(Component::Prefix(_)) => {
            Err(ExportError::InvalidFilename {
                name: name.into(),
                reason: FilenameReason::Absolute,
            })
        }
        _ => Err(ExportError::InvalidFilename {
            name: name.into(),
            reason: FilenameReason::NotABasename,
        }),
    }
}

fn looks_absolute(name: &str) -> bool {
    name.starts_with('/')
        || name.starts_with('\\')
        || (name.len() >= 2
            && name.as_bytes()[0].is_ascii_alphabetic()
            && name.as_bytes()[1] == b':')
}

/// Writer configuration: profile, files, timestamp, overwrite, optional claims.
///
/// This is a small dense-block config (thresholds, weights, decay, optional
/// readout), not a general tensor serializer.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportConfig {
    profile: ExportProfile,
    files: ExportFileLayout,
    timestamp: TimestampSpec,
    target_latency_us: Option<f32>,
    overwrite: OverwritePolicy,
    model_id: Option<String>,
}

impl ExportConfig {
    /// Neutral dense export: no Spikenaut tag, no wall clock, no timing claim,
    /// overwrite refused.
    pub fn generic() -> Self {
        Self {
            profile: ExportProfile::Generic,
            files: ExportFileLayout::generic(),
            timestamp: TimestampSpec::Omit,
            target_latency_us: None,
            overwrite: OverwritePolicy::Refuse,
            model_id: None,
        }
    }

    /// Historical Spikenaut-v2 writer: documented filenames, wall-clock
    /// timestamp, declared 35 µs target, overwrite allowed.
    pub fn spikenaut_legacy() -> Self {
        Self {
            profile: ExportProfile::SpikenautLegacy,
            files: ExportFileLayout::spikenaut(),
            timestamp: TimestampSpec::Now,
            target_latency_us: Some(SPIKENAUT_DECLARED_TARGET_LATENCY_US),
            overwrite: OverwritePolicy::Replace,
            model_id: None,
        }
    }

    /// Corrected Spikenaut signed-output contract.
    ///
    /// Requires a K×N readout. Uses [`SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION`],
    /// not `Spikenaut-v2`. Overwrite refused; timestamp omitted unless supplied.
    pub fn spikenaut_signed_output() -> Self {
        Self {
            profile: ExportProfile::SpikenautSignedOutput,
            files: ExportFileLayout::spikenaut(),
            timestamp: TimestampSpec::Omit,
            target_latency_us: None,
            overwrite: OverwritePolicy::Refuse,
            model_id: None,
        }
    }

    /// Selected profile.
    pub fn profile(&self) -> ExportProfile {
        self.profile
    }

    /// Schema tag this config will write to [`FpgaMetadata::version`].
    pub fn schema_version(&self) -> &'static str {
        self.profile.schema_version()
    }

    /// File layout to write.
    pub fn files(&self) -> &ExportFileLayout {
        &self.files
    }

    /// Timestamp policy.
    pub fn timestamp(&self) -> &TimestampSpec {
        &self.timestamp
    }

    /// Declared target latency, if any. Never a measured value.
    pub fn target_latency_us(&self) -> Option<f32> {
        self.target_latency_us
    }

    /// Overwrite policy.
    pub fn overwrite(&self) -> OverwritePolicy {
        self.overwrite
    }

    /// Optional caller model identity. Distinct from schema and crate version.
    pub fn model_id(&self) -> Option<&str> {
        self.model_id.as_deref()
    }

    /// Replace the file layout. Validates uniqueness and basenames.
    ///
    /// `# Errors`
    ///
    /// Propagates [`ExportFileLayout::validate`].
    pub fn with_files(mut self, files: ExportFileLayout) -> Result<Self, ExportError> {
        files.validate()?;
        self.files = files;
        Ok(self)
    }

    /// Caller-supplied timestamp (opt-in). Repeating the same string keeps
    /// JSON byte-identical.
    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = TimestampSpec::Explicit(timestamp.into());
        self
    }

    /// Do not record a timestamp.
    pub fn omit_timestamp(mut self) -> Self {
        self.timestamp = TimestampSpec::Omit;
        self
    }

    /// Record a **declared** target latency. This crate does not measure it.
    pub fn with_target_latency_us(mut self, micros: f32) -> Self {
        self.target_latency_us = Some(micros);
        self
    }

    /// Allow replacing existing destination files.
    pub fn allow_overwrite(mut self) -> Self {
        self.overwrite = OverwritePolicy::Replace;
        self
    }

    /// Refuse if any destination already exists.
    pub fn refuse_overwrite(mut self) -> Self {
        self.overwrite = OverwritePolicy::Refuse;
        self
    }

    /// Caller model / network identity. Distinct from profile and schema.
    pub fn with_model_id(mut self, id: impl Into<String>) -> Self {
        self.model_id = Some(id.into());
        self
    }

    /// Request a flattening by name. Only row-major is implemented.
    ///
    /// `# Errors`
    ///
    /// [`ExportError::UnsupportedMatrixLayout`].
    pub fn with_matrix_layout_name(self, name: &str) -> Result<Self, ExportError> {
        let _layout = MatrixLayout::parse(name)?;
        Ok(self)
    }
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self::generic()
    }
}

/// Result of a successful checked write. The core writer returns this instead
/// of printing to stdout.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    /// Directory that received the files.
    pub output_dir: PathBuf,
    /// Profile that produced the bundle.
    pub profile: ExportProfile,
    /// Schema tag written to metadata.
    pub schema_version: String,
    /// Files created or replaced, in write order.
    pub files_written: Vec<PathBuf>,
    /// Files removed (legacy leftover readout only).
    pub files_removed: Vec<PathBuf>,
    /// Whether existing destinations were replaced.
    pub replaced_existing: bool,
    /// Metadata that was serialized.
    pub metadata: FpgaMetadata,
}

impl ExportReport {
    /// Basenames written, for tests and summaries.
    pub fn written_basenames(&self) -> Vec<String> {
        self.files_written
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }
}

impl fmt::Display for ExportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "FPGA parameter export")?;
        writeln!(f, "  directory: {}", self.output_dir.display())?;
        writeln!(f, "  profile: {}", self.profile)?;
        writeln!(f, "  schema: {}", self.schema_version)?;
        writeln!(
            f,
            "  neurons: {}  channels: {}",
            self.metadata.num_neurons, self.metadata.num_channels
        )?;
        writeln!(f, "  files:")?;
        for path in &self.files_written {
            writeln!(f, "    {}", path.display())?;
        }
        Ok(())
    }
}

pub(super) fn resolve_timestamp(spec: &TimestampSpec) -> String {
    match spec {
        TimestampSpec::Omit => String::new(),
        TimestampSpec::Explicit(value) => value.clone(),
        TimestampSpec::Now => chrono::Utc::now().to_rfc3339(),
    }
}

pub(super) fn producer_crate_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

pub(super) fn layout_for_params(
    params: &FpgaParameters,
    files: &ExportFileLayout,
    overflow: OverflowPolicy,
    include_readout_file: bool,
) -> ExportLayout {
    let encodings = params.metadata.encodings;
    let n = params.metadata.num_neurons;
    let m = params.metadata.num_channels;
    let readout = params.output_weights.as_ref().map(|words| {
        let cols = n;
        let rows = words.len().checked_div(cols).unwrap_or(0);
        BlockShape::matrix(
            rows,
            cols,
            encodings.output_weights.unwrap_or(Q88Encoding::Signed),
        )
    });
    ExportLayout {
        thresholds: BlockShape::vector(n, encodings.thresholds),
        weights: BlockShape::matrix(n, m, encodings.weights),
        decay_rates: BlockShape::vector(n, encodings.decay_rates),
        output_weights: readout,
        rounding: RoundingMode::TruncateTowardZero,
        overflow,
        flattening: MatrixLayout::RowMajor,
        mem_word_format: MemWordFormat::AsciiHexU16,
        files: WrittenFiles {
            thresholds: files.thresholds().to_string(),
            weights: files.weights().to_string(),
            decay: files.decay().to_string(),
            output_weights: include_readout_file.then(|| files.output_weights().to_string()),
            metadata: files.metadata().to_string(),
        },
    }
}

pub(super) fn destinations(
    output_dir: &Path,
    files: &ExportFileLayout,
    write_readout: bool,
) -> Result<Vec<(ParameterBlock, PathBuf)>, ExportError> {
    files.validate()?;
    let mut paths = vec![
        (
            ParameterBlock::Thresholds,
            output_dir.join(files.thresholds()),
        ),
        (ParameterBlock::Weights, output_dir.join(files.weights())),
        (ParameterBlock::DecayRates, output_dir.join(files.decay())),
    ];
    if write_readout {
        paths.push((
            ParameterBlock::Readout,
            output_dir.join(files.output_weights()),
        ));
    }
    Ok(paths)
}

pub(super) fn metadata_json_path(output_dir: &Path, files: &ExportFileLayout) -> PathBuf {
    output_dir.join(files.metadata())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CheckedParameterExport, FpgaParameterExporter, MemFileWriter, ParameterShapeError,
        Q88_SIGNED_MAX,
    };
    use std::fs;

    fn dense_4x6() -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(
            vec![1.0, 0.5, 1.5, 0.75],
            vec![
                vec![0.5, -1.0, 0.25, 1.0, -0.5, 0.0],
                vec![-128.0, Q88_SIGNED_MAX, 1.0 / 256.0, -1.0 / 256.0, 2.0, -2.0],
                vec![1.0; 6],
                vec![-0.5, 0.5, -0.5, 0.5, -0.5, 0.5],
            ],
            vec![0.5, 0.75, 0.25, 1.0],
        )
    }

    fn dense_2x2() -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(
            vec![1.0, 0.75],
            vec![vec![0.5, 2.0], vec![0.25, 1.5]],
            vec![0.5, 0.75],
        )
    }

    fn with_readout() -> FpgaParameterExporter {
        let mut exporter = dense_2x2();
        exporter.set_output_weights(vec![vec![-1.0, 0.5], vec![0.25, 1.0]]);
        exporter
    }

    #[test]
    fn generic_4x6_has_no_spikenaut_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect("generic 4×6");

        assert_eq!(report.profile, ExportProfile::Generic);
        assert_eq!(report.schema_version, GENERIC_EXPORT_SCHEMA_VERSION);
        assert_eq!(report.metadata.version, GENERIC_EXPORT_SCHEMA_VERSION);
        assert_eq!(report.metadata.profile, GENERIC_PROFILE_ID);
        assert_eq!(
            report.metadata.producer_crate_version,
            env!("CARGO_PKG_VERSION")
        );
        assert!(report.metadata.timestamp.is_empty());
        assert_eq!(report.metadata.target_latency_us, None);
        assert_eq!(report.metadata.num_neurons, 4);
        assert_eq!(report.metadata.num_channels, 6);
        assert!(report.metadata.layout.is_some());
        let layout = report.metadata.layout.as_ref().expect("layout");
        assert_eq!(layout.weights.rows, 4);
        assert_eq!(layout.weights.cols, 6);
        assert_eq!(layout.weights.encoding, Q88Encoding::Signed);
        assert_eq!(layout.flattening, MatrixLayout::RowMajor);
        assert_eq!(layout.mem_word_format, MemWordFormat::AsciiHexU16);
        assert_eq!(layout.output_weights, None);

        let json = fs::read_to_string(dir.path().join("parameters.json")).expect("json");
        assert!(
            !json.contains("Spikenaut"),
            "generic metadata must not mention Spikenaut: {json}"
        );
        assert!(
            !json.contains("target_latency"),
            "generic metadata must not invent a latency claim: {json}"
        );
        assert_eq!(json.matches("silicon-bridge-dense-v1").count(), 1);
    }

    #[test]
    fn generic_repeat_is_byte_identical() {
        let dir1 = tempfile::tempdir().expect("tempdir");
        let dir2 = tempfile::tempdir().expect("tempdir");
        let config = ExportConfig::generic().with_model_id("fixture-4x6");
        dense_4x6()
            .write_with_config(dir1.path(), &config)
            .expect("first");
        dense_4x6()
            .write_with_config(dir2.path(), &config)
            .expect("second");

        for name in [
            "parameters.mem",
            "parameters_weights.mem",
            "parameters_decay.mem",
            "parameters.json",
        ] {
            let a = fs::read(dir1.path().join(name)).expect(name);
            let b = fs::read(dir2.path().join(name)).expect(name);
            assert_eq!(a, b, "{name} must be byte-identical across repeats");
        }
    }

    #[test]
    fn caller_supplied_timestamp_is_deterministic() {
        let dir1 = tempfile::tempdir().expect("tempdir");
        let dir2 = tempfile::tempdir().expect("tempdir");
        let config = ExportConfig::generic().with_timestamp("2026-09-15T00:00:00Z");
        dense_2x2()
            .write_with_config(dir1.path(), &config)
            .expect("first");
        dense_2x2()
            .write_with_config(dir2.path(), &config)
            .expect("second");
        assert_eq!(
            fs::read(dir1.path().join("parameters.json")).unwrap(),
            fs::read(dir2.path().join("parameters.json")).unwrap()
        );
        let json = fs::read_to_string(dir1.path().join("parameters.json")).unwrap();
        assert!(json.contains("2026-09-15T00:00:00Z"));
    }

    #[test]
    fn legacy_profile_keeps_documented_filenames_and_spikenaut_v2() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = dense_2x2()
            .write_with_config(dir.path(), &ExportConfig::spikenaut_legacy())
            .expect("legacy");
        assert_eq!(report.schema_version, EXPORT_FORMAT_VERSION);
        assert_eq!(report.metadata.version, EXPORT_FORMAT_VERSION);
        assert_eq!(report.metadata.profile, SPIKENAUT_LEGACY_PROFILE_ID);
        assert_eq!(
            report.metadata.target_latency_us,
            Some(SPIKENAUT_DECLARED_TARGET_LATENCY_US)
        );
        assert!(!report.metadata.timestamp.is_empty());
        let mut names = report.written_basenames();
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
        assert_eq!(
            fs::read_to_string(dir.path().join("parameters.mem")).unwrap(),
            "0100\n00C0\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("parameters_weights.mem")).unwrap(),
            "0080\n0200\n0040\n0180\n"
        );
    }

    #[test]
    fn mem_file_writer_is_the_legacy_profile_and_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report =
            MemFileWriter::write_mem_files(&dense_2x2(), dir.path()).expect("legacy write");
        assert_eq!(report.profile, ExportProfile::SpikenautLegacy);
        assert_eq!(report.metadata.version, EXPORT_FORMAT_VERSION);
        assert!(dir.path().join("parameters.mem").exists());
    }

    #[test]
    fn signed_output_profile_records_kx_n_readout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = with_readout()
            .write_with_config(dir.path(), &ExportConfig::spikenaut_signed_output())
            .expect("signed-output");
        assert_eq!(
            report.schema_version,
            SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION
        );
        assert_ne!(report.schema_version, EXPORT_FORMAT_VERSION);
        let layout = report.metadata.layout.expect("layout");
        let readout = layout.output_weights.expect("K×N");
        assert_eq!(readout.rows, 2);
        assert_eq!(readout.cols, 2);
        assert_eq!(readout.encoding, Q88Encoding::Signed);
        assert!(dir.path().join("parameters_output_weights.mem").exists());
        let json = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(json.contains("Spikenaut-signed-output-v1"));
        assert!(!json.contains(&format!("\"version\": \"{EXPORT_FORMAT_VERSION}\"")));
    }

    #[test]
    fn signed_output_profile_rejects_missing_readout() {
        let err = dense_2x2()
            .try_export_with_config(&ExportConfig::spikenaut_signed_output())
            .expect_err("readout required");
        assert_eq!(err, ParameterShapeError::ReadoutRequired);
        let dir = tempfile::tempdir().expect("tempdir");
        let write_err = dense_2x2()
            .write_with_config(dir.path(), &ExportConfig::spikenaut_signed_output())
            .expect_err("must not write");
        assert!(matches!(
            write_err,
            ExportError::InvalidParameters(ParameterShapeError::ReadoutRequired)
        ));
        assert!(
            !dir.path().join("parameters.mem").exists(),
            "missing readout must not create files"
        );
    }

    #[test]
    fn rejects_absolute_and_traversal_filenames() {
        for (name, reason) in [
            ("", FilenameReason::Empty),
            ("/tmp/parameters.mem", FilenameReason::Absolute),
            ("C:\\parameters.mem", FilenameReason::Absolute),
            ("../parameters.mem", FilenameReason::Traversal),
            ("foo/parameters.mem", FilenameReason::NotABasename),
            ("..", FilenameReason::Traversal),
        ] {
            let err = validate_basename(name).expect_err(name);
            match err {
                ExportError::InvalidFilename {
                    name: got,
                    reason: got_reason,
                } => {
                    assert_eq!(got, name);
                    assert_eq!(got_reason, reason, "for {name}");
                }
                other => panic!("expected InvalidFilename for {name}, got {other}"),
            }
        }
    }

    #[test]
    fn rejects_colliding_filenames_before_write() {
        let err = ExportFileLayout::new(
            "parameters.mem",
            "parameters.mem",
            "parameters_decay.mem",
            "parameters_output_weights.mem",
            "parameters.json",
        )
        .expect_err("duplicate");
        assert!(matches!(err, ExportError::DuplicateFilename { .. }));

        let case = ExportFileLayout::new(
            "parameters.mem",
            "Parameters.mem",
            "parameters_decay.mem",
            "parameters_output_weights.mem",
            "parameters.json",
        )
        .expect_err("case collision");
        assert!(matches!(case, ExportError::DuplicateFilename { .. }));
    }

    #[test]
    fn refuses_overwrite_on_generic_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let seed = dir.path().join("parameters.mem");
        fs::write(&seed, b"KEEP\n").expect("seed");
        let err = dense_2x2()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect_err("overwrite refused");
        match err {
            ExportError::OverwriteRefused { path } => {
                assert_eq!(path, seed);
            }
            other => panic!("expected OverwriteRefused, got {other}"),
        }
        assert_eq!(fs::read(&seed).unwrap(), b"KEEP\n");
        assert!(
            !dir.path().join("parameters_weights.mem").exists(),
            "refused overwrite must not write sibling files"
        );
    }

    #[test]
    fn explicit_overwrite_replaces_generic_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("parameters.mem"), b"OLD\n").unwrap();
        dense_2x2()
            .write_with_config(dir.path(), &ExportConfig::generic().allow_overwrite())
            .expect("replace");
        assert_eq!(
            fs::read_to_string(dir.path().join("parameters.mem")).unwrap(),
            "0100\n00C0\n"
        );
    }

    #[test]
    fn custom_layout_writes_configured_names_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let files =
            ExportFileLayout::new("thr.mem", "w.mem", "decay.mem", "readout.mem", "meta.json")
                .expect("layout");
        let config = ExportConfig::generic().with_files(files).expect("config");
        dense_2x2()
            .write_with_config(dir.path(), &config)
            .expect("write");
        assert!(dir.path().join("thr.mem").exists());
        assert!(dir.path().join("w.mem").exists());
        assert!(dir.path().join("decay.mem").exists());
        assert!(dir.path().join("meta.json").exists());
        assert!(!dir.path().join("parameters.mem").exists());
        assert!(!dir.path().join("readout.mem").exists());
    }

    #[test]
    fn column_major_is_rejected_not_exposed() {
        let err = MatrixLayout::parse("column_major").expect_err("unsupported");
        assert!(matches!(err, ExportError::UnsupportedMatrixLayout { .. }));
        ExportConfig::generic()
            .with_matrix_layout_name("column_major")
            .expect_err("unsupported layout");
        assert_eq!(
            MatrixLayout::parse("row_major").unwrap(),
            MatrixLayout::RowMajor
        );
    }

    #[test]
    fn generic_declared_latency_is_opt_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = dense_2x2()
            .write_with_config(
                dir.path(),
                &ExportConfig::generic().with_target_latency_us(20.0),
            )
            .expect("declared target");
        assert_eq!(report.metadata.target_latency_us, Some(20.0));
        let json = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(json.contains("\"target_latency_us\": 20.0"));
    }

    #[test]
    fn in_memory_try_export_stays_on_the_legacy_tag() {
        let params = CheckedParameterExport::try_export(&dense_2x2()).expect("legacy memory");
        assert_eq!(params.metadata.version, EXPORT_FORMAT_VERSION);
        assert_eq!(
            params.metadata.target_latency_us,
            Some(SPIKENAUT_DECLARED_TARGET_LATENCY_US)
        );
    }

    #[test]
    fn schema_profile_and_crate_version_are_separate_fields() {
        let params = dense_2x2()
            .try_export_with_config(&ExportConfig::generic().with_model_id("net-a"))
            .expect("generic memory");
        assert_eq!(params.metadata.version, GENERIC_EXPORT_SCHEMA_VERSION);
        assert_eq!(params.metadata.profile, GENERIC_PROFILE_ID);
        assert_eq!(
            params.metadata.producer_crate_version,
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(params.metadata.model_id.as_deref(), Some("net-a"));
        assert_ne!(params.metadata.version, params.metadata.profile);
        assert_ne!(
            params.metadata.version,
            params.metadata.producer_crate_version
        );
    }
}
