// SPDX-License-Identifier: MIT OR Apache-2.0
//! Framework-agnostic checked `.mem` export for a 4-neuron × 6-input layer.
//!
//! Offline after Cargo dependency resolution: no Spikenaut identity, private
//! paths, sibling crates, serial device, or vendor toolchain. Weights are a
//! synthetic format fixture, not a trained model.
//!
//! ```text
//! cargo run --example generic_mem_export
//! cargo run --example generic_mem_export -- /tmp/my-export
//! ```

use silicon_bridge::{
    CheckedParameterExport, FpgaParameterExporter, ParameterShapeError, Q88_SIGNED_MAX,
    RangePolicy, encode_q88_signed, format_q88_hex, q88_signed_to_f32,
};
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn generic_4x6() -> FpgaParameterExporter {
    let mut exporter = FpgaParameterExporter::from_params(
        vec![1.0, 0.5, 1.5, 0.75],
        vec![
            vec![0.5, -1.0, 0.25, 1.0, -0.5, 0.0],
            vec![-128.0, Q88_SIGNED_MAX, 1.0 / 256.0, -1.0 / 256.0, 2.0, -2.0],
            vec![1.0; 6],
            vec![-0.5, 0.5, -0.5, 0.5, -0.5, 0.5],
        ],
        vec![0.5, 0.75, 0.25, 1.0],
    );
    // Layout tag only. Same 16-bit Q8.8 words as the default writer; this is
    // not a second on-disk format and is not Spikenaut-specific.
    exporter.set_format_version("generic-dense-q88");
    exporter
        .set_timestamp("1970-01-01T00:00:00Z")
        .expect("rfc3339 utc");
    exporter
}

fn demonstrate_errors() {
    let ragged = FpgaParameterExporter::from_params(
        vec![1.0, 1.0],
        vec![vec![0.5, 0.5], vec![0.5]],
        vec![0.9, 0.9],
    );
    match ragged.try_export() {
        Err(ParameterShapeError::RaggedWeights {
            expected: 2,
            row: 1,
            len: 1,
        }) => {}
        other => panic!("expected RaggedWeights, got {other:?}"),
    }

    let mut non_finite = FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5]], vec![0.9]);
    non_finite.set_weights(vec![vec![f32::NAN]]);
    match non_finite.try_export() {
        Err(ParameterShapeError::NonFinite { .. }) => {}
        other => panic!("expected NonFinite, got {other:?}"),
    }

    let mut overflow = FpgaParameterExporter::from_params(vec![1.0], vec![vec![200.0]], vec![0.5]);
    match overflow.try_export() {
        Err(ParameterShapeError::OutOfRange { .. }) => {}
        other => panic!("expected OutOfRange under RangePolicy::Reject, got {other:?}"),
    }

    overflow.set_range_policy(RangePolicy::Saturate);
    let (saturated, report) = overflow
        .try_export_with_report()
        .expect("Saturate path reports clamps");
    assert_eq!(report.len(), 1);
    assert_eq!(format!("{:04X}", saturated.weights[0] as u16), "7FFF");
    println!(
        "intentional saturate: 200.0 (signed) → {} (hex 7FFF); events={}",
        q88_signed_to_f32(saturated.weights[0]),
        report.len()
    );

    // Parameter path vs UART helper: -128 is 8000 in .mem, 8003 on the wire.
    assert_eq!(format_q88_hex(-128.0), "8000");
    assert_eq!(encode_q88_signed(-128.0), -32765);
    println!("Q8.8 ownership: silicon-bridge quantizes; the HDL reader owns signedness.");
    println!("  signed parameter -128.0 → 8000; UART clamp → 8003");
}

fn main() -> Result<(), Box<dyn Error>> {
    demonstrate_errors();

    let output_dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("silicon-bridge-generic-mem"));

    let exporter = generic_4x6();
    let params = CheckedParameterExport::try_export(&exporter)?;
    assert_eq!(params.metadata.version, "generic-dense-q88");
    assert!(
        !params.metadata.version.contains("Spikenaut"),
        "generic export must not carry mandatory Spikenaut identity"
    );
    assert_eq!(params.metadata.timestamp, "1970-01-01T00:00:00Z");
    assert_eq!(params.metadata.num_neurons, 4);
    assert_eq!(params.metadata.num_channels, 6);
    assert_eq!(params.output_weights, None);
    assert_eq!(params.weights[1], -256, "negative weight -1.0 must survive");
    assert_eq!(format!("{:04X}", params.weights[1] as u16), "FF00");

    fs::create_dir_all(&output_dir)?;
    exporter.write_with_config(
        &output_dir,
        &silicon_bridge::ExportConfig::generic().allow_replace(),
    )?;

    let json = fs::read_to_string(output_dir.join("parameters.json"))?;
    assert!(
        !json.contains("Spikenaut"),
        "generic parameters.json must not stamp Spikenaut identity"
    );
    let weights = fs::read_to_string(output_dir.join("parameters_weights.mem"))?;
    assert!(weights.lines().any(|line| line == "FF00"));
    assert!(!output_dir.join("parameters_output_weights.mem").exists());

    println!("generic export directory: {}", output_dir.display());
    println!("HDL reader contract (this crate does not interpret trained meaning):");
    println!("  width=16 bits, fractional_bits=8, signed two's complement, scale=256");
    println!("  flattening=row-major dense; addr = neuron * num_channels + input");
    println!(
        "  files: parameters.mem, parameters_weights.mem, parameters_decay.mem, parameters.json"
    );
    println!("  encodings recorded in metadata.encodings — do not infer from filenames");
    Ok(())
}
