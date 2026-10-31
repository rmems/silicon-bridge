// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic Spikenaut-shaped export: 16 neurons × 16 inputs + signed readout.
//!
//! **Opt-in** compatibility demo. New callers should start from
//! `examples/generic_mem_export.rs` (`write_generic`). This binary names the
//! historical **layout** (`Spikenaut-v2`, [`EXPORT_FORMAT_VERSION`]). That
//! string is an on-disk identifier. This binary does **not** claim the weights
//! are trained, that old model artifacts are validated, or that silicon-hdl
//! `OutputLayer` addresses readout the same way.
//!
//! Golden numeric evidence: `tests/golden/spikenaut_16/` (#53). Offline;
//! no serial device, private paths, or vendor toolchain.
//!
//! ```text
//! cargo run --example spikenaut_profile_export
//! cargo run --example spikenaut_profile_export -- /tmp/spikenaut-shaped
//! ```

use silicon_bridge::{
    EXPORT_FORMAT_VERSION, ExportConfig, FpgaParameterExporter, FpgaParameters, ParameterBlock,
    Q88_SIGNED_MAX, Q88Encoding, q88_signed_to_f32,
};
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn synthetic_spikenaut_16() -> FpgaParameterExporter {
    let mut thresholds = vec![1.0; 16];
    thresholds[0] = 0.5;
    thresholds[15] = 1.5;

    let mut weights = vec![vec![0.0; 16]; 16];
    for (i, row) in weights.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    weights[0][1] = -1.0;
    weights[1][0] = -0.5;
    weights[15][0] = -128.0;
    weights[15][15] = Q88_SIGNED_MAX;

    // K×N (3 classes × 16 hidden), signed. silicon-hdl OutputLayer indexes
    // N×K (neuron * K + class). #53 records both files; this exporter writes K×N.
    let mut readout = vec![vec![0.0; 16]; 3];
    readout[0][0] = -1.0;
    readout[1][1] = 0.5;
    readout[2][15] = 1.0;

    let mut exporter = FpgaParameterExporter::from_params(thresholds, weights, vec![0.5; 16]);
    exporter.set_output_weights(readout);
    exporter.set_encoding(ParameterBlock::Readout, Q88Encoding::Signed);
    // These shape the in-memory `try_export()` result below. The disk writer
    // stamps `parameters.json` from its `ExportConfig`, so the same pinned
    // timestamp is passed again via `with_timestamp` in `main`.
    exporter.set_format_version(EXPORT_FORMAT_VERSION);
    exporter
        .set_timestamp(PINNED_TIMESTAMP)
        .expect("rfc3339 utc");
    exporter
}

/// Fixed timestamp so repeated runs produce byte-identical `parameters.json`.
const PINNED_TIMESTAMP: &str = "1970-01-01T00:00:00Z";

fn dense_addr(row: usize, width: usize, col: usize) -> usize {
    row * width + col
}

fn main() -> Result<(), Box<dyn Error>> {
    let output_dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("silicon-bridge-spikenaut-shaped"));

    let exporter = synthetic_spikenaut_16();
    let params = exporter.try_export()?;
    assert_eq!(params.metadata.version, EXPORT_FORMAT_VERSION);
    assert_eq!(params.metadata.timestamp, PINNED_TIMESTAMP);
    assert_eq!(params.metadata.num_neurons, 16);
    assert_eq!(params.metadata.num_channels, 16);
    assert_eq!(
        params.metadata.encodings.output_weights,
        Some(Q88Encoding::Signed)
    );
    let readout = params
        .output_weights
        .as_ref()
        .expect("signed readout present");
    assert_eq!(q88_signed_to_f32(readout[dense_addr(0, 16, 0)]), -1.0);
    assert_eq!(q88_signed_to_f32(readout[dense_addr(1, 16, 1)]), 0.5);
    assert_eq!(q88_signed_to_f32(readout[dense_addr(2, 16, 15)]), 1.0);

    fs::create_dir_all(&output_dir)?;
    // `MemFileWriter::write_mem_files` is this same legacy contract with a
    // wall-clock timestamp; pin it here so the on-disk JSON matches `params`.
    let config = ExportConfig::legacy_spikenaut_v2().with_timestamp(PINNED_TIMESTAMP);
    let report = exporter.write_with_config(&output_dir, &config)?;
    let metadata_file = report
        .written
        .iter()
        .find(|name| name.ends_with(".json"))
        .expect("legacy layout writes parameters.json");
    let on_disk: FpgaParameters =
        serde_json::from_str(&fs::read_to_string(output_dir.join(metadata_file))?)?;
    assert_eq!(on_disk.metadata.timestamp, PINNED_TIMESTAMP);

    println!(
        "Spikenaut-shaped export directory: {}",
        output_dir.display()
    );
    println!(
        "profile/layout tag: {EXPORT_FORMAT_VERSION} (historical layout id, not a trained model)"
    );
    println!(
        "synthetic 16×16 hidden + 3×16 signed readout; fixtures are format tests, not trained weights"
    );
    println!(
        "readout file is K×N row-major as this crate writes it; HDL OutputLayer is N×K — see tests/golden/spikenaut_16/"
    );
    Ok(())
}
