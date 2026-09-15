// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public-API consumer checks for the #54 examples (no hardware, no private paths).

use silicon_bridge::{
    CheckedParameterExport, DenseQ88Layout, EXPORT_FORMAT_VERSION, FpgaParameterExporter,
    MemFileWriter, ParameterShapeError, Q88_SIGNED_MAX, RangePolicy, encode_q88_signed,
    encode_stimuli, format_q88_hex, q88_signed_to_f32,
};
use std::fs;

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
    exporter.set_format_version("generic-dense-q88");
    exporter.set_timestamp("1970-01-01T00:00:00Z");
    exporter
}

#[test]
fn generic_export_has_no_spikenaut_metadata_and_keeps_negatives() {
    let exporter = generic_4x6();
    let params = CheckedParameterExport::try_export(&exporter).expect("checked");
    assert_eq!(params.metadata.version, "generic-dense-q88");
    assert!(!params.metadata.version.contains("Spikenaut"));
    assert_eq!(params.metadata.timestamp, "1970-01-01T00:00:00Z");
    assert_eq!(params.output_weights, None);
    assert_eq!(params.weights[1], -256);
    assert_eq!(format!("{:04X}", params.weights[1] as u16), "FF00");
    assert_eq!(format_q88_hex(-1.0), "FF00");

    let dir = tempfile::tempdir().expect("tempdir");
    MemFileWriter::write_mem_files(&exporter, dir.path()).expect("write");
    let json = fs::read_to_string(dir.path().join("parameters.json")).expect("json");
    assert!(
        !json.contains("Spikenaut"),
        "generic parameters.json must not stamp Spikenaut identity: {json}"
    );
    assert!(!dir.path().join("parameters_output_weights.mem").exists());
}

#[test]
fn checked_path_shows_shape_nonfinite_and_overflow() {
    let ragged = FpgaParameterExporter::from_params(
        vec![1.0, 1.0],
        vec![vec![0.5, 0.5], vec![0.5]],
        vec![0.9, 0.9],
    );
    assert!(matches!(
        ragged.try_export(),
        Err(ParameterShapeError::RaggedWeights { .. })
    ));

    let mut non_finite = FpgaParameterExporter::from_params(vec![1.0], vec![vec![0.5]], vec![0.9]);
    non_finite.set_weights(vec![vec![f32::NAN]]);
    assert!(matches!(
        non_finite.try_export(),
        Err(ParameterShapeError::NonFinite { .. })
    ));

    let mut overflow = FpgaParameterExporter::from_params(vec![1.0], vec![vec![200.0]], vec![0.5]);
    assert!(matches!(
        overflow.try_export(),
        Err(ParameterShapeError::OutOfRange { .. })
    ));
    overflow.set_range_policy(RangePolicy::Saturate);
    let (params, report) = overflow.try_export_with_report().expect("report");
    assert_eq!(report.len(), 1);
    assert_eq!(format!("{:04X}", params.weights[0] as u16), "7FFF");
    assert_eq!(encode_q88_signed(-128.0), -32765);
    assert_eq!(format_q88_hex(-128.0), "8000");
}

#[test]
fn spikenaut_layout_tag_is_explicit_and_readout_is_signed() {
    let mut thresholds = vec![1.0; 16];
    thresholds[0] = 0.5;
    thresholds[15] = 1.5;
    let mut weights = vec![vec![0.0; 16]; 16];
    for (i, row) in weights.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    weights[0][1] = -1.0;
    let mut readout = vec![vec![0.0; 16]; 3];
    readout[0][0] = -1.0;

    let mut exporter = FpgaParameterExporter::from_params(thresholds, weights, vec![0.5; 16]);
    exporter.set_output_weights(readout);
    exporter.set_format_version(EXPORT_FORMAT_VERSION);
    let params = exporter.try_export().expect("checked");
    assert_eq!(params.metadata.version, "Spikenaut-v2");
    let out = params.output_weights.expect("readout");
    assert_eq!(q88_signed_to_f32(out[0]), -1.0);
}

#[test]
fn dense_codec_does_not_need_uart_feature() {
    let v3 = DenseQ88Layout::silicon_bridge_v3();
    let tx = encode_stimuli(&v3, &[0.1; 16]).expect("v3");
    assert_eq!(tx.len(), 33);
    let dense8 = DenseQ88Layout::dense(8, 8).expect("host codec");
    assert_eq!(encode_stimuli(&dense8, &[0.0; 8]).expect("8").len(), 17);
}
