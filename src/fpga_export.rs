// SPDX-License-Identifier: MIT OR Apache-2.0
//! FPGA parameter export for [silicon-hdl](https://github.com/rmems/silicon-hdl).
//!
//! Converts trained SNN floats to unsigned **Q8.8** (`u16`) vectors and writes
//! Vivado `$readmemh` `.mem` files consumable by `WeightRam` and `NeuronParamRam`
//! in the silicon-hdl core library.
//!
//! ## Traits
//!
//! Hardware-facing crates should depend on the traits here rather than the
//! concrete [`FpgaParameterExporter`] type when possible:
//!
//! - [`FixedPointEncode`] — `f32` → Q8.8 `u16`
//! - [`ParameterExport`] — produce [`FpgaParameters`]
//! - [`MemFileWriter`] — write `.mem` + metadata JSON
//!
//! ## Q8.8 convention used here: unsigned
//!
//! This module encodes **unsigned** Q8.8 (`u16`). Host stimuli on the UART path
//! use a **signed** `i16` convention ([`encode_q88_signed`]). The two are not
//! interchangeable — see the crate-root “Q8.8 conventions” table.
//!
//! | Aspect | This module (`.mem` export) |
//! |---|---|
//! | Raw type | `u16` (unsigned — negatives are **not** representable) |
//! | Width | 16 bits — 8 integer + 8 fractional |
//! | Scaling | `raw = value × 256`, truncated toward zero |
//! | Encoder input clamp | `[0.0, 255.99609375]` (scaled clamp `0..=65535`) |
//! | Encoder raw output | `0..=65535` |
//! | Serialized as | ASCII hex, one `{:04X}` word per line for `$readmemh` |
//! | Use it for | weights, thresholds, decay rates |

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Metadata / layout tag for the Q8.8 `.mem` bundle shared with silicon-hdl.
///
/// Historical name from the Spikenaut deployment pipeline; kept for downstream
/// tooling that keys on this string.
pub const EXPORT_FORMAT_VERSION: &str = "Spikenaut-v2";

/// Encode host floating-point values as unsigned Q8.8 (`u16`).
///
/// Q8.8 maps `value × 256` into a 16-bit word. Values outside the representable
/// range are clamped.
///
/// This is the **unsigned** convention (`0.0..=255.99609375`, raw `0..=65535`)
/// used for `.mem` parameter export. Host stimuli sent over UART use the signed
/// `i16` convention ([`encode_q88_signed`]) — see the crate-root
/// “Q8.8 conventions” table.
pub trait FixedPointEncode {
    /// Convert one `f32` to unsigned Q8.8 fixed-point.
    ///
    /// Negative inputs clamp to `0`; inputs above `255.99609375` clamp to
    /// `65535`; `NaN` encodes as `0`.
    fn encode_q88(&self, value: f32) -> u16;
}

/// Export SNN parameters as an FPGA-facing Q8.8 parameter bundle.
///
/// The resulting [`FpgaParameters`] align with silicon-hdl RAM contents
/// (`WeightRam`, `NeuronParamRam`).
pub trait ParameterExport {
    /// Build the full Q8.8 parameter set and metadata.
    fn export(&self) -> FpgaParameters;
}

/// Write Q8.8 parameter vectors as Vivado `$readmemh` `.mem` files.
pub trait MemFileWriter {
    /// Error type for filesystem / I/O failures.
    type Error;

    /// Write `parameters.mem`, `parameters_weights.mem`, `parameters_decay.mem`,
    /// and `parameters.json` under `output_dir`.
    fn write_mem_files(&self, output_dir: impl AsRef<Path>) -> Result<(), Self::Error>;
}

/// Default FPGA parameter exporter for the silicon-hdl Q8.8 layout.
///
/// Exports learned SNN parameters in Q8.8 fixed-point format for FPGA
/// deployment with a &lt;35µs/tick target latency budget.
pub struct FpgaParameterExporter {
    thresholds: Vec<f32>,
    weights: Vec<Vec<f32>>,
    decay_rates: Vec<f32>,
}

/// FPGA-compatible parameter format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FpgaParameters {
    /// Neuron thresholds in Q8.8 format
    pub thresholds: Vec<u16>,
    /// Weight matrix [neurons x channels] in Q8.8 format  
    pub weights: Vec<u16>,
    /// Decay rates in Q8.8 format
    pub decay_rates: Vec<u16>,
    /// Metadata about the parameter set
    pub metadata: FpgaMetadata,
}

/// Layout and provenance of an exported parameter bundle.
///
/// Serialized alongside the Q8.8 vectors as `parameters.json`, so a `.mem` set
/// on disk can be matched back to the shape it was generated for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FpgaMetadata {
    /// Format tag for the `.mem` bundle — always [`EXPORT_FORMAT_VERSION`].
    ///
    /// Not the crate version: downstream tooling keys on this string, so it
    /// moves only when the on-disk layout does.
    pub version: String,
    /// RFC 3339 UTC timestamp of the export, from `chrono::Utc::now`.
    pub timestamp: String,
    /// Number of neurons, taken from the threshold count.
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
    /// Per-tick latency budget for the silicon-hdl deployment, in
    /// microseconds.
    ///
    /// A fixed design target (35 µs) recorded for downstream tooling, not a
    /// measurement of this export.
    pub target_latency_us: f32,
    /// Total size of the three Q8.8 vectors in kibibytes, at 2 bytes per
    /// parameter.
    ///
    /// Counts parameters only — it excludes `$readmemh` ASCII overhead and any
    /// padding the target RAM applies.
    pub memory_usage_kb: f32,
}

impl FpgaParameterExporter {
    /// Create new exporter with default parameters
    pub fn new() -> Self {
        Self {
            thresholds: Vec::new(),
            weights: Vec::new(),
            decay_rates: Vec::new(),
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

    /// Convert `f32` to Q8.8 fixed-point format.
    ///
    /// Prefer [`FixedPointEncode::encode_q88`] when coding against the trait.
    pub fn to_q88(&self, value: f32) -> u16 {
        self.encode_q88(value)
    }

    /// Export parameters to FPGA-compatible format.
    ///
    /// Prefer [`ParameterExport::export`] when coding against the trait.
    pub fn export(&self) -> FpgaParameters {
        ParameterExport::export(self)
    }

    /// Export parameters to `.mem` files for silicon-hdl / Vivado `$readmemh`.
    ///
    /// Prefer [`MemFileWriter::write_mem_files`] when coding against the trait.
    pub fn export_to_mem_files<P: AsRef<Path>>(
        &self,
        output_dir: P,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
        }
    }

    fn calculate_memory_usage(&self) -> f32 {
        let total_params = self.thresholds.len()
            + self.weights.iter().map(|row| row.len()).sum::<usize>()
            + self.decay_rates.len();

        // Each parameter is 2 bytes (u16) in Q8.8 format
        (total_params * 2) as f32 / 1024.0
    }

    fn write_mem_file(
        path: impl AsRef<Path>,
        values: &[u16],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = fs::File::create(path)?;
        for value in values {
            writeln!(file, "{:04X}", value)?;
        }
        Ok(())
    }

    fn print_export_summary<P: AsRef<Path>>(&self, params: &FpgaParameters, output_dir: P) {
        println!("=== FPGA Parameter Export Summary ===");
        println!("Output Directory: {}", output_dir.as_ref().display());
        println!("Version: {}", params.metadata.version);
        println!("Timestamp: {}", params.metadata.timestamp);
        println!("Neurons: {}", params.metadata.num_neurons);
        println!("Channels: {}", params.metadata.num_channels);
        println!("Target Latency: {:.1}µs", params.metadata.target_latency_us);
        println!("Memory Usage: {:.2} KB", params.metadata.memory_usage_kb);
        println!();
        println!("Files Generated:");
        println!(
            "  parameters.mem         - {} thresholds",
            params.thresholds.len()
        );
        println!(
            "  parameters_weights.mem - {} weights",
            params.weights.len()
        );
        println!(
            "  parameters_decay.mem   - {} decay rates",
            params.decay_rates.len()
        );
        println!("  parameters.json        - metadata and configuration");
        println!();
        println!("SUCCESS: FPGA parameters ready for silicon-hdl deployment");
    }
}

impl FixedPointEncode for FpgaParameterExporter {
    fn encode_q88(&self, value: f32) -> u16 {
        // ENCODE SITE (unsigned Q8.8) — `.mem` / synthesis path.
        // Negatives are not representable and clamp to 0. Signed host stimuli
        // use encode_q88_signed (i16) instead.
        encode_q88_unsigned(value)
    }
}

impl ParameterExport for FpgaParameterExporter {
    fn export(&self) -> FpgaParameters {
        let thresholds_q88: Vec<u16> = self
            .thresholds
            .iter()
            .map(|&v| self.encode_q88(v))
            .collect();

        let weights_q88: Vec<u16> = self
            .weights
            .iter()
            .flat_map(|row| row.iter())
            .map(|&v| self.encode_q88(v))
            .collect();

        let decay_rates_q88: Vec<u16> = self
            .decay_rates
            .iter()
            .map(|&v| self.encode_q88(v))
            .collect();

        let metadata = FpgaMetadata {
            version: EXPORT_FORMAT_VERSION.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            num_neurons: self.thresholds.len(),
            num_channels: if self.weights.is_empty() {
                0
            } else {
                self.weights[0].len()
            },
            target_latency_us: 35.0,
            memory_usage_kb: self.calculate_memory_usage(),
        };

        FpgaParameters {
            thresholds: thresholds_q88,
            weights: weights_q88,
            decay_rates: decay_rates_q88,
            metadata,
        }
    }
}

impl MemFileWriter for FpgaParameterExporter {
    type Error = Box<dyn std::error::Error>;

    fn write_mem_files(&self, output_dir: impl AsRef<Path>) -> Result<(), Self::Error> {
        fs::create_dir_all(&output_dir)?;

        let params = ParameterExport::export(self);

        Self::write_mem_file(
            output_dir.as_ref().join("parameters.mem"),
            &params.thresholds,
        )?;
        Self::write_mem_file(
            output_dir.as_ref().join("parameters_weights.mem"),
            &params.weights,
        )?;
        Self::write_mem_file(
            output_dir.as_ref().join("parameters_decay.mem"),
            &params.decay_rates,
        )?;

        let metadata_path = output_dir.as_ref().join("parameters.json");
        let metadata_json = serde_json::to_string_pretty(&params)?;
        fs::write(metadata_path, metadata_json)?;

        self.print_export_summary(&params, output_dir);

        Ok(())
    }
}

impl Default for FpgaParameterExporter {
    fn default() -> Self {
        Self::new()
    }
}

/// Lower clamp bound for host stimuli on the signed Q8.8 UART path.
///
/// Values below this saturate to raw `-32765` (`i16`).
pub const STIMULUS_Q88_MIN: f32 = -127.99;

/// Upper clamp bound for host stimuli on the signed Q8.8 UART path.
///
/// Values above this saturate to raw `32765` (`i16`).
pub const STIMULUS_Q88_MAX: f32 = 127.99;

/// Encode an `f32` as **unsigned** Q8.8 (`u16`) without needing an exporter.
///
/// Single source of truth for the export-path encoding used by
/// [`FixedPointEncode::encode_q88`], `.mem` files, and [`format_q88_hex`]:
/// `raw = value × 256`, truncated toward zero, scaled result clamped to
/// `0..=65535`. `NaN` encodes as `0`.
///
/// Host stimuli over UART must **not** use this function — they use
/// [`encode_q88_signed`].
///
/// ```rust
/// use silicon_bridge::{encode_q88_unsigned, q88_to_f32};
///
/// assert_eq!(encode_q88_unsigned(1.0), 256);
/// assert_eq!(encode_q88_unsigned(-1.0), 0); // no negatives on the export path
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

/// Encode a host stimulus as **signed** Q8.8 (`i16`, two's complement).
///
/// `raw = value × 256`, truncated toward zero, with the *unscaled* input
/// clamped to [`STIMULUS_Q88_MIN`]`..=`[`STIMULUS_Q88_MAX`]. `NaN` encodes as
/// `0`. The UART wire format is big-endian (`to_be_bytes()`).
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
/// ```
pub fn encode_q88_signed(value: f32) -> i16 {
    // ENCODE SITE (signed Q8.8) — UART / host-stimulus path.
    // Clamp happens on the *unscaled* value so saturation lands on raw
    // ±32765 (±127.99 × 256, truncated) rather than the i16 limits.
    // `f32::clamp` propagates NaN, so map it to 0 before the i16 cast.
    if value.is_nan() {
        return 0;
    }
    (value.clamp(STIMULUS_Q88_MIN, STIMULUS_Q88_MAX) * 256.0) as i16
}

/// Decode a **signed** Q8.8 (`i16`) wire word back to `f32`.
///
/// Counterpart of [`encode_q88_signed`], but wider: every `i16` the FPGA can
/// send is valid (`-32768..=32767` → `-128.0..=127.99609375`).
pub fn q88_signed_to_f32(raw: i16) -> f32 {
    raw as f32 / 256.0
}

/// Helper function to format unsigned Q8.8 value as hex string
pub fn format_q88_hex(value: f32) -> String {
    format!("{:04X}", encode_q88_unsigned(value))
}

/// Convert **unsigned** Q8.8 back to `f32`.
///
/// Counterpart of [`encode_q88_unsigned`]. For the signed UART path use
/// [`q88_signed_to_f32`] — decoding a signed raw word with this function
/// reads negatives as large positives.
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
        assert_eq!(exporter.to_q88(255.0), 65280);

        // Test precision
        assert_eq!(q88_to_f32(256), 1.0);
        assert_eq!(q88_to_f32(0), 0.0);
        assert_eq!(q88_to_f32(65280), 255.0);
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

    fn parse_mem_words(lines: &[String]) -> Vec<u16> {
        lines
            .iter()
            .map(|line| u16::from_str_radix(line, 16).expect("mem line should be valid hex"))
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

        let decoded: Vec<f32> = thresholds.iter().copied().map(q88_to_f32).collect();
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
        assert!(!round_tripped.metadata.timestamp.is_empty());
        assert!((round_tripped.metadata.target_latency_us - 35.0).abs() < 1e-6);
        // (2 thresholds + 4 weights + 2 decay) * 2 bytes = 16 bytes
        assert!((round_tripped.metadata.memory_usage_kb - 16.0 / 1024.0).abs() < 1e-6);
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

    #[test]
    fn trait_and_inherent_encoders_match_the_free_function() {
        let exporter = FpgaParameterExporter::new();
        for value in [-5.0_f32, 0.0, 0.3, 1.0, 255.0, 300.0] {
            let expected = encode_q88_unsigned(value);
            assert_eq!(FixedPointEncode::encode_q88(&exporter, value), expected);
            assert_eq!(exporter.to_q88(value), expected);
        }
    }

    #[test]
    fn mem_words_are_four_hex_digits_uppercase() {
        assert_eq!(format_q88_hex(0.0), "0000");
        assert_eq!(format_q88_hex(1.0), "0100");
        assert_eq!(format_q88_hex(MAX_UNSIGNED_INPUT), "FFFF");
        assert_eq!(format_q88_hex(-1.0), "0000");
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
