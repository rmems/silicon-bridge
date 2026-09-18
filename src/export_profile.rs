// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dense-parameter export profiles: generic Q8.8, pinned silicon-hdl v3,
//! legacy Spikenaut-v2, and a distinct signed-output Spikenaut contract.
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
use crate::fpga_codec::{
    DENSE_Q88_SYNC, SILICON_BRIDGE_V3_CHANNELS, SILICON_BRIDGE_V3_RX_LEN, SILICON_BRIDGE_V3_TX_LEN,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, ErrorKind};
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

/// Profile identity for the pinned silicon-hdl v3 reference contract.
pub const SILICON_HDL_V3_PROFILE_ID: &str = "silicon-hdl-v3-compatible";

/// Metadata schema for the pinned silicon-hdl v3 compatibility profile.
pub const SILICON_HDL_V3_SCHEMA_VERSION: &str = "silicon-hdl-v3-profile-v1";

/// Explicit host/HDL contract id recorded by [`ExportConfig::silicon_hdl_v3`].
pub const SILICON_HDL_V3_CONTRACT_ID: &str = "silicon-bridge-silicon-hdl-v3-contract-v1";

/// Pinned silicon-hdl revision used by the reference compatibility profile.
pub const SILICON_HDL_V3_SUPPORTED_REVISION: &str = "d45163f38ac1cd88f8a3918e3793a08ace85e132";

/// Hidden-neuron count supported by the pinned silicon-hdl v3 profile.
pub const SILICON_HDL_V3_HIDDEN_NEURONS: usize = 16;

/// Input-channel count supported by the pinned silicon-hdl v3 profile.
pub const SILICON_HDL_V3_INPUT_CHANNELS: usize = 16;

/// Output-class count supported by the pinned silicon-hdl v3 profile.
pub const SILICON_HDL_V3_OUTPUT_CLASSES: usize = 3;

/// HDL-native readout image emitted by [`ExportConfig::silicon_hdl_v3`].
pub const SILICON_HDL_V3_READOUT_FILENAME: &str = "hdl_readout_neuron_major.mem";

/// Historical per-tick design target recorded by the legacy Spikenaut-v2
/// writer, in microseconds. A declared target, never a measured latency.
pub const SPIKENAUT_LEGACY_TARGET_LATENCY_US: f32 = 35.0;

/// Fixed dimensions supported by a pinned compatibility profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityDimensions {
    /// UART input channel count `M`.
    pub input_channels: usize,
    /// Hidden-neuron count `N`.
    pub hidden_neurons: usize,
    /// Output class / readout row count `K`.
    pub output_classes: usize,
}

impl CompatibilityDimensions {
    /// Dimensions of the pinned silicon-hdl v3 reference profile.
    pub const SILICON_HDL_V3: Self = Self {
        input_channels: SILICON_HDL_V3_INPUT_CHANNELS,
        hidden_neurons: SILICON_HDL_V3_HIDDEN_NEURONS,
        output_classes: SILICON_HDL_V3_OUTPUT_CLASSES,
    };
}

/// File basenames that form a compatibility-profile bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityFiles {
    /// Threshold `$readmemh` image.
    pub thresholds: String,
    /// Hidden-weight `$readmemh` image.
    pub hidden_weights: String,
    /// Decay-rate `$readmemh` image.
    pub decay: String,
    /// Generic class-major `K×N` readout image.
    pub generic_readout_kxn: String,
    /// HDL-native neuron-major `N×K` readout image.
    pub hdl_readout_nxk: String,
    /// JSON manifest.
    pub manifest: String,
}

/// SiliconBridge v3.0 UART framing pinned by the profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UartContractMetadata {
    /// Protocol label.
    pub protocol: String,
    /// Stimulus input channels.
    pub input_channels: usize,
    /// Request frame size in bytes.
    pub request_bytes: usize,
    /// Response frame size in bytes.
    pub response_bytes: usize,
    /// Sync byte at the start of every request frame.
    pub sync_byte: String,
    /// Word byte order inside request/response frames.
    pub word_byte_order: String,
}

/// Upstream contract or fixture reference pinned by compatibility metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractReference {
    /// Human-readable reference name.
    pub name: String,
    /// Canonical repository or source URL.
    pub repository: String,
    /// Pinned revision, tag, digest, or version in the referenced source.
    pub revision: String,
}

/// Machine-readable compatibility contract recorded in `parameters.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityMetadata {
    /// Stable contract identifier.
    pub contract_id: String,
    /// Versioned metadata schema for this contract record.
    pub contract_version: String,
    /// Profile identity that selected the contract.
    pub profile: String,
    /// Producing crate semantic version.
    pub silicon_bridge_version: String,
    /// Producing checkout revision when available at build time.
    pub silicon_bridge_revision: String,
    /// Generic upstream contract reference pinned by this profile.
    pub reference: Option<ContractReference>,
    /// Only dimensions supported by this pinned profile.
    pub supported_dimensions: CompatibilityDimensions,
    /// Generic hidden-weight layout.
    pub hidden_weights_layout: String,
    /// Readout layout preserved in the generic export file and JSON vector.
    pub readout_source_layout: String,
    /// Readout layout emitted for silicon-hdl `OutputLayer`.
    pub readout_hdl_layout: String,
    /// Exported file names and the expected `$readmemh` targets.
    pub files: CompatibilityFiles,
    /// Reset assumption for replaying the contract.
    pub reset_assumption: String,
    /// Logical timestep assumption for the host/HDL exchange.
    pub timestep_assumption: String,
    /// UART frame contract that must remain unchanged for this profile.
    pub uart: UartContractMetadata,
}

/// Named dense-parameter export contract.
///
/// [`ExportProfile::GenericDenseQ88`] is the framework-agnostic path.
/// [`ExportProfile::LegacySpikenautV2`] preserves the historical tag,
/// filenames, overwrite behaviour, and declared 35 µs target.
/// [`ExportProfile::SpikenautSignedOutputV1`] is the corrected signed-readout
/// contract and **must not** be serialized as `Spikenaut-v2`.
/// [`ExportProfile::SiliconHdlV3`] pins the reference silicon-hdl contract and
/// emits an additional HDL-native readout image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ExportProfile {
    /// Neutral dense Q8.8 bundle: no Spikenaut branding, no invented timing,
    /// timestamps omitted unless the caller supplies one.
    GenericDenseQ88,
    /// Historical Spikenaut-v2 compatibility profile.
    LegacySpikenautV2,
    /// Corrected Spikenaut contract that requires a signed `K×N` readout.
    /// Construct with [`ExportConfig::generic_with_required_readout`] (or
    /// the Spikenaut-named [`ExportConfig::spikenaut_signed_output_v1`]).
    /// **Must not** be serialized as `Spikenaut-v2`.
    SpikenautSignedOutputV1,
    /// Pinned silicon-hdl v3 reference profile. Writes the generic `K×N`
    /// readout and an additional HDL-native `N×K` readout image.
    SiliconHdlV3,
}

impl ExportProfile {
    /// Stable profile identity string, distinct from schema and crate version.
    pub fn id(self) -> &'static str {
        match self {
            Self::GenericDenseQ88 => GENERIC_DENSE_PROFILE_ID,
            Self::LegacySpikenautV2 => SPIKENAUT_V2_LEGACY_PROFILE_ID,
            Self::SpikenautSignedOutputV1 => SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID,
            Self::SiliconHdlV3 => SILICON_HDL_V3_PROFILE_ID,
        }
    }

    /// Schema of the metadata document this profile writes.
    pub fn schema_version(self) -> &'static str {
        match self {
            Self::GenericDenseQ88 => GENERIC_DENSE_SCHEMA_VERSION,
            // Historical tag is the schema for the compatibility profile.
            Self::LegacySpikenautV2 => EXPORT_FORMAT_VERSION,
            Self::SpikenautSignedOutputV1 => SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION,
            Self::SiliconHdlV3 => SILICON_HDL_V3_SCHEMA_VERSION,
        }
    }

    /// Whether this profile requires a readout / output-weight matrix.
    pub fn requires_readout(self) -> bool {
        matches!(self, Self::SpikenautSignedOutputV1 | Self::SiliconHdlV3)
    }
}

/// Readout file contract for a dense export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadoutContract {
    /// Readout/output weights are optional.
    Optional,
    /// A generic class-major `K×N` readout file is required.
    RequiredKxN,
    /// A generic `K×N` readout is required and an HDL-native `N×K` copy is emitted.
    RequiredKxNAndHdlNxK {
        /// HDL-native readout filename.
        filename: String,
    },
}

impl ReadoutContract {
    /// Whether the contract rejects exports without output/readout weights.
    pub fn requires_readout(&self) -> bool {
        matches!(self, Self::RequiredKxN | Self::RequiredKxNAndHdlNxK { .. })
    }

    /// HDL-native readout filename emitted in addition to the generic `K×N` readout, if any.
    pub fn hdl_filename(&self) -> Option<&str> {
        match self {
            Self::RequiredKxNAndHdlNxK { filename } => Some(filename.as_str()),
            Self::Optional | Self::RequiredKxN => None,
        }
    }
}

/// A dense Q8.8 export contract selected by [`ExportConfig`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExportContract {
    profile_id: String,
    schema_version: String,
    files: ExportFileLayout,
    readout: ReadoutContract,
    compatibility: Option<CompatibilityMetadata>,
    immutable_files: bool,
    legacy_version: Option<String>,
    default_target_latency_us: Option<f32>,
    supported_dimensions: Option<CompatibilityDimensions>,
}

impl ExportContract {
    /// Build a user-defined HDL/export contract.
    pub fn custom(
        profile_id: impl Into<String>,
        schema_version: impl Into<String>,
        files: ExportFileLayout,
        readout: ReadoutContract,
    ) -> Result<Self, ExportError> {
        let contract = Self {
            profile_id: profile_id.into(),
            schema_version: schema_version.into(),
            files,
            readout,
            compatibility: None,
            immutable_files: false,
            legacy_version: None,
            default_target_latency_us: None,
            supported_dimensions: None,
        };
        contract.validate_file_names(true)?;
        Ok(contract)
    }

    /// Framework-agnostic dense Q8.8 contract.
    pub fn generic_dense_q88() -> Self {
        Self::known(
            GENERIC_DENSE_PROFILE_ID,
            GENERIC_DENSE_SCHEMA_VERSION,
            ExportFileLayout::generic_default(),
            ReadoutContract::Optional,
        )
    }

    /// Generic dense Q8.8 contract that requires a signed `K×N` readout.
    pub fn generic_required_readout() -> Self {
        Self::known(
            SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID,
            SPIKENAUT_SIGNED_OUTPUT_SCHEMA_VERSION,
            ExportFileLayout::spikenaut_deployment(),
            ReadoutContract::RequiredKxN,
        )
    }

    /// Historical Spikenaut-v2 compatibility contract.
    pub fn legacy_spikenaut_v2() -> Self {
        let mut contract = Self::known(
            SPIKENAUT_V2_LEGACY_PROFILE_ID,
            EXPORT_FORMAT_VERSION,
            ExportFileLayout::spikenaut_deployment(),
            ReadoutContract::Optional,
        );
        contract.legacy_version = Some(EXPORT_FORMAT_VERSION.to_string());
        contract.default_target_latency_us = Some(SPIKENAUT_LEGACY_TARGET_LATENCY_US);
        contract
    }

    /// Pinned silicon-hdl v3 reference contract.
    pub fn silicon_hdl_v3() -> Self {
        let files = ExportFileLayout::spikenaut_deployment();
        Self {
            profile_id: SILICON_HDL_V3_PROFILE_ID.to_string(),
            schema_version: SILICON_HDL_V3_SCHEMA_VERSION.to_string(),
            compatibility: Some(silicon_hdl_v3_metadata_for_files(&files)),
            files,
            readout: ReadoutContract::RequiredKxNAndHdlNxK {
                filename: SILICON_HDL_V3_READOUT_FILENAME.to_string(),
            },
            immutable_files: true,
            legacy_version: None,
            default_target_latency_us: None,
            supported_dimensions: Some(CompatibilityDimensions::SILICON_HDL_V3),
        }
    }

    fn known(
        profile_id: impl Into<String>,
        schema_version: impl Into<String>,
        files: ExportFileLayout,
        readout: ReadoutContract,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            schema_version: schema_version.into(),
            files,
            readout,
            compatibility: None,
            immutable_files: false,
            legacy_version: None,
            default_target_latency_us: None,
            supported_dimensions: None,
        }
    }

    /// Profile identity written to metadata.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Metadata schema version written by this contract.
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    /// File layout for this contract.
    pub fn files(&self) -> &ExportFileLayout {
        &self.files
    }

    /// Readout behavior for this contract.
    pub fn readout(&self) -> &ReadoutContract {
        &self.readout
    }

    fn validate_file_names(&self, include_optional_readout_name: bool) -> Result<(), ExportError> {
        self.files.validate_for(include_optional_readout_name)?;
        if let Some(name) = self.readout.hdl_filename() {
            validate_basename(name)?;
            let mut seen = self.files.names_for(include_optional_readout_name);
            if seen
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
            {
                return Err(ExportError::DuplicateFilename {
                    name: name.to_string(),
                });
            }
            seen.push(name);
        }
        Ok(())
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
    ///
    /// Order is thresholds, weights, decay, optional readout, then metadata.
    fn names_for(&self, has_readout: bool) -> Vec<&str> {
        let mut names = vec![
            self.thresholds.as_str(),
            self.weights.as_str(),
            self.decay.as_str(),
        ];
        if has_readout && let Some(name) = self.output_weights.as_deref() {
            names.push(name);
        }
        names.push(self.metadata.as_str());
        names
    }

    /// Reject unsafe or colliding names before any filesystem write.
    pub fn validate(&self) -> Result<(), ExportError> {
        self.validate_for(true)
    }

    fn validate_for(&self, include_optional_readout_name: bool) -> Result<(), ExportError> {
        let mut seen = BTreeSet::new();
        let mut seen_folded = BTreeSet::new();
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
            // Windows/macOS default volumes are case-insensitive; ASCII
            // case-folded duplicates would truncate the first block.
            if !seen_folded.insert(name.to_ascii_lowercase()) {
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
    contract: ExportContract,
    overwrite: OverwritePolicy,
    timestamp: TimestampPolicy,
    declared_target_latency_us: Option<f32>,
    model_id: String,
}

impl ExportConfig {
    /// Neutral dense Q8.8 export: no Spikenaut tag, no timestamp, no declared
    /// latency, overwrite prohibited until [`Self::allow_replace`].
    ///
    /// This is also [`Default`]: new public helpers start here. Call
    /// [`Self::generic_with_required_readout`] when a signed `K×N` readout
    /// is required. Spikenaut-v2 deployments use
    /// [`Self::legacy_spikenaut_v2`].
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
        Self::from_contract(ExportContract::generic_dense_q88())
    }

    /// Historical Spikenaut-v2 writer: documented filenames, wall-clock
    /// timestamp, declared 35 µs target, and replacement of existing files.
    ///
    /// This is the compatibility default used by
    /// [`super::MemFileWriter::write_mem_files`].
    pub fn legacy_spikenaut_v2() -> Self {
        Self {
            contract: ExportContract::legacy_spikenaut_v2(),
            overwrite: OverwritePolicy::Replace,
            timestamp: TimestampPolicy::Now,
            declared_target_latency_us: Some(SPIKENAUT_LEGACY_TARGET_LATENCY_US),
            model_id: String::new(),
        }
    }

    /// Neutral required-readout dense Q8.8 export.
    ///
    /// Same contract as [`Self::spikenaut_signed_output_v1`]: a signed `K×N`
    /// readout is required, overwrite is prohibited until
    /// [`Self::allow_replace`], and timestamps / declared latency are omitted
    /// unless supplied. On-disk metadata still records
    /// [`SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID`] — this constructor does not
    /// invent a second schema.
    #[doc(alias = "spikenaut_signed_output_v1")]
    pub fn generic_with_required_readout() -> Self {
        Self::from_contract(ExportContract::generic_required_readout())
    }

    /// Spikenaut-named alias of [`Self::generic_with_required_readout`].
    ///
    /// Prefer the generic name for new callers. This name remains so existing
    /// Spikenaut deployments keep compiling. Distinct schema/profile ids —
    /// does not emit `Spikenaut-v2`.
    #[doc(alias = "generic_with_required_readout")]
    pub fn spikenaut_signed_output_v1() -> Self {
        Self::generic_with_required_readout()
    }

    /// Pinned silicon-hdl v3 compatibility profile.
    ///
    /// This profile keeps the generic `parameters_output_weights.mem` readout
    /// as class-major `K×N` and also emits
    /// [`SILICON_HDL_V3_READOUT_FILENAME`] as neuron-major `N×K` for the
    /// reference silicon-hdl `OutputLayer`. It supports only the pinned
    /// 16-input, 16-hidden, 3-output contract recorded by
    /// [`SILICON_HDL_V3_CONTRACT_ID`].
    pub fn silicon_hdl_v3() -> Self {
        Self::from_contract(ExportContract::silicon_hdl_v3())
    }

    /// Build a config from a user-defined or predefined export contract.
    pub fn from_contract(contract: ExportContract) -> Self {
        Self {
            contract,
            overwrite: OverwritePolicy::Prohibit,
            timestamp: TimestampPolicy::Omit,
            declared_target_latency_us: None,
            model_id: String::new(),
        }
    }

    /// Contract this configuration will write.
    pub fn contract(&self) -> &ExportContract {
        &self.contract
    }

    /// Profile identity this configuration will write.
    pub fn profile(&self) -> &str {
        self.contract.profile_id()
    }

    /// File layout this configuration will write.
    pub fn files(&self) -> &ExportFileLayout {
        self.contract.files()
    }

    /// Optional HDL-native readout file emitted in addition to the generic
    /// `K×N` readout file.
    pub fn hdl_readout_file(&self) -> Option<&str> {
        self.contract.readout.hdl_filename()
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
    ///
    /// Pinned compatibility profiles may reject this because their filenames
    /// are part of the external HDL contract.
    pub fn with_files(mut self, files: ExportFileLayout) -> Result<Self, ExportError> {
        if self.contract.immutable_files {
            return Err(ExportError::ImmutableFileLayout {
                profile: self.contract.profile_id.clone(),
            });
        }
        files.validate_for(true)?;
        let mut contract = self.contract.clone();
        contract.files = files;
        contract.validate_file_names(true)?;
        self.contract = contract;
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

    fn names_for(&self, has_readout: bool) -> Vec<&str> {
        let mut names = vec![
            self.contract.files.thresholds.as_str(),
            self.contract.files.weights.as_str(),
            self.contract.files.decay.as_str(),
        ];
        if has_readout && let Some(name) = self.contract.files.output_weights.as_deref() {
            names.push(name);
        }
        if has_readout && let Some(name) = self.contract.readout.hdl_filename() {
            names.push(name);
        }
        names.push(self.contract.files.metadata.as_str());
        names
    }

    fn validate_file_names(&self, has_readout: bool) -> Result<(), ExportError> {
        self.contract.validate_file_names(has_readout)
    }
}

impl Default for ExportConfig {
    /// [`Self::generic`]: framework-agnostic dense Q8.8.
    fn default() -> Self {
        Self::generic()
    }
}

/// Files written by a profiled export. Returned instead of printing.
///
/// This report does not claim board compatibility, measured latency, or HDL
/// correctness. Matching documented filenames is not proof that silicon-hdl
/// RAM will consume the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Profile identity that produced the files.
    pub profile: String,
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
    if path.is_absolute()
        || name.starts_with('/')
        || name.starts_with('\\')
        || looks_like_windows_drive(name)
    {
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

/// `C:` / `C:\…` prefixes are absolute on Windows and must not be treated as
/// portable relative basenames on Unix either.
fn looks_like_windows_drive(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
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
        profile: config.contract.profile_id.clone(),
        schema_version: config.contract.schema_version.clone(),
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

    metadata.version = config.contract.legacy_version.clone().unwrap_or_default();
    metadata.target_latency_us = config
        .declared_target_latency_us
        .or(config.contract.default_target_latency_us);

    metadata.timestamp = match &config.timestamp {
        TimestampPolicy::Omit => String::new(),
        TimestampPolicy::Supplied(value) => value.clone(),
        TimestampPolicy::Now => chrono::Utc::now().to_rfc3339(),
    };

    metadata.compatibility = config.contract.compatibility.clone();

    params.metadata = metadata;
}

fn silicon_hdl_v3_metadata_for_files(files: &ExportFileLayout) -> CompatibilityMetadata {
    CompatibilityMetadata {
        contract_id: SILICON_HDL_V3_CONTRACT_ID.to_string(),
        contract_version: SILICON_HDL_V3_SCHEMA_VERSION.to_string(),
        profile: SILICON_HDL_V3_PROFILE_ID.to_string(),
        silicon_bridge_version: env!("CARGO_PKG_VERSION").to_string(),
        silicon_bridge_revision: env!("SILICON_BRIDGE_GIT_REV").to_string(),
        reference: Some(ContractReference {
            name: "silicon-hdl".to_string(),
            repository: "https://github.com/rmems/silicon-hdl".to_string(),
            revision: SILICON_HDL_V3_SUPPORTED_REVISION.to_string(),
        }),
        supported_dimensions: CompatibilityDimensions::SILICON_HDL_V3,
        hidden_weights_layout: "row_major_nxm".to_string(),
        readout_source_layout: "class_major_kxn".to_string(),
        readout_hdl_layout: "neuron_major_nxk".to_string(),
        files: CompatibilityFiles {
            thresholds: files.thresholds.clone(),
            hidden_weights: files.weights.clone(),
            decay: files.decay.clone(),
            generic_readout_kxn: files
                .output_weights
                .clone()
                .unwrap_or_else(|| "parameters_output_weights.mem".to_string()),
            hdl_readout_nxk: SILICON_HDL_V3_READOUT_FILENAME.to_string(),
            manifest: files.metadata.clone(),
        },
        reset_assumption: "reset asserted before loading/replay; no retained state across fixtures"
            .to_string(),
        timestep_assumption: "one synchronous logical timestep per SiliconBridge v3.0 exchange"
            .to_string(),
        uart: UartContractMetadata {
            protocol: "SiliconBridge v3.0".to_string(),
            input_channels: SILICON_BRIDGE_V3_CHANNELS,
            request_bytes: SILICON_BRIDGE_V3_TX_LEN,
            response_bytes: SILICON_BRIDGE_V3_RX_LEN,
            sync_byte: format!("0x{DENSE_Q88_SYNC:02X}"),
            word_byte_order: "big_endian_16_bit_words".to_string(),
        },
    }
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
    /// The complete bundle is written into a temporary subdirectory of
    /// `output_dir`, then each planned file is promoted into place. A failure
    /// during validation or staging leaves destination files untouched and
    /// removes the staging directory. [`OverwritePolicy::Prohibit`] promotes
    /// with an exclusive link/create so a dest that appears after the
    /// pre-check — including a dangling symlink, which `Path::exists`
    /// misses — is refused rather than replaced. It also deletes any files
    /// already promoted if a later promote fails.
    /// [`OverwritePolicy::Replace`] cannot restore previous contents after
    /// the first successful rename: same-filesystem rename is per-file, not
    /// a multi-file swap (Windows may `unlink` the destination first).
    ///
    /// ASCII `.mem` lines are uppercase 16-bit hex patterns, one word per
    /// line — not UART byte order, and not an endianness switch.
    pub fn write_with_config(
        &self,
        output_dir: impl AsRef<Path>,
        config: &ExportConfig,
    ) -> Result<ExportReport, ExportError> {
        config.validate_file_names(true)?;
        if config
            .declared_target_latency_us
            .is_some_and(|us| !us.is_finite())
        {
            return Err(ExportError::NonFiniteDeclaredLatency);
        }
        if let Some(block) = self.unsigned_hardware_block() {
            return Err(ExportError::UnsignedHardwareEncoding { block });
        }
        if config.contract.readout.requires_readout() && self.output_weights.is_none() {
            return Err(ExportError::MissingRequiredReadout {
                profile: config.contract.profile_id.clone(),
            });
        }
        if self.output_weights.is_some() && config.files().output_weights.is_none() {
            return Err(ExportError::MissingReadoutFilename);
        }

        let mut params = CheckedParameterExport::try_export(self)?;
        let has_readout = params.output_weights.is_some();
        config.validate_file_names(has_readout)?;
        if let Some(expected) = config.contract.supported_dimensions {
            validate_compatibility_shape(&params, config.contract.profile_id(), expected)?;
        }

        stamp_profile_metadata(&mut params, config, self.range_policy);

        let output_dir = output_dir.as_ref();
        let planned = planned_paths(output_dir, config, has_readout);
        if matches!(config.overwrite, OverwritePolicy::Prohibit) {
            for path in &planned {
                if dest_occupied(path) {
                    return Err(ExportError::OverwriteRefused { path: path.clone() });
                }
            }
        }

        let metadata_json =
            serde_json::to_string_pretty(&params).map_err(|source| ExportError::Serialize {
                path: output_dir.join(&config.files().metadata),
                source,
            })?;

        fs::create_dir_all(output_dir).map_err(|source| ExportError::Io {
            path: output_dir.to_path_buf(),
            source,
        })?;

        // Re-check after creating the directory so a concurrent file still
        // cannot be truncated under Prohibit. `create_dir_all` does not
        // truncate our targets.
        if matches!(config.overwrite, OverwritePolicy::Prohibit) {
            for path in &planned {
                if dest_occupied(path) {
                    return Err(ExportError::OverwriteRefused { path: path.clone() });
                }
            }
        }

        let staging = create_staging_dir(output_dir)?;
        let staged_overwrite = OverwritePolicy::Replace;
        Self::write_mem_file(
            staging.path.join(&config.files().thresholds),
            &params.thresholds,
            staged_overwrite,
        )?;
        Self::write_mem_file(
            staging.path.join(&config.files().weights),
            &params.weights,
            staged_overwrite,
        )?;
        Self::write_mem_file(
            staging.path.join(&config.files().decay),
            &params.decay_rates,
            staged_overwrite,
        )?;
        if let Some(readout) = &params.output_weights {
            let Some(name) = &config.files().output_weights else {
                return Err(ExportError::MissingReadoutFilename);
            };
            Self::write_mem_file(staging.path.join(name), readout, staged_overwrite)?;
            if let Some(hdl_name) = config.hdl_readout_file() {
                let Some(shape) = params.metadata.readout_shape else {
                    return Err(ExportError::MissingRequiredReadout {
                        profile: config.contract.profile_id.clone(),
                    });
                };
                let hdl_readout = transpose_readout_kxn_to_nxk(readout, shape);
                Self::write_mem_file(staging.path.join(hdl_name), &hdl_readout, staged_overwrite)?;
            }
        }
        Self::write_json_file(
            staging.path.join(&config.files().metadata),
            &metadata_json,
            staged_overwrite,
        )?;

        let names = config.names_for(has_readout);
        let mut promoted = Vec::new();
        for name in &names {
            let from = staging.path.join(name);
            let to = output_dir.join(name);
            if let Err(err) = promote_file(&from, &to, config.overwrite) {
                if matches!(config.overwrite, OverwritePolicy::Prohibit) {
                    for path in &promoted {
                        let _ = fs::remove_file(path);
                    }
                }
                return Err(err);
            }
            promoted.push(to);
        }

        if !has_readout
            && matches!(config.overwrite, OverwritePolicy::Replace)
            && let Some(name) = &config.files().output_weights
        {
            // Same leftover-delete as the legacy writer: a readout-less
            // re-export must not leave a stale `$readmemh` image.
            Self::remove_mem_file_if_present(output_dir.join(name))?;
        }

        Ok(ExportReport {
            profile: config.contract.profile_id.clone(),
            output_dir: output_dir.to_path_buf(),
            written: names.into_iter().map(str::to_string).collect(),
            overwrite: config.overwrite,
            num_neurons: params.metadata.num_neurons,
            num_channels: params.metadata.num_channels,
            readout_shape: params.metadata.readout_shape,
        })
    }

    /// Write a [`ExportConfig::generic`] dense Q8.8 bundle.
    ///
    /// This is the default public disk path. Equivalent to
    /// `write_with_config(output_dir, &ExportConfig::generic())`. Staging and
    /// overwrite semantics are those of [`Self::write_with_config`]. Prefer
    /// this over [`super::ParameterExport::export`] and
    /// [`super::MemFileWriter::write_mem_files`]. Required-readout bundles use
    /// [`ExportConfig::generic_with_required_readout`]. Spikenaut-v2
    /// deployments call [`super::MemFileWriter::write_mem_files`] or
    /// [`ExportConfig::legacy_spikenaut_v2`].
    ///
    /// ```
    /// use silicon_bridge::FpgaParameterExporter;
    ///
    /// let exporter = FpgaParameterExporter::from_params(
    ///     vec![1.0, 0.5, 1.5, 0.75],
    ///     vec![vec![0.5; 6]; 4],
    ///     vec![0.9; 4],
    /// );
    /// let dir = tempfile::tempdir().unwrap();
    /// let report = exporter.write_generic(dir.path()).unwrap();
    /// assert_eq!(report.profile, silicon_bridge::GENERIC_DENSE_PROFILE_ID);
    /// let json = std::fs::read_to_string(dir.path().join("parameters.json")).unwrap();
    /// assert!(!json.contains("Spikenaut"));
    /// ```
    pub fn write_generic(&self, output_dir: impl AsRef<Path>) -> Result<ExportReport, ExportError> {
        self.write_with_config(output_dir, &ExportConfig::generic())
    }
}

fn planned_paths(output_dir: &Path, config: &ExportConfig, has_readout: bool) -> Vec<PathBuf> {
    config
        .names_for(has_readout)
        .into_iter()
        .map(|name| output_dir.join(name))
        .collect()
}

fn validate_compatibility_shape(
    params: &FpgaParameters,
    profile: &str,
    expected: CompatibilityDimensions,
) -> Result<(), ExportError> {
    let readout =
        params
            .output_weights
            .as_ref()
            .ok_or_else(|| ExportError::MissingRequiredReadout {
                profile: profile.to_string(),
            })?;
    let output_classes = readout
        .len()
        .checked_div(params.metadata.num_neurons)
        .unwrap_or(0);
    let actual = CompatibilityDimensions {
        input_channels: params.metadata.num_channels,
        hidden_neurons: params.metadata.num_neurons,
        output_classes,
    };
    if actual == expected {
        Ok(())
    } else {
        Err(ExportError::UnsupportedCompatibilityShape {
            profile: profile.to_string(),
            expected,
            actual,
        })
    }
}

fn transpose_readout_kxn_to_nxk(readout: &[i16], shape: ReadoutShape) -> Vec<i16> {
    let mut out = Vec::with_capacity(readout.len());
    for neuron in 0..shape.cols {
        for class in 0..shape.rows {
            out.push(readout[class * shape.cols + neuron]);
        }
    }
    out
}

const STAGING_PREFIX: &str = ".silicon-bridge-staging-";

struct StagingDir {
    path: PathBuf,
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_staging_dir(output_dir: &Path) -> Result<StagingDir, ExportError> {
    for n in 0..1024u32 {
        let path = output_dir.join(format!("{STAGING_PREFIX}{}-{n}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(StagingDir { path }),
            Err(source) if source.kind() == ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(ExportError::Io { path, source }),
        }
    }
    Err(ExportError::Io {
        path: output_dir.to_path_buf(),
        source: std::io::Error::new(
            ErrorKind::AlreadyExists,
            "could not allocate a unique staging directory",
        ),
    })
}

fn dest_occupied(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn promote_file(from: &Path, to: &Path, overwrite: OverwritePolicy) -> Result<(), ExportError> {
    match overwrite {
        OverwritePolicy::Prohibit => promote_exclusive(from, to),
        OverwritePolicy::Replace => promote_replace(from, to),
    }
}

/// Promote without replacing an existing dest, including a dangling symlink.
///
/// Unix `rename` would clobber a dest created after `exists()`, and
/// `Path::exists` is false for a dangling symlink. A hard link (same
/// directory, same filesystem) fails with `AlreadyExists` instead. If the
/// filesystem rejects hard links, fall back to `create_new` + copy.
fn promote_exclusive(from: &Path, to: &Path) -> Result<(), ExportError> {
    match fs::hard_link(from, to) {
        Ok(()) => {
            let _ = fs::remove_file(from);
            Ok(())
        }
        Err(source) if source.kind() == ErrorKind::AlreadyExists => {
            Err(ExportError::OverwriteRefused {
                path: to.to_path_buf(),
            })
        }
        Err(_) => promote_exclusive_copy(from, to),
    }
}

fn promote_exclusive_copy(from: &Path, to: &Path) -> Result<(), ExportError> {
    let mut dest = match fs::OpenOptions::new().write(true).create_new(true).open(to) {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::AlreadyExists => {
            return Err(ExportError::OverwriteRefused {
                path: to.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(ExportError::Io {
                path: to.to_path_buf(),
                source,
            });
        }
    };
    let copied = (|| -> Result<(), ExportError> {
        let mut src = fs::File::open(from).map_err(|source| ExportError::Io {
            path: from.to_path_buf(),
            source,
        })?;
        io::copy(&mut src, &mut dest).map_err(|source| ExportError::Io {
            path: to.to_path_buf(),
            source,
        })?;
        dest.sync_all().map_err(|source| ExportError::Io {
            path: to.to_path_buf(),
            source,
        })?;
        Ok(())
    })();
    if copied.is_err() {
        let _ = fs::remove_file(to);
    } else {
        let _ = fs::remove_file(from);
    }
    copied
}

fn promote_replace(from: &Path, to: &Path) -> Result<(), ExportError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(source) => {
            if dest_occupied(to) {
                fs::remove_file(to).map_err(|source| ExportError::Io {
                    path: to.to_path_buf(),
                    source,
                })?;
                fs::rename(from, to).map_err(|source| ExportError::Io {
                    path: to.to_path_buf(),
                    source,
                })
            } else {
                Err(ExportError::Io {
                    path: to.to_path_buf(),
                    source,
                })
            }
        }
    }
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
    fn export_config_default_is_generic() {
        assert_eq!(ExportConfig::default(), ExportConfig::generic());
        assert_eq!(ExportConfig::default().profile(), GENERIC_DENSE_PROFILE_ID);
    }

    #[test]
    fn required_readout_alias_matches_spikenaut_named_constructor() {
        assert_eq!(
            ExportConfig::generic_with_required_readout(),
            ExportConfig::spikenaut_signed_output_v1()
        );
        assert_eq!(
            ExportConfig::generic_with_required_readout().profile(),
            SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID
        );
        assert!(
            ExportConfig::generic_with_required_readout()
                .contract()
                .readout()
                .requires_readout()
        );
    }

    #[test]
    fn generic_4x6_has_no_spikenaut_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let report = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect("generic 4×6");
        assert_eq!(report.profile, GENERIC_DENSE_PROFILE_ID);
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
        assert_eq!(report.profile, SPIKENAUT_V2_LEGACY_PROFILE_ID);
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
                &ExportConfig::generic_with_required_readout(),
            )
            .expect_err("readout required");
        assert!(matches!(
            err,
            ExportError::MissingRequiredReadout { profile } if profile == SPIKENAUT_SIGNED_OUTPUT_PROFILE_ID
        ));
        assert!(fs::read_dir(missing_dir.path()).unwrap().next().is_none());

        let dir = tempfile::tempdir().unwrap();
        let report = signed_readout_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic_with_required_readout())
            .expect("signed-output write");
        assert_eq!(
            report.readout_shape,
            Some(ReadoutShape { rows: 3, cols: 4 })
        );
        assert_eq!(
            report.written,
            [
                "parameters.mem",
                "parameters_weights.mem",
                "parameters_decay.mem",
                "parameters_output_weights.mem",
                "parameters.json",
            ]
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

        let case_fold = ExportFileLayout {
            thresholds: "Weights.mem".into(),
            weights: "weights.mem".into(),
            decay: "parameters_decay.mem".into(),
            metadata: "parameters.json".into(),
            output_weights: None,
        };
        let err = ExportConfig::generic()
            .with_files(case_fold)
            .expect_err("case-insensitive collision");
        assert!(matches!(err, ExportError::DuplicateFilename { .. }));

        assert_eq!(fs::read(&marker).unwrap(), b"KEEP");
    }

    #[test]
    fn silicon_hdl_profile_rejects_file_layout_overrides() {
        let custom = ExportFileLayout {
            thresholds: "custom_thresholds.mem".into(),
            weights: "custom_weights.mem".into(),
            decay: "custom_decay.mem".into(),
            metadata: "custom_metadata.json".into(),
            output_weights: Some("custom_readout.mem".into()),
        };

        let err = ExportConfig::silicon_hdl_v3()
            .with_files(custom)
            .expect_err("silicon-hdl v3 filenames are contract fields");

        assert!(matches!(
            err,
            ExportError::ImmutableFileLayout { profile } if profile == SILICON_HDL_V3_PROFILE_ID
        ));

        let config = ExportConfig::silicon_hdl_v3();
        assert_eq!(config.files(), &ExportFileLayout::spikenaut_deployment());
        assert_eq!(
            config.hdl_readout_file(),
            Some(SILICON_HDL_V3_READOUT_FILENAME)
        );
    }

    #[test]
    fn custom_contract_requires_readout_without_named_profiles() {
        let contract = ExportContract::custom(
            "acme-hdl-v1",
            "acme-hdl-schema-v1",
            ExportFileLayout::generic_default(),
            ReadoutContract::RequiredKxN,
        )
        .expect("custom contract");

        let missing_dir = tempfile::tempdir().unwrap();
        let err = dense_4x6()
            .write_with_config(
                missing_dir.path(),
                &ExportConfig::from_contract(contract.clone()),
            )
            .expect_err("readout required");
        assert!(matches!(
            err,
            ExportError::MissingRequiredReadout { profile } if profile == "acme-hdl-v1"
        ));

        let dir = tempfile::tempdir().unwrap();
        let report = signed_readout_4x6()
            .write_with_config(dir.path(), &ExportConfig::from_contract(contract))
            .expect("custom write");
        assert_eq!(report.profile, "acme-hdl-v1");

        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join("parameters.json")).unwrap())
                .unwrap();
        assert_eq!(json["metadata"]["profile"], "acme-hdl-v1");
        assert_eq!(json["metadata"]["schema_version"], "acme-hdl-schema-v1");
        assert!(
            !json.to_string().contains("Spikenaut"),
            "custom contract must not inherit Spikenaut branding"
        );
        assert!(json["metadata"].get("compatibility").is_none());
    }

    #[test]
    fn custom_contract_can_emit_hdl_native_readout() {
        let contract = ExportContract::custom(
            "acme-hdl-v1",
            "acme-hdl-schema-v1",
            ExportFileLayout::generic_default(),
            ReadoutContract::RequiredKxNAndHdlNxK {
                filename: "acme_readout_nxk.mem".into(),
            },
        )
        .expect("custom contract");

        let dir = tempfile::tempdir().unwrap();
        let report = signed_readout_4x6()
            .write_with_config(dir.path(), &ExportConfig::from_contract(contract))
            .expect("custom write");

        assert_eq!(
            report.written,
            [
                "parameters.mem",
                "parameters_weights.mem",
                "parameters_decay.mem",
                "parameters_output_weights.mem",
                "acme_readout_nxk.mem",
                "parameters.json",
            ]
        );

        let source = fs::read_to_string(dir.path().join("parameters_output_weights.mem")).unwrap();
        let hdl = fs::read_to_string(dir.path().join("acme_readout_nxk.mem")).unwrap();
        assert_ne!(source, hdl);
        assert_eq!(hdl.lines().count(), 12);
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
        assert!(leftover_staging(dir.path()).is_empty());

        dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic().allow_replace())
            .expect("explicit replace");
        assert_ne!(fs::read(&marker).unwrap(), b"KEEP");
        assert!(leftover_staging(dir.path()).is_empty());
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
    fn non_finite_declared_latency_is_rejected_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let err = dense_4x6()
            .write_with_config(
                dir.path(),
                &ExportConfig::generic().with_declared_target_latency_us(f32::NAN),
            )
            .expect_err("NaN latency");
        assert!(matches!(err, ExportError::NonFiniteDeclaredLatency));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn write_mem_files_is_the_legacy_replace_path() {
        let dir = tempfile::tempdir().unwrap();
        let report = MemFileWriter::write_mem_files(&positive_2x2(), dir.path()).expect("legacy");
        assert_eq!(report.profile, SPIKENAUT_V2_LEGACY_PROFILE_ID);
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
        assert!(
            leftover_staging(dir.path()).is_empty(),
            "successful replace must not leave a staging directory"
        );
    }

    fn leftover_staging(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(STAGING_PREFIX))
            })
            .collect()
    }

    #[test]
    fn staged_export_leaves_no_staging_dir() {
        let dir = tempfile::tempdir().unwrap();
        dense_4x6()
            .write_generic(dir.path())
            .expect("generic write");
        assert!(dir.path().join("parameters.mem").is_file());
        assert!(dir.path().join("parameters.json").is_file());
        assert!(
            leftover_staging(dir.path()).is_empty(),
            "successful export must promote out of the staging directory"
        );
    }

    #[test]
    fn staging_dir_is_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let staging = create_staging_dir(dir.path()).expect("allocate staging");
        let path = staging.path.clone();
        assert!(path.is_dir());
        drop(staging);
        assert!(!path.exists(), "StagingDir Drop must remove the directory");
    }

    #[test]
    fn prohibit_existing_metadata_does_not_write_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let metadata = dir.path().join("parameters.json");
        fs::write(&metadata, b"KEEP").unwrap();

        let err = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect_err("overwrite prohibited");
        assert!(matches!(err, ExportError::OverwriteRefused { .. }));
        assert_eq!(fs::read(&metadata).unwrap(), b"KEEP");
        assert!(!dir.path().join("parameters.mem").exists());
        assert!(!dir.path().join("parameters_weights.mem").exists());
        assert!(!dir.path().join("parameters_decay.mem").exists());
        assert!(leftover_staging(dir.path()).is_empty());
    }

    #[test]
    fn replace_promote_failure_cleans_staging() {
        let dir = tempfile::tempdir().unwrap();
        let json_dir = dir.path().join("parameters.json");
        fs::create_dir(&json_dir).unwrap();
        fs::write(json_dir.join("nested"), b"KEEP").unwrap();

        let err = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic().allow_replace())
            .expect_err("cannot replace a directory named parameters.json");
        assert!(matches!(err, ExportError::Io { .. }));
        assert!(
            leftover_staging(dir.path()).is_empty(),
            "failed promote must still remove the staging directory"
        );
        assert!(
            json_dir.is_dir(),
            "Replace does not restore a directory target"
        );
        assert_eq!(fs::read(json_dir.join("nested")).unwrap(), b"KEEP");
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_promote_refuses_dangling_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("staged");
        let to = dir.path().join("dest");
        fs::write(&from, b"NEW").unwrap();
        symlink("missing-target", &to).unwrap();
        assert!(
            !to.exists(),
            "Path::exists must be false for a dangling symlink"
        );
        let err = promote_exclusive(&from, &to).expect_err("no clobber");
        assert!(matches!(err, ExportError::OverwriteRefused { .. }));
        assert!(
            to.symlink_metadata().unwrap().file_type().is_symlink(),
            "dangling symlink must remain"
        );
        assert_eq!(fs::read(&from).unwrap(), b"NEW");
    }

    #[cfg(unix)]
    #[test]
    fn prohibit_refuses_dangling_symlink_without_writing_siblings() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("parameters.json");
        symlink("missing-target", &json).unwrap();

        let err = dense_4x6()
            .write_with_config(dir.path(), &ExportConfig::generic())
            .expect_err("overwrite prohibited");
        assert!(matches!(err, ExportError::OverwriteRefused { .. }));
        assert!(json.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!dir.path().join("parameters.mem").exists());
        assert!(!dir.path().join("parameters_weights.mem").exists());
        assert!(leftover_staging(dir.path()).is_empty());
    }
}
