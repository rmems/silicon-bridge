// SPDX-License-Identifier: MIT OR Apache-2.0
//! `.nir` → `.mem` integration tests against vendored upstream fixtures (#15).
//!
//! The expected words below are hand-computed from the fixture parameters and
//! the documented forward-Euler mapping (`dt = 1 ms`), not produced by the
//! mapper. Run with `cargo test --features nir-io` (needs libhdf5).
#![cfg(feature = "nir-io")]

use sha2::{Digest, Sha256};
use silicon_bridge::{
    LoadNirError, MapError, NirMapOptions, load_nir_to_mem, load_nir_to_mem_with,
};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/nir")
        .join(name)
}

fn mem_lines(dir: &Path, name: &str) -> Vec<String> {
    fs::read_to_string(dir.join(name))
        .unwrap_or_else(|err| panic!("{name} should exist: {err}"))
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn vendored_fixtures_match_recorded_sha256() {
    let expected = [
        (
            "lif_norse.nir",
            "0dd9143ef624892d4a6461f474d93653a337b3490a62e23ec9ec5d18fde9b7b6",
        ),
        (
            "lif_rockpool.nir",
            "f493a2cd00e20be6305acd7808faa1d4558993c91f3af43454bf637371e543d6",
        ),
        (
            "two_lif_neurons.nir",
            "806c7c1cfae72b5be92ce15a670f37014b82a5d4fba8a077efce46d193c21a82",
        ),
        (
            "braille_noDelay_bias_zero.nir",
            "f1aab3ce74024e7feac508a03b04c58d483ee3a01e293dba4b53d0b519b8650e",
        ),
        (
            "cnn_sinabs.nir",
            "e2fa55bda7aab5a772485e1b690358bcb825b303eca7dc426e3973937fcb5bcb",
        ),
    ];
    for (name, sha) in expected {
        let digest = Sha256::digest(fs::read(fixture(name)).expect("fixture readable"));
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, sha, "{name} must stay byte-identical to upstream");
    }
}

#[test]
fn norse_lif_maps_to_q88_mem() {
    // Affine W=1 b=0 -> LIF tau=0.0025 R=1 v_th=0.1, dt=1ms:
    //   alpha = 0.4, decay = 0.6, folded weight = 1 * 1 * 0.4 = 0.4.
    let dir = tempfile::tempdir().unwrap();
    let report = load_nir_to_mem(fixture("lif_norse.nir"), dir.path()).unwrap();
    assert_eq!((report.num_neurons, report.num_channels), (1, 1));
    assert!(report.readout_shape.is_none());

    // trunc(0.1*256)=25, trunc(0.4*256)=102, trunc(0.6*256)=153.
    assert_eq!(mem_lines(dir.path(), "parameters.mem"), ["0019"]);
    assert_eq!(mem_lines(dir.path(), "parameters_weights.mem"), ["0066"]);
    assert_eq!(mem_lines(dir.path(), "parameters_decay.mem"), ["0099"]);
}

#[test]
fn rockpool_resistance_is_folded_into_weights() {
    // Linear W=0.04 -> LIF tau=0.0025 R=24.019737 v_th=0.1, dt=1ms:
    //   alpha = 0.4, gain = R*alpha = 9.6078948, weight = 0.04*9.6078948 = 0.384316.
    let dir = tempfile::tempdir().unwrap();
    load_nir_to_mem(fixture("lif_rockpool.nir"), dir.path()).unwrap();
    assert_eq!(mem_lines(dir.path(), "parameters.mem"), ["0019"]);
    // trunc(0.384316*256) = trunc(98.38) = 98 = 0x62.
    assert_eq!(mem_lines(dir.path(), "parameters_weights.mem"), ["0062"]);
    assert_eq!(mem_lines(dir.path(), "parameters_decay.mem"), ["0099"]);
}

#[test]
fn dt_option_changes_decay_and_gain() {
    // dt = 0.5 ms: alpha = 0.2 -> decay 0.8 (trunc 204.8 = 204 = 0xCC),
    // weight 0.2 (trunc 51.2 = 51 = 0x33).
    let dir = tempfile::tempdir().unwrap();
    let options = NirMapOptions::default().with_dt(0.0005);
    load_nir_to_mem_with(fixture("lif_norse.nir"), dir.path(), &options).unwrap();
    assert_eq!(mem_lines(dir.path(), "parameters_weights.mem"), ["0033"]);
    assert_eq!(mem_lines(dir.path(), "parameters_decay.mem"), ["00CC"]);
}

#[test]
fn dt_larger_than_tau_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let options = NirMapOptions::default().with_dt(0.01);
    let err = load_nir_to_mem_with(fixture("lif_norse.nir"), dir.path(), &options).unwrap_err();
    assert!(
        matches!(
            err,
            LoadNirError::Map(MapError::InvalidParameter { field: "tau", .. })
        ),
        "got {err:?}"
    );
}

fn assert_unsupported(name: &str, node: &str, node_type: &str) {
    let dir = tempfile::tempdir().unwrap();
    let err = load_nir_to_mem(fixture(name), dir.path()).unwrap_err();
    match err {
        LoadNirError::Map(MapError::Unsupported {
            node: got_node,
            node_type: got_type,
            ..
        }) => {
            assert_eq!((got_node.as_str(), got_type), (node, node_type), "{name}");
        }
        other => panic!("{name}: expected Unsupported, got {other:?}"),
    }
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        0,
        "{name}: a refused graph must not write any file"
    );
}

#[test]
fn cuba_lif_recurrent_rnn_is_refused() {
    assert_unsupported("braille_noDelay_bias_zero.nir", "lif1.lif", "CubaLIF");
}

#[test]
fn convolutional_network_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_nir_to_mem(fixture("cnn_sinabs.nir"), dir.path()).unwrap_err();
    assert!(
        matches!(
            err,
            LoadNirError::Map(MapError::Unsupported {
                node_type: "Conv2d" | "SumPool2d",
                ..
            })
        ),
        "got {err:?}"
    );
}

#[test]
fn two_spiking_layers_are_refused() {
    assert_unsupported("two_lif_neurons.nir", "lif2", "LIF");
}

#[test]
fn missing_file_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_nir_to_mem(dir.path().join("absent.nir"), dir.path()).unwrap_err();
    assert!(matches!(err, LoadNirError::Read(_)), "got {err:?}");
}

#[test]
fn existing_output_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    load_nir_to_mem(fixture("lif_norse.nir"), dir.path()).unwrap();
    let err = load_nir_to_mem(fixture("lif_norse.nir"), dir.path()).unwrap_err();
    assert!(matches!(err, LoadNirError::Export(_)), "got {err:?}");
}
