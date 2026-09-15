// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dense-parameter export profiles: generic Q8.8, legacy Spikenaut-v2, and a
//! distinct signed-output Spikenaut contract.
//!
//! This is a small configuration layer on top of [`super::CheckedParameterExport`]
//! and [`super::Q88Encoding`]. It is not a plugin or tensor-serialization
//! framework. ASCII `.mem` word order (one uppercase 16-bit hex pattern per
//! line) is independent of UART frame byte order; there is no endianness
//! switch for text hex.

use super::{
    CheckedParameterExport, EXPORT_FORMAT_VERSION, ExportError, FilenameError, FpgaMetadata,
    FpgaParameterExporter, FpgaParameters, OverflowPolicy, Q88Encoding, QFormat, RangePolicy,
    RoundingMode,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// Crate name recorded as the producer of a profiled export.
pub const PRODUCER_CRATE: &str = "silicon-bridge";

/// Profile identity for a framework-agnostic dense Q8.8 bundle.
pub const GENERIC_DENSE_PROFILE_ID: &str = "generic-dense-q88";

/// Schema version for [`ExportProfile::GenericDenseQ88`] metadata.
///
/// Distinct from [`EXPORT_FORMAT_VERSION`] (`Spikenaut-v2`), the producer
/// crate version, and any model id the caller supplies.
pub const GENERIC_DENSE_SCHEMA_VERSION: &str = "silicon-bridge-dense-q88-v1";

/// Profile identity for the historical Spikenaut deployment filenames and
/// `Spikenaut-v2` layout tag. Compatibility only.
pub const SPIKENAUT_V2_LEGACY_PROFILE_ID: &str = "spikenaut-v2-legacy";

/// Profile identity for the corrected signed-output Spikenaut contract.
///
/// Distinct from historical [`EXPORT_FORMAT_VERSION`]. This tag is **not** a
/// silent redefinition of `Spikenaut-v2`.
pub const SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID: &str = "spikenaut-signed-output-v1";

/// Schema version for [`ExportProfile::SpikenautSignedOutputV1`].
pub const SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION: &str = "spikenaut-signed-output-v1";

/// Historical per-tick design target recorded by the legacy Spikenaut-v2
/// writer, in microseconds. A declared target, never a measured latency.
pub const SPIKENAUT_LEGACY_TARGET_LATENCY_US: f32 = 35.0;

/// Named dense-parameter export contract.
///
/// [`ExportProfile::GenericDenseQ88`] is the framework-agnostic path.
/// [`ExportProfile::LegacySpikenautV2`] preserves the historical tag,
/// filenames, overwrite behaviour, and declared 35 µs target.
/// [`ExportProfile::SpikenautSignedOutputV1`] is the corrected signed-readout
/// contract and **must not** be serialized as `Spikenaut-v2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ExportProfile {
    /// Neutral dense Q8.8 bundle: no Spikenaut branding, no invented timing,
    /// timestamps omitted unless the caller supplies one.
    GenericDenseQ88,
    /// Historical Spikenaut-v2 compatibility profile.
    LegacySpikenautV2,
    /// Corrected Spikenaut contract that requires a signed `K×N` readout.
    SpikenautSignedOutputV1,
}

impl ExportProfile {
    /// Stable profile identity string, distinct from schema and crate version.
    pub fn id(self) -> &'static str {
        match self {
            Self::GenericDenseQ88 => GENERIC_DENSE_PROFILE_ID,
            Self::LegacySpikenautV2 => SPIKENAUT_V2_LEGACY_PROFILE_ID,
            Self::SpikenautSignedOutputV1 => SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID,
        }
    }

    /// Schema of the metadata document this profile writes.
    pub fn schema_version(self) -> &'static str {
        match self {
            Self::GenericDenseQ88 => GENERIC_DENSE_SCHEMA_VERSION,
            // Historical tag is the schema for the compatibility profile.
            Self::LegacySpikenautV2 => EXPORT_FORMAT_VERSION,
            Self::SpikenautSignedOutputV1 => SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION,
        }
    }

    /// Whether this profile requires a readout / output-weight matrix.
    pub fn requires_readout(self) -> bool {
        matches!(self, Self::SpikenautSignedOutputV1)
    }
}

/// Whether an existing file in the output directory may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    /// Refuse if any configured output file already exists. Default for the
    /// generic path. Validation happens **before** any truncate.
    Prohibit,
    /// Replace existing files. [`super::MemFileWriter::write_mem_files`] uses
    /// this so the historical writer keeps overwriting.
    Replace,
}

/// How the export timestamp is produced.
///
/// The generic path defaults to [`TimestampPolicy::Omit`]. The same input,
/// configuration, and metadata therefore yield byte-identical JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampPolicy {
    /// Do not record a timestamp (generic default).
    Omit,
    /// Caller-supplied stamp, copied verbatim so repeats stay deterministic.
    Supplied(String),
    /// `chrono::Utc::now()` RFC 3339. Used by the legacy writer. Not
    /// deterministic.
    Now,
}

/// Configurable relative basenames for a dense export.
///
/// Only single-component relative names are accepted. Absolute paths and
/// `..` traversal are rejected. The Spikenaut deployment profile uses
/// `parameters.mem`, `parameters_weights.mem`, `parameters_decay.mem`,
/// optional `parameters_output_weights.mem`, and `parameters.json`. Matching
/// names do **not** imply HDL compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFileLayout {
    /// Threshold vector image (`N` words).
    pub thresholds: String,
    /// Hidden-weight matrix image (`N×M` words, row-major).
    pub weights: String,
    /// Decay-rate vector image (`N` words).
    pub decay: String,
    /// Metadata JSON (vectors plus [`FpgaMetadata`]).
    pub metadata: String,
    /// Optional readout image. `None` means “do not write a readout file”.
    pub output_weights: Option<String>,
}

impl ExportFileLayout {
    /// Documented Spikenaut deployment filenames, including the optional
    /// readout name used when a `K×N` matrix is present.
    pub fn spikenaut_deployment() -> Self {
        Self {
            thresholds: "parameters.mem".into(),
            weights: "parameters_weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: Some("parameters_output_weights.mem".into()),
        }
    }

    /// Default generic layout. Same practical basenames as the Spikenaut
    /// deployment profile (they are descriptive, not branded); callers may
    /// replace them through [`ExportConfig::with_files`].
    pub fn generic_default() -> Self {
        Self::spikenaut_deployment()
    }

    /// Configured names that will actually be written for this bundle.
    fn names_for(&self, has_readout: bool) -> Vec<&str> {
        let mut names = vec![
            self.thresholds.as_str(),
            self.weights.as_str(),
            self.decay.as_str(),
            self.metadata.as_str(),
        ];
        if has_readout && let Some(name) = self.output_weights.as_deref() {
            names.push(name);
        }
        names
    }

    /// Reject unsafe or colliding names before any filesystem write.
    pub fn validate(&self) -> Result<(), ExportError> {
        self.validate_for(true)
    }

    fn validate_for(&self, include_optional_readout_name: bool) -> Result<(), ExportError> {
        let mut seen = BTreeSet::new();
        let mut names = vec![
            self.thresholds.as_str(),
            self.weights.as_str(),
            self.decay.as_str(),
            self.metadata.as_str(),
        ];
        if include_optional_readout_name && let Some(name) = self.output_weights.as_deref() {
            names.push(name);
        }
        for name in names {
            validate_basename(name)?;
            if !seen.insert(name) {
                return Err(ExportError::DuplicateFilename {
                    name: name.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Writer configuration for one dense export.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportConfig {
    profile: ExportProfile,
    files: ExportFileLayout,
    overwrite: OverwritePolicy,
    timestamp: TimestampPolicy,
    declared_target_latency_us: Option<f32>,
    model_id: String,
}

impl ExportConfig {
    /// Neutral dense Q8.8 export: no Spikenaut tag, no timestamp, no declared
    /// latency, overwrite prohibited until [`Self::allow_replace`].
    ///
    /// ```
    /// use silicon_bridge::{ExportConfig, FpgaParameterExporter};
    ///
    /// let exporter = FpgaParameterExporter::from_params(
    ///     vec![1.0, 0.5, 1.5, 0.75],
    ///     vec![vec![0.5; 6]; 4],
    ///     vec![0.9; 4],
    /// );
    /// let dir = tempfile::tempdir().unwrap();
    /// let report = exporter
    ///     .write_with_config(dir.path(), &ExportConfig::generic())
    ///     .unwrap();
    /// assert_eq!(report.num_neurons, 4);
    /// assert_eq!(report.num_channels, 6);
    /// let json = std::fs::read_to_string(dir.path().join("parameters.json")).unwrap();
    /// assert!(!json.contains("Spikenaut"));
    /// ```
    pub fn generic() -> Self {
        Self {
            profile: ExportProfile::GenericDenseQ88,
            files: ExportFileLayout::generic_default(),
            overwrite: OverwritePolicy::Prohibit,
            timestamp: TimestampPolicy::Omit,
            declared_target_latency_us: None,
            model_id: String::new(),
        }
    }

    /// Historical Spikenaut-v2 writer: documented filenames, wall-clock
    /// timestamp, declared 35 µs target, and replacement of existing files.
    ///
    /// This is the compatibility default used by
    /// [`super::MemFileWriter::write_mem_files`].
    pub fn legacy_spikenaut_v2() -> Self {
        Self {
            profile: ExportProfile::LegacySpikenautV2,
            files: ExportFileLayout::spikenaut_deployment(),
            overwrite: OverwritePolicy::Replace,
            timestamp: TimestampPolicy::Now,
            declared_target_latency_us: Some(SPIKENAUT_LEGACY_TARGET_LATENCY_US),
            model_id: String::new(),
        }
    }

    /// Corrected signed-output Spikenaut contract. Requires a `K×N` readout.
    /// Distinct schema/profile ids — does not emit `Spikenaut-v2`.
    ///
    /// Overwrite is prohibited by default (same as the generic path).
    pub fn spikenaut_signed_output_v1() -> Self {
        Self {
            profile: ExportProfile::SpikenautSignedOutputV1,
            files: ExportFileLayout::spikenaut_deployment(),
            overwrite: OverwritePolicy::Prohibit,
            timestamp: TimestampPolicy::Omit,
            declared_target_latency_us: None,
            model_id: String::new(),
        }
    }

    /// Profile this configuration will write.
    pub fn profile(&self) -> ExportProfile {
        self.profile
    }

    /// File layout this configuration will write.
    pub fn files(&self) -> &ExportFileLayout {
        &self.files
    }

    /// Overwrite policy.
    pub fn overwrite(&self) -> OverwritePolicy {
        self.overwrite
    }

    /// Timestamp policy.
    pub fn timestamp(&self) -> &TimestampPolicy {
        &self.timestamp
    }

    /// Declared per-tick latency target in microseconds, if any.
    ///
    /// Never a measured latency. The generic profile omits this unless the
    /// caller sets it with [`Self::with_declared_target_latency_us`].
    pub fn declared_target_latency_us(&self) -> Option<f32> {
        self.declared_target_latency_us
    }

    /// Optional caller-supplied model identity, distinct from profile and
    /// schema version.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Replace the file layout after validating uniqueness and basenames.
    pub fn with_files(mut self, files: ExportFileLayout) -> Result<Self, ExportError> {
        files.validate()?;
        self.files = files;
        Ok(self)
    }

    /// Explicit consent to replace existing files in the output directory.
    pub fn allow_replace(mut self) -> Self {
        self.overwrite = OverwritePolicy::Replace;
        self
    }

    /// Keep existing files; refuse the write if any target already exists.
    pub fn prohibit_overwrite(mut self) -> Self {
        self.overwrite = OverwritePolicy::Prohibit;
        self
    }

    /// Omit the timestamp so repeats stay byte-identical.
    pub fn without_timestamp(mut self) -> Self {
        self.timestamp = TimestampPolicy::Omit;
        self
    }

    /// Pin the timestamp to a caller-supplied string (copied verbatim).
    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = TimestampPolicy::Supplied(timestamp.into());
        self
    }

    /// Record a **declared** per-tick latency target. Not a measurement.
    pub fn with_declared_target_latency_us(mut self, microseconds: f32) -> Self {
        self.declared_target_latency_us = Some(microseconds);
        self
    }

    /// Record a caller-supplied model identity. Distinct from profile id and
    /// schema version.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }
}

/// Files written by a profiled export. Returned instead of printing.
///
/// This report does not claim board compatibility, measured latency, or HDL
/// correctness. Matching documented filenames is not proof that silicon-hdl
/// RAM will consume the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Profile that produced the files.
    pub profile: ExportProfile,
    /// Directory the files were written under.
    pub output_dir: PathBuf,
    /// Basenames actually written, in a stable order (thresholds, weights,
    /// decay, optional readout, metadata).
    pub written: Vec<String>,
    /// Whether replacement of existing files was permitted for this write.
    pub overwrite: OverwritePolicy,
    /// Hidden-layer neuron count `N`.
    pub num_neurons: usize,
    /// Hidden-layer input count `M`.
    pub num_channels: usize,
    /// Readout shape `K×N` when a readout file was written.
    pub readout_shape: Option<ReadoutShape>,
}

/// `K` output rows by `N` hidden columns for a readout matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadoutShape {
    /// Output / class count `K`.
    pub rows: usize,
    /// Hidden-neuron count `N`.
    pub cols: usize,
}

/// Per-block dense shape and Q-format recorded in the export manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockShape {
    /// Row count (`N` for vectors stored as `N×1`).
    pub rows: usize,
    /// Column count (`1` for per-neuron vectors, `M` or `N` for matrices).
    pub cols: usize,
    /// Whether the block was encoded as signed two's-complement Q8.8.
    pub signed: bool,
    /// Total bits of each stored word (16 for Q8.8).
    pub total_bits: u8,
    /// Fractional bits of each stored word (8 for Q8.8).
    pub fractional_bits: u8,
}

impl BlockShape {
    fn vector(len: usize, encoding: Q88Encoding, q: QFormat) -> Self {
        Self {
            rows: len,
            cols: 1,
            signed: encoding == Q88Encoding::Signed,
            total_bits: q.total_bits,
            fractional_bits: q.fractional_bits,
        }
    }

    fn matrix(rows: usize, cols: usize, encoding: Q88Encoding, q: QFormat) -> Self {
        Self {
            rows,
            cols,
            signed: encoding == Q88Encoding::Signed,
            total_bits: q.total_bits,
            fractional_bits: q.fractional_bits,
        }
    }
}

/// Shapes of every dense block in a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleShapes {
    /// Threshold vector (`N×1`).
    pub thresholds: BlockShape,
    /// Hidden weights (`N×M`, row-major).
    pub weights: BlockShape,
    /// Decay vector (`N×1`).
    pub decay_rates: BlockShape,
    /// Optional readout (`K×N`, row-major).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_weights: Option<BlockShape>,
}

/// Reject a name that is not a single relative basename inside `output_dir`.
pub(super) fn validate_basename(name: &str) -> Result<(), ExportError> {
    if name.is_empty() {
        return Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::Empty,
        });
    }
    if name.contains('\0') {
        return Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::InvalidCharacters,
        });
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::Absolute,
        });
    }
    if name.contains("..") {
        return Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::ParentTraversal,
        });
    }
    if name.contains('/') || name.contains('\\') {
        return Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::NotABasename,
        });
    }

    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(os)), None) if os == name => {
            if name == "." {
                return Err(ExportError::UnsafeFilename {
                    name: name.to_string(),
                    reason: FilenameError::NotABasename,
                });
            }
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(ExportError::UnsafeFilename {
                    name: name.to_string(),
                    reason: FilenameError::InvalidCharacters,
                });
            }
            Ok(())
        }
        (Some(Component::ParentDir), _) | (_, Some(Component::ParentDir)) => {
            Err(ExportError::UnsafeFilename {
                name: name.to_string(),
                reason: FilenameError::ParentTraversal,
            })
        }
        (Some(Component::RootDir), _) => Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::Absolute,
        }),
        _ => Err(ExportError::UnsafeFilename {
            name: name.to_string(),
            reason: FilenameError::NotABasename,
        }),
    }
}

pub(super) fn stamp_profile_metadata(
    params: &mut FpgaParameters,
    config: &ExportConfig,
    range_policy: RangePolicy,
) {
    let n = params.metadata.num_neurons;
    let m = params.metadata.num_channels;
    let encodings = params.metadata.encodings;
    let q = QFormat::Q8_8;
    let readout_shape = params.output_weights.as_ref().map(|words| ReadoutShape {
        rows: words.len().checked_div(n).unwrap_or(0),
        cols: n,
    });
    let overflow = match range_policy {
        RangePolicy::Reject => OverflowPolicy::Reject,
        RangePolicy::Saturate => OverflowPolicy::Saturate,
    };
    let blocks = BundleShapes {
        thresholds: BlockShape::vector(n, encodings.thresholds, q),
        weights: BlockShape::matrix(n, m, encodings.weights, q),
        decay_rates: BlockShape::vector(n, encodings.decay_rates, q),
        output_weights: readout_shape.map(|shape| {
            BlockShape::matrix(
                shape.rows,
                shape.cols,
                encodings.output_weights.unwrap_or(Q88Encoding::Signed),
                q,
            )
        }),
    };

    let mut metadata = FpgaMetadata {
        num_neurons: n,
        num_channels: m,
        memory_usage_kb: params.metadata.memory_usage_kb,
        encodings,
        profile: config.profile.id().to_string(),
        schema_version: config.profile.schema_version().to_string(),
        producer_crate: PRODUCER_CRATE.to_string(),
        producer_version: env!("CARGO_PKG_VERSION").to_string(),
        model_id: config.model_id.clone(),
        flattening: Some(super::MatrixFlattening::RowMajorDense),
        rounding: Some(RoundingMode::TruncateTowardZero),
        overflow_policy: Some(overflow),
        q_format: Some(q),
        readout_shape,
        blocks: Some(blocks),
        ..FpgaMetadata::default()
    };

    match config.profile {
        ExportProfile::LegacySpikenautV2 => {
            metadata.version = EXPORT_FORMAT_VERSION.to_string();
            metadata.target_latency_us = config
                .declared_target_latency_us
                .or(Some(SPIKENAUT_LEGACY_TARGET_LATENCY_US));
        }
        ExportProfile::GenericDenseQ88 | ExportProfile::SpikenautSignedOutputV1 => {
            metadata.target_latency_us = config.declared_target_latency_us;
        }
    }

    metadata.timestamp = match &config.timestamp {
        TimestampPolicy::Omit => String::new(),
        TimestampPolicy::Supplied(value) => value.clone(),
        TimestampPolicy::Now => chrono::Utc::now().to_rfc3339(),
    };

    params.metadata = metadata;
}

impl FpgaParameterExporter {
    /// Write a dense Q8.8 bundle using `config`.
    ///
    /// Reuses [`CheckedParameterExport::try_export`] and the #49 encoding
    /// policy. Returns an [`ExportReport`] instead of printing. The generic
    /// and signed-output profiles refuse to replace existing files unless
    /// [`ExportConfig::allow_replace`] was set. Filename and overwrite checks
    /// run before any create/truncate.
    ///
    /// ASCII `.mem` lines are uppercase 16-bit hex patterns, one word per
    /// line — not UART byte order, and not an endianness switch.
    pub fn write_with_config(
        &self,
        output_dir: impl AsRef<Path>,
        config: &ExportConfig,
    ) -> Result<ExportReport, ExportError> {
        config.files.validate()?;
        if let Some(block) = self.unsigned_hardware_block() {
            return Err(ExportError::UnsignedHardwareEncoding { block });
        }
        if config.profile.requires_readout() && self.output_weights.is_none() {
            return Err(ExportError::MissingRequiredReadout);
        }
        if self.output_weights.is_some() && config.files.output_weights.is_none() {
            return Err(ExportError::MissingReadoutFilename);
        }

        let mut params = CheckedParameterExport::try_export(self)?;
        let has_readout = params.output_weights.is_some();
        config.files.validate_for(has_readout)?;

        stamp_profile_metadata(&mut params, config, self.range_policy);

        let output_dir = output_dir.as_ref();
        let planned = planned_paths(output_dir, &config.files, has_readout);
        if matches!(config.overwrite, OverwritePolicy::Prohibit) {
            for path in &planned {
                if path.exists() {
                    return Err(ExportError::OverwriteRefused { path: path.clone() });
                }
            }
        }

        std::fs::create_dir_all(output_dir).map_err(|source| ExportError::Io {
            path: output_dir.to_path_buf(),
            source,
        })?;

        // Re-check after creating the directory so a concurrent file still
        // cannot be truncated under Prohibit. `create_dir_all` does not
        // truncate our targets.
        if matches!(config.overwrite, OverwritePolicy::Prohibit) {
            for path in &planned {
                if path.exists() {
                    return Err(ExportError::OverwriteRefused { path: path.clone() });
                }
            }
        }

        Self::write_mem_file(
            output_dir.join(&config.files.thresholds),
            &params.thresholds,
            config.overwrite,
        )?;
        Self::write_mem_file(
            output_dir.join(&config.files.weights),
            &params.weights,
            config.overwrite,
        )?;
        Self::write_mem_file(
            output_dir.join(&config.files.decay),
            &params.decay_rates,
            config.overwrite,
        )?;
        if let Some(readout) = &params.output_weights {
            let Some(name) = &config.files.output_weights else {
                return Err(ExportError::MissingReadoutFilename);
            };
            Self::write_mem_file(output_dir.join(name), readout, config.overwrite)?;
        } else if matches!(config.overwrite, OverwritePolicy::Replace)
            && let Some(name) = &config.files.output_weights
        {
            // Same leftover-delete as the legacy writer: a readout-less
            // re-export must not leave a stale `$readmemh` image.
            Self::remove_mem_file_if_present(output_dir.join(name))?;
        }

        let metadata_path = output_dir.join(&config.files.metadata);
        let metadata_json =
            serde_json::to_string_pretty(&params).map_err(|source| ExportError::Serialize {
                path: metadata_path.clone(),
                source,
            })?;
        Self::write_json_file(&metadata_path, &metadata_json, config.overwrite)?;

        Ok(ExportReport {
            profile: config.profile,
            output_dir: output_dir.to_path_buf(),
            written: config
                .files
                .names_for(has_readout)
                .into_iter()
                .map(str::to_string)
                .collect(),
            overwrite: config.overwrite,
            num_neurons: params.metadata.num_neurons,
            num_channels: params.metadata.num_channels,
            readout_shape: params.metadata.readout_shape,
        })
    }
}

fn planned_paths(output_dir: &Path, files: &ExportFileLayout, has_readout: bool) -> Vec<PathBuf> {
    files
        .names_for(has_readout)
        .into_iter()
        .map(|name| output_dir.join(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{super::MatrixFlattening, super::MemFileWriter, super::Q88_SIGNED_MAX};
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

    fn positive_2x2() -> FpgaParameterExporter {
        FpgaParameterExporter::from_params(
            vec![1.0, 0.75],
            vec![vec![0.5, 2.0], vec![0.25, 1.5]],
            vec![0.5, 0.75],
        )
    }

    fn signed_readout_4x6() -> FpgaParameterExporter {
        let mut exporter = dense_4x6();
        exporter.set_output_weights(vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, -1.0, 0.0, 0.0],
            vec![0.0, 0.0, 0.5, 0.0],
        ]);
        exporter
    }

    fn read_bytes(dir: &Path, name: &str) -> Vec<u8> {
        fs::read(dir.join(name)).unwrap_or_else(|err| panic!("read {name}: {err}"))
    }

    #[test]
    fn generic_4x6_has_no_spikenaut_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let report = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect("generic 4×6");
        assert_eq!(report.profile, ExportProfile::GenericDenseQ88);
        assert_eq!(report.num_neurons, 4);
        assert_eq!(report.num_channels, 6);
        assert_eq!(report.readout_shape, None);

        let json = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(
            !json.contains("Spikenaut"),
            "generic JSON must not inherit Spikenaut branding: {json}"
        );
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let meta = &value["metadata"];
        assert_eq!(meta["profile"], GENERIC_DENSE_PROFILE_ID);
        assert_eq!(meta["schema_version"], GENERIC_DENSE_SCHEMA_VERSION);
        assert_eq!(meta["producer_crate"], PRODUCER_CRATE);
        assert_eq!(meta["producer_version"], env!("CARGO_PKG_VERSION"));
        assert!(meta.get("version").is_none());
        assert!(meta.get("timestamp").is_none());
        assert!(meta.get("target_latency_us").is_none());
        assert_eq!(meta["flattening"], "row_major_dense");
        assert_eq!(meta["rounding"], "truncate_toward_zero");
        assert_eq!(meta["overflow_policy"], "reject");
        assert_eq!(meta["q_format"]["total_bits"], 16);
        assert_eq!(meta["q_format"]["fractional_bits"], 8);
        assert_eq!(meta["blocks"]["weights"]["rows"], 4);
        assert_eq!(meta["blocks"]["weights"]["cols"], 6);
        assert_eq!(meta["blocks"]["weights"]["signed"], true);
        assert!(value.get("output_weights").is_none());
    }

    #[test]
    fn generic_repeat_is_byte_identical() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let config = ExportConfig::generic().with_model_id("fixture-4x6");
        dense_4x6().write_with_config(dir1.path(), &config).unwrap();
        dense_4x6().write_with_config(dir2.path(), &config).unwrap();
        for name in [
            "parameters.mem",
            "parameters_weights.mem",
            "parameters_decay.mem",
            "parameters.json",
        ] {
            assert_eq!(read_bytes(dir1.path(), name), read_bytes(dir2.path(), name));
        }
        assert!(!dir1.path().join("parameters_output_weights.mem").exists());
    }

    #[test]
    fn supplied_timestamp_is_copied_verbatim_and_stable() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let config = ExportConfig::generic().with_timestamp("2026-09-15T00:00:00Z");
        dense_4x6().write_with_config(dir1.path(), &config).unwrap();
        dense_4x6().write_with_config(dir2.path(), &config).unwrap();
        assert_eq!(
            read_bytes(dir1.path(), "parameters.json"),
            read_bytes(dir2.path(), "parameters.json")
        );
        let json = fs::read_to_string(dir1.path().join("parameters.json")).unwrap();
        assert!(json.contains("2026-09-15T00:00:00Z"));
    }

    #[test]
    fn legacy_profile_reproduces_positive_fixture_filenames_and_words() {
        let dir = tempfile::tempdir().unwrap();
        let report = positive_2x2()
            .write_with_config(dir.path(), &ExportConfig::legacy_spikenaut_v2())
            .expect("legacy write");
        assert_eq!(report.profile, ExportProfile::LegacySpikenautV2);
        assert_eq!(
            report.written,
            [
                "parameters.mem",
                "parameters_weights.mem",
                "parameters_decay.mem",
                "parameters.json",
            ]
        );

        let thresholds = fs::read_to_string(dir.path().join("parameters.mem")).unwrap();
        let weights = fs::read_to_string(dir.path().join("parameters_weights.mem")).unwrap();
        let decay = fs::read_to_string(dir.path().join("parameters_decay.mem")).unwrap();
        assert_eq!(thresholds, "0100\n00C0\n");
        assert_eq!(weights, "0080\n0200\n0040\n0180\n");
        assert_eq!(decay, "0080\n00C0\n");

        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join("parameters.json")).unwrap())
                .unwrap();
        assert_eq!(json["metadata"]["version"], EXPORT_FORMAT_VERSION);
        assert_eq!(json["metadata"]["profile"], SPIKENAUT_V2_LEGACY_PROFILE_ID);
        assert_eq!(json["metadata"]["target_latency_us"], 35.0);
        assert!(!json["metadata"]["timestamp"].as_str().unwrap().is_empty());
    }

    #[test]
    fn signed_output_profile_records_kx_n_or_rejects_missing_readout() {
        let missing_dir = tempfile::tempdir().unwrap();
        let err = dense_4x6()
            .write_with_config(
                missing_dir.path(),
                &ExportConfig::spikenaut_signed_output_v1(),
            )
            .expect_err("readout required");
        assert!(matches!(err, ExportError::MissingRequiredReadout));
        assert!(fs::read_dir(missing_dir.path()).unwrap().next().is_none());

        let dir = tempfile::tempdir().unwrap();
        let report = signed_readout_4x6()
            .write_with_config(dir.path(), &ExportConfig::spikenaut_signed_output_v1())
            .expect("signed-output write");
        assert_eq!(
            report.readout_shape,
            Some(ReadoutShape { rows: 3, cols: 4 })
        );
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join("parameters.json")).unwrap())
                .unwrap();
        assert_eq!(
            json["metadata"]["profile"],
            SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID
        );
        assert_eq!(
            json["metadata"]["schema_version"],
            SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION
        );
        assert!(
            json["metadata"]
                .get("version")
                .and_then(|v| v.as_str())
                .is_none_or(|v| v != EXPORT_FORMAT_VERSION),
            "corrected contract must not silently reuse Spikenaut-v2"
        );
        assert_eq!(json["metadata"]["readout_shape"]["rows"], 3);
        assert_eq!(json["metadata"]["readout_shape"]["cols"], 4);
        assert_eq!(json["metadata"]["blocks"]["output_weights"]["rows"], 3);
        assert_eq!(json["metadata"]["blocks"]["output_weights"]["cols"], 4);
        assert_eq!(json["metadata"]["blocks"]["output_weights"]["signed"], true);
        assert!(dir.path().join("parameters_output_weights.mem").exists());
    }

    #[test]
    fn unsafe_and_colliding_filenames_are_rejected_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("parameters.mem");
        fs::write(&marker, b"KEEP").unwrap();

        let absolute = ExportFileLayout {
            thresholds: "/tmp/thresholds.mem".into(),
            weights: "parameters_weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: None,
        };
        let err = ExportConfig::generic()
            .with_files(absolute)
            .expect_err("absolute");
        assert!(matches!(
            err,
            ExportError::UnsafeFilename {
                reason: FilenameError::Absolute,
                ..
            }
        ));

        let traversal = ExportFileLayout {
            thresholds: "../escape.mem".into(),
            weights: "parameters_weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: None,
        };
        let err = ExportConfig::generic()
            .with_files(traversal)
            .expect_err("traversal");
        assert!(matches!(
            err,
            ExportError::UnsafeFilename {
                reason: FilenameError::ParentTraversal,
                ..
            }
        ));

        let nested = ExportFileLayout {
            thresholds: "nested/thresholds.mem".into(),
            weights: "parameters_weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: None,
        };
        let err = ExportConfig::generic()
            .with_files(nested)
            .expect_err("nested");
        assert!(matches!(
            err,
            ExportError::UnsafeFilename {
                reason: FilenameError::NotABasename,
                ..
            }
        ));

        let colliding = ExportFileLayout {
            thresholds: "same.mem".into(),
            weights: "same.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: None,
        };
        let err = ExportConfig::generic()
            .with_files(colliding)
            .expect_err("collision");
        assert!(matches!(err, ExportError::DuplicateFilename { .. }));

        assert_eq!(fs::read(&marker).unwrap(), b"KEEP");
    }

    #[test]
    fn generic_path_refuses_overwrite_without_consent() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("parameters.mem");
        fs::write(&marker, b"KEEP").unwrap();

        let err = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect_err("overwrite prohibited");
        assert!(matches!(err, ExportError::OverwriteRefused { .. }));
        assert_eq!(fs::read(&marker).unwrap(), b"KEEP");
        assert!(!dir.path().join("parameters.json").exists());

        dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic().allow_replace())
            .expect("explicit replace");
        assert_ne!(fs::read(&marker).unwrap(), b"KEEP");
    }

    #[test]
    fn unsupported_flattening_is_rejected() {
        let err = MatrixFlattening::parse_name("column_major").expect_err("column-major");
        assert!(matches!(
            err,
            ExportError::UnsupportedFlattening { ref requested } if requested == "column_major"
        ));
        assert_eq!(
            MatrixFlattening::parse_name("row_major_dense").unwrap(),
            MatrixFlattening::RowMajorDense
        );
        assert_eq!(
            MatrixFlattening::parse_name("row_major").unwrap(),
            MatrixFlattening::RowMajorDense
        );
    }

    #[test]
    fn generic_declared_latency_is_opt_in_not_measured() {
        let dir = tempfile::tempdir().unwrap();
        dense_4x6()
            .write_with_config(
                dir.path(),
                &ExportConfig::generic().with_declared_target_latency_us(20.0),
            )
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join("parameters.json")).unwrap())
                .unwrap();
        assert_eq!(json["metadata"]["target_latency_us"], 20.0);
        assert_ne!(
            json["metadata"]["target_latency_us"],
            SPIKENAUT_LEGACY_TARGET_LATENCY_US
        );
    }

    #[test]
    fn write_mem_files_is_the_legacy_replace_path() {
        let dir = tempfile::tempdir().unwrap();
        let report = MemFileWriter::write_mem_files(&positive_2x2(), dir.path()).expect("legacy");
        assert_eq!(report.profile, ExportProfile::LegacySpikenautV2);
        assert_eq!(report.overwrite, OverwritePolicy::Replace);
        MemFileWriter::write_mem_files(&positive_2x2(), dir.path())
            .expect("legacy overwrite still allowed");
    }

    #[test]
    fn generic_readout_without_filename_is_rejected_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ExportFileLayout {
            thresholds: "parameters.mem".into(),
            weights: "parameters_weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: None,
        };
        let err = signed_readout_4x6()
            .write_with_config(
                dir.path(),
                &ExportConfig::generic().with_files(layout).unwrap(),
            )
            .expect_err("JSON must not record readout without a .mem file");
        assert!(matches!(err, ExportError::MissingReadoutFilename));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn generic_replace_removes_stale_readout_file() {
        let dir = tempfile::tempdir().unwrap();
        signed_readout_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .unwrap();
        let leftover = dir.path().join("parameters_output_weights.mem");
        assert!(leftover.exists());

        dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic().allow_replace())
            .unwrap();
        assert!(
            !leftover.exists(),
            "stale parameters_output_weights.mem must not survive a generic re-export"
        );
        let json = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(!json.contains("output_weights"));
    }
}
