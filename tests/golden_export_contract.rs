// SPDX-License-Identifier: MIT OR Apache-2.0
//! Independent Rust↔HDL golden contracts for FPGA `.mem` export (#53).
//!
//! Expected words and `.mem` lines live under `tests/golden/` and were
//! specified from the Q8.8 definition (`raw = trunc(value × 256)`). They are
//! not produced by [`silicon_bridge::encode_q88_signed_full`] or
//! [`silicon_bridge::MemFileWriter`]. Round-tripping the encoder against
//! itself is not this suite.
//!
//! UART request/response golden bytes are deferred until #52. This file is
//! export-only and runs without Vivado, Icarus, or a board. HDL simulation
//! evidence is `tests/golden/hdl/run.sh` (see `tests/golden/hdl/SIMULATION.md`).

use serde::Deserialize;
use sha2::{Digest, Sha256};
use silicon_bridge::{
    BlockEncodings, EXPORT_FORMAT_VERSION, FpgaParameterExporter, MemFileWriter, Q88_SIGNED_MAX,
    Q88_SIGNED_MIN, Q88_UNSIGNED_MAX, Q88Encoding, encode_q88_signed_full, encode_q88_unsigned,
    format_q88_hex, q88_signed_to_f32, q88_to_f32,
};
use std::fs;
use std::path::{Path, PathBuf};

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn read_mem_lines(path: impl AsRef<Path>) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("golden mem should be readable: {err}"))
        .lines()
        .map(str::to_string)
        .collect()
}

fn parse_signed_words(lines: &[String]) -> Vec<i16> {
    lines
        .iter()
        .map(|line| u16::from_str_radix(line, 16).expect("golden mem line is hex") as i16)
        .collect()
}

/// SHA-256 of fixture bytes with CR stripped so a Windows CRLF checkout
/// matches the LF digest recorded in `checksums.sha256`.
fn sha256_hex(path: impl AsRef<Path>) -> String {
    let bytes = fs::read(path.as_ref()).expect("checksum target");
    sha256_lf(&bytes)
}

fn sha256_lf(bytes: &[u8]) -> String {
    let digest = Sha256::digest(strip_cr(bytes));
    hex_lower(&digest)
}

fn strip_cr(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|&b| b != b'\r').collect()
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Dense row-major address: `row * width + col`. Named so the layout stays
/// visible without `0 * width` identity-op lints.
fn dense_addr(row: usize, width: usize, col: usize) -> usize {
    row.checked_mul(width)
        .and_then(|base| base.checked_add(col))
        .expect("dense address fits usize")
}

fn generic_4x6_exporter() -> FpgaParameterExporter {
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

fn spikenaut_16_exporter() -> FpgaParameterExporter {
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

    let mut readout = vec![vec![0.0; 16]; 3];
    readout[0][0] = -1.0;
    readout[1][1] = 0.5;
    readout[2][15] = 1.0;

    let mut exporter = FpgaParameterExporter::from_params(thresholds, weights, vec![0.5; 16]);
    exporter.set_output_weights(readout);
    exporter
}

fn write_and_read(exporter: &FpgaParameterExporter) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    MemFileWriter::write_mem_files(exporter, dir.path()).expect("checked hardware write");
    dir
}

#[derive(Debug, Deserialize)]
struct SignedCase {
    value: f32,
    hex: String,
    raw_i16: i16,
}

#[derive(Debug, Deserialize)]
struct SignedTable {
    cases: Vec<SignedCase>,
}

#[derive(Debug, Deserialize)]
struct UnsignedCase {
    value: f32,
    hex: String,
    raw_u16: u16,
}

#[derive(Debug, Deserialize)]
struct FfffDual {
    hex: String,
    unsigned_f32: f32,
    signed_f32: f32,
}

#[derive(Debug, Deserialize)]
struct UnsignedTable {
    cases: Vec<UnsignedCase>,
    ffff_dual_interpretation: FfffDual,
}

#[derive(Debug, Deserialize)]
struct Provenance {
    schema_version: String,
    rounding: String,
    overflow_policy: String,
    flattening: String,
    uart_golden_bytes: String,
    peer_hdl: PeerHdl,
}

#[derive(Debug, Deserialize)]
struct PeerHdl {
    commit: String,
    evidence_class: String,
}

#[test]
fn fixture_provenance_records_schema_and_hdl_pin() {
    let raw = fs::read_to_string(golden_root().join("provenance.json")).expect("provenance");
    let prov: Provenance = serde_json::from_str(&raw).expect("provenance json");

    assert_eq!(prov.schema_version, EXPORT_FORMAT_VERSION);
    assert_eq!(prov.rounding, "truncate_toward_zero");
    assert_eq!(prov.overflow_policy, "reject");
    assert_eq!(prov.flattening, "row_major_dense");
    assert_eq!(prov.uart_golden_bytes, "tests/golden/uart/");
    assert_eq!(
        prov.peer_hdl.commit,
        "d45163f38ac1cd88f8a3918e3793a08ace85e132"
    );
    assert!(
        prov.peer_hdl.evidence_class.contains("simulation"),
        "HDL evidence must be labelled simulation, not board parity: {}",
        prov.peer_hdl.evidence_class
    );

    let vendor = fs::read_to_string(golden_root().join("hdl/VENDOR.txt")).expect("vendor");
    assert!(vendor.contains(&prov.peer_hdl.commit));
}

#[test]
fn recorded_checksums_match_committed_fixtures() {
    let manifest = fs::read_to_string(golden_root().join("checksums.sha256")).expect("checksums");
    let mut checked = 0usize;
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (want, rel) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("checksum line must be `hash  path`: {line}"));
        let path = golden_root().join(rel);
        assert_eq!(sha256_hex(&path), want, "checksum mismatch for {rel}");
        checked += 1;
    }
    assert!(
        checked >= 12,
        "expected a recorded checksum per committed fixture, got {checked}"
    );
}

#[test]
fn checksum_treats_crlf_as_the_recorded_lf_digest() {
    let lf = fs::read(golden_root().join("generic_4x6/expected.json")).expect("fixture");
    let crlf: Vec<u8> = lf
        .iter()
        .flat_map(|&b| {
            if b == b'\n' {
                vec![b'\r', b'\n']
            } else {
                vec![b]
            }
        })
        .collect();
    assert_ne!(Sha256::digest(&lf), Sha256::digest(&crlf));
    assert_eq!(
        sha256_lf(&lf),
        sha256_lf(&crlf),
        "Windows CRLF checkout must match the LF checksums.sha256 digest"
    );
}

#[test]
fn signed_words_match_independent_table_and_reconstruct() {
    let table: SignedTable = serde_json::from_str(
        &fs::read_to_string(golden_root().join("q88_signed.json")).expect("signed table"),
    )
    .expect("signed json");

    for case in &table.cases {
        assert_eq!(
            case.hex,
            format!("{:04X}", case.raw_i16 as u16),
            "fixture hex/raw disagree for {}",
            case.value
        );
        assert_eq!(
            q88_signed_to_f32(case.raw_i16),
            case.value,
            "independent reconstruction of {}",
            case.value
        );
        assert_eq!(
            format_q88_hex(case.value),
            case.hex,
            "encoder must match independently specified {}",
            case.value
        );
        assert_eq!(encode_q88_signed_full(case.value), case.raw_i16);
    }

    assert_eq!(table.cases[0].hex, "FF00");
    assert_eq!(
        table.cases.iter().find(|c| c.value == -128.0).unwrap().hex,
        "8000"
    );
    assert_eq!(
        table
            .cases
            .iter()
            .find(|c| (c.value - Q88_SIGNED_MAX).abs() < f32::EPSILON)
            .unwrap()
            .hex,
        "7FFF"
    );
    assert_eq!(Q88_SIGNED_MIN, -128.0);
}

#[test]
fn unsigned_full_range_and_ffff_dual_meaning() {
    let table: UnsignedTable = serde_json::from_str(
        &fs::read_to_string(golden_root().join("q88_unsigned.json")).expect("unsigned table"),
    )
    .expect("unsigned json");

    for case in &table.cases {
        assert_eq!(case.hex, format!("{:04X}", case.raw_u16));
        assert_eq!(q88_to_f32(case.raw_u16), case.value);
        assert_eq!(encode_q88_unsigned(case.value), case.raw_u16);
    }

    let dual = &table.ffff_dual_interpretation;
    assert_eq!(dual.hex, "FFFF");
    assert_eq!(dual.unsigned_f32, Q88_UNSIGNED_MAX);
    assert_eq!(dual.signed_f32, -1.0 / 256.0);
    assert_eq!(q88_to_f32(0xFFFF), dual.unsigned_f32);
    assert_eq!(q88_signed_to_f32(0xFFFF_u16 as i16), dual.signed_f32);
    assert_ne!(
        q88_to_f32(0xFFFF),
        q88_signed_to_f32(0xFFFF_u16 as i16),
        "FFFF is unsigned 255.99609375 and signed -1/256"
    );
}

#[test]
fn generic_4x6_matches_committed_mem_and_row_major_layout() {
    let fixture = golden_root().join("generic_4x6");
    let dir = write_and_read(&generic_4x6_exporter());

    for name in [
        "parameters.mem",
        "parameters_weights.mem",
        "parameters_decay.mem",
    ] {
        assert_eq!(
            read_mem_lines(dir.path().join(name)),
            read_mem_lines(fixture.join(name)),
            "{name} must match the committed fixture, not an encoder self-check"
        );
    }
    assert!(
        !dir.path().join("parameters_output_weights.mem").exists(),
        "generic layer has no readout file"
    );

    let weights = read_mem_lines(fixture.join("parameters_weights.mem"));
    assert_eq!(weights.len(), 24);
    assert_eq!(weights[1], "FF00");
    assert_eq!(weights[6], "8000");

    let params = generic_4x6_exporter()
        .try_export()
        .expect("checked generic export");
    assert_eq!(params.metadata.version, EXPORT_FORMAT_VERSION);
    assert_eq!(params.metadata.num_neurons, 4);
    assert_eq!(params.metadata.num_channels, 6);
    assert_eq!(
        params.metadata.encodings,
        BlockEncodings {
            thresholds: Q88Encoding::Signed,
            weights: Q88Encoding::Signed,
            decay_rates: Q88Encoding::Signed,
            output_weights: None,
        }
    );
    assert_eq!(params.output_weights, None);

    let reconstructed: Vec<f32> = parse_signed_words(&weights)
        .into_iter()
        .map(q88_signed_to_f32)
        .collect();
    assert_eq!(reconstructed[1], -1.0);
    assert_eq!(reconstructed[6], -128.0);
}

#[test]
fn spikenaut_16_matches_committed_bundle_including_signed_readout() {
    let fixture = golden_root().join("spikenaut_16");
    let dir = write_and_read(&spikenaut_16_exporter());

    for name in [
        "parameters.mem",
        "parameters_weights.mem",
        "parameters_decay.mem",
        "parameters_output_weights.mem",
    ] {
        assert_eq!(
            read_mem_lines(dir.path().join(name)),
            read_mem_lines(fixture.join(name)),
            "{name}"
        );
    }

    let hidden = read_mem_lines(fixture.join("parameters_weights.mem"));
    let readout = read_mem_lines(fixture.join("parameters_output_weights.mem"));
    assert_eq!(hidden.len(), 256);
    assert_eq!(readout.len(), 48);
    assert_eq!(hidden[dense_addr(0, 16, 1)], "FF00");
    assert_eq!(hidden[dense_addr(1, 16, 0)], "FF80");
    assert_eq!(hidden[dense_addr(15, 16, 0)], "8000");
    assert_eq!(hidden[dense_addr(15, 16, 15)], "7FFF");
    assert_eq!(readout[dense_addr(0, 16, 0)], "FF00");
    assert_eq!(readout[dense_addr(1, 16, 1)], "0080");
    assert_eq!(readout[dense_addr(2, 16, 15)], "0100");

    let params = spikenaut_16_exporter()
        .try_export()
        .expect("checked spikenaut-shaped export");
    assert_eq!(params.metadata.num_neurons, 16);
    assert_eq!(params.metadata.num_channels, 16);
    assert_eq!(
        params.metadata.encodings.output_weights,
        Some(Q88Encoding::Signed)
    );
    let out = params.output_weights.expect("readout present");
    assert_eq!(q88_signed_to_f32(out[0]), -1.0);
    assert_eq!(q88_signed_to_f32(out[17]), 0.5);
}

#[test]
fn readout_exporter_kx_n_is_not_hdl_neuron_major() {
    let fixture = golden_root().join("spikenaut_16");
    let kxn = read_mem_lines(fixture.join("parameters_output_weights.mem"));
    let nxk = read_mem_lines(fixture.join("hdl_readout_neuron_major.mem"));
    assert_eq!(kxn.len(), 48);
    assert_eq!(nxk.len(), 48);
    assert_ne!(
        kxn, nxk,
        "K×N exporter image must not be silently rewritten to OutputLayer N×K"
    );
    assert_eq!(kxn[dense_addr(1, 16, 1)], "0080");
    assert_eq!(nxk[dense_addr(1, 3, 1)], "0080");
    assert_ne!(kxn[dense_addr(1, 3, 1)], "0080");
}

#[test]
fn corrupted_shape_fails_the_line_count_contract() {
    let expected = read_mem_lines(golden_root().join("generic_4x6/parameters_weights.mem"));
    assert_eq!(expected.len(), 24);

    let mut short = generic_4x6_exporter();
    short.set_weights(vec![
        vec![0.5, -1.0, 0.25, 1.0, -0.5],
        vec![-128.0, Q88_SIGNED_MAX, 1.0 / 256.0, -1.0 / 256.0, 2.0],
        vec![1.0; 5],
        vec![-0.5, 0.5, -0.5, 0.5, -0.5],
    ]);
    let params = short.try_export().expect("4×5 is still rectangular");
    assert_ne!(
        params.weights.len(),
        expected.len(),
        "a 4×5 matrix must not satisfy the 4×6 line-count contract"
    );
}

#[test]
fn wrong_signedness_fails_the_numeric_contract() {
    let word = u16::from_str_radix("FF00", 16).unwrap();
    let signed = q88_signed_to_f32(word as i16);
    let unsigned = q88_to_f32(word);
    assert_eq!(signed, -1.0);
    assert_eq!(unsigned, 255.0);
    assert_ne!(
        signed, unsigned,
        "interpreting FF00 as unsigned must fail the signed contract"
    );
}

#[test]
fn wrong_flattening_order_fails_the_layout_contract() {
    let expected: ExpectedGeneric = serde_json::from_str(
        &fs::read_to_string(golden_root().join("generic_4x6/expected.json")).expect("expected"),
    )
    .expect("expected json");

    assert_eq!(
        expected.row_major_weight_hex,
        read_mem_lines(golden_root().join("generic_4x6/parameters_weights.mem"))
    );
    assert_ne!(
        expected.row_major_weight_hex, expected.column_major_weight_hex_would_be,
        "column-major order must not satisfy the dense row-major contract"
    );
    assert_eq!(expected.row_major_weight_hex[1], "FF00");
    assert_eq!(expected.column_major_weight_hex_would_be[1], "8000");
}

#[derive(Debug, Deserialize)]
struct ExpectedGeneric {
    row_major_weight_hex: Vec<String>,
    column_major_weight_hex_would_be: Vec<String>,
}

#[test]
fn missing_output_weight_file_fails_the_spikenaut_bundle_contract() {
    let dir = write_and_read(&generic_4x6_exporter());
    assert!(
        !dir.path().join("parameters_output_weights.mem").exists(),
        "a readout-less export must not invent parameters_output_weights.mem"
    );

    let required = golden_root().join("spikenaut_16/parameters_output_weights.mem");
    assert!(
        required.exists(),
        "Spikenaut-shaped fixture must commit the readout file"
    );
    assert_ne!(
        dir.path().join("parameters_output_weights.mem").exists(),
        required.exists(),
        "missing output-weight file must fail the 16-neuron readout contract"
    );
}

#[test]
fn uart_golden_bytes_live_beside_mem_fixtures() {
    let raw = fs::read_to_string(golden_root().join("provenance.json")).expect("provenance");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(v["uart_golden_bytes"], "tests/golden/uart/");
    assert!(
        v["uart_note"]
            .as_str()
            .unwrap()
            .contains("not UART byte order"),
        "ASCII hex layout must stay distinct from UART bytes"
    );
}
