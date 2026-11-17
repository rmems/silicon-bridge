// SPDX-License-Identifier: MIT OR Apache-2.0
//! `aer-route-v1` export config derivation from an in-memory `NirGraph` (#79).
//!
//! Identity routing comes from canonical input ordering — never from NIR
//! edges. Needs only the `nir` feature (no libhdf5).
#![cfg(feature = "nir")]

use silicon_bridge::nir::nodes::{Input, Lif, Linear, NirNode, Output};
use silicon_bridge::nir::{NirGraph, Tensor};
use silicon_bridge::{
    AER_ROUTE_FILENAME, AerRouteEntry, AerRouteError, AerRouteSource, AerRouteTable, ExportConfig,
    FpgaParameterExporter, MapError, SILICON_HDL_AER_PROFILE_ID,
};
use std::fs;
use std::path::PathBuf;

fn t(shape: &[usize], data: &[f32]) -> Tensor {
    Tensor::from_f32(shape.to_vec(), data.to_vec()).unwrap()
}

fn chain(nodes: Vec<(&str, NirNode)>) -> NirGraph {
    let mut g = NirGraph::new();
    let names: Vec<String> = nodes.iter().map(|(n, _)| n.to_string()).collect();
    for (name, node) in nodes {
        g.insert_node(name, node).unwrap();
    }
    for pair in names.windows(2) {
        g.add_edge(pair[0].clone(), pair[1].clone());
    }
    g
}

/// Canonical Spikenaut shape: `Input[16] → Linear[16×16] → LIF[16] →
/// Linear[3×16] → Output[3]`. The readout exists because the pinned profile
/// requires a K×N class output; routes still come from input ordering only.
fn canonical_graph() -> NirGraph {
    let mut w = vec![0.0f32; 16 * 16];
    for i in 0..16 {
        w[i * 16 + i] = 1.0;
    }
    let mut ro = vec![0.0f32; 3 * 16];
    ro[0] = 1.0;
    ro[16 + 1] = 0.5;
    ro[2 * 16 + 15] = 1.0;
    chain(vec![
        (
            "in",
            NirNode::Input(Input {
                shape: vec![16],
                metadata: Default::default(),
            }),
        ),
        (
            "fc",
            NirNode::Linear(Linear {
                weight: t(&[16, 16], &w),
                metadata: Default::default(),
            }),
        ),
        (
            "lif",
            NirNode::Lif(Lif {
                tau: t(&[16], &[0.0025; 16]),
                r: t(&[16], &[1.0; 16]),
                v_leak: t(&[16], &[0.0; 16]),
                v_threshold: t(&[16], &[0.5; 16]),
                v_reset: None,
                metadata: Default::default(),
            }),
        ),
        (
            "ro",
            NirNode::Linear(Linear {
                weight: t(&[3, 16], &ro),
                metadata: Default::default(),
            }),
        ),
        (
            "out",
            NirNode::Output(Output {
                shape: vec![3],
                metadata: Default::default(),
            }),
        ),
    ])
}

fn golden(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(rel)
}

#[test]
fn canonical_graph_produces_deterministic_identity_route_image() {
    let graph = canonical_graph();
    let config = FpgaParameterExporter::aer_export_config_from_nir(&graph, None)
        .expect("canonical graph derives an AER config");
    assert_eq!(config.profile(), SILICON_HDL_AER_PROFILE_ID);
    let routing = config.aer_routing().expect("routing present");
    assert_eq!(routing.table, AerRouteTable::identity());
    assert_eq!(routing.source, AerRouteSource::CanonicalInputOrder);
    assert!(routing.nir_derived);

    let exporter = FpgaParameterExporter::from_nir(&graph).expect("graph maps");
    let dir = tempfile::tempdir().unwrap();
    let report = exporter
        .write_with_config(dir.path(), &config)
        .expect("aer bundle writes");
    assert!(report.written.iter().any(|n| n == AER_ROUTE_FILENAME));

    let produced = fs::read(dir.path().join(AER_ROUTE_FILENAME)).unwrap();
    let golden = fs::read(golden("aer/identity_routes.mem")).unwrap();
    assert_eq!(
        produced, golden,
        "identity route image must match the committed fixture"
    );

    // Determinism: a second export is byte-identical.
    let dir2 = tempfile::tempdir().unwrap();
    exporter
        .write_with_config(dir2.path(), &config)
        .expect("repeat");
    assert_eq!(
        produced,
        fs::read(dir2.path().join(AER_ROUTE_FILENAME)).unwrap()
    );
    assert_eq!(
        fs::read(dir.path().join("parameters.json")).unwrap(),
        fs::read(dir2.path().join("parameters.json")).unwrap()
    );

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("parameters.json")).unwrap())
            .unwrap();
    let routing_json = &json["metadata"]["aer_routing"];
    assert_eq!(routing_json["nir_derived"], true);
    assert_eq!(routing_json["route_source"], "canonical_input_order");
    assert_eq!(routing_json["schema"], "aer-route-v1");
    assert!(json["metadata"]["artifact_hashes"].is_object());
}

#[test]
fn aer_bundle_parameter_files_match_the_plain_v3_profile() {
    let graph = canonical_graph();
    let exporter = FpgaParameterExporter::from_nir(&graph).expect("graph maps");

    let v3_dir = tempfile::tempdir().unwrap();
    exporter
        .write_with_config(v3_dir.path(), &ExportConfig::silicon_hdl_v3())
        .expect("v3 bundle");
    let aer_dir = tempfile::tempdir().unwrap();
    let aer_config = FpgaParameterExporter::aer_export_config_from_nir(&graph, None).unwrap();
    exporter
        .write_with_config(aer_dir.path(), &aer_config)
        .expect("aer bundle");

    // Parameter images are identical; only the route image and manifest
    // differ between the two profiles.
    for name in [
        "parameters.mem",
        "parameters_weights.mem",
        "parameters_decay.mem",
        "parameters_output_weights.mem",
        "hdl_readout_neuron_major.mem",
    ] {
        assert_eq!(
            fs::read(v3_dir.path().join(name)).unwrap(),
            fs::read(aer_dir.path().join(name)).unwrap(),
            "{name} must not differ between profiles"
        );
    }
    assert!(!v3_dir.path().join(AER_ROUTE_FILENAME).exists());
    let v3_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(v3_dir.path().join("parameters.json")).unwrap())
            .unwrap();
    assert!(v3_json["metadata"].get("aer_routing").is_none());
    assert!(v3_json["metadata"].get("artifact_hashes").is_none());
}

#[test]
fn explicit_multi_hop_plan_is_accepted_and_recorded() {
    let graph = canonical_graph();
    // Synthetic 0 -> 1 -> 2 plan (a hardware capability, not NIR topology).
    let table = AerRouteTable::identity()
        .with_entry(0, AerRouteEntry::forwarding(1).unwrap())
        .unwrap()
        .with_entry(1, AerRouteEntry::forwarding(2).unwrap())
        .unwrap()
        .with_entry(2, AerRouteEntry::terminal(2).unwrap())
        .unwrap();
    let config = FpgaParameterExporter::aer_export_config_from_nir(&graph, Some(table))
        .expect("explicit plan");
    let routing = config.aer_routing().unwrap();
    assert_eq!(routing.source, AerRouteSource::ExplicitHardwarePlan);
    assert!(routing.nir_derived);
    assert_eq!(routing.table.words()[0], 0x8001);
    assert_eq!(routing.table.words()[1], 0x8002);
    assert_eq!(routing.table.words()[2], 0xC002);

    let exporter = FpgaParameterExporter::from_nir(&graph).expect("graph maps");
    let dir = tempfile::tempdir().unwrap();
    exporter
        .write_with_config(dir.path(), &config)
        .expect("bundle");
    let image = fs::read_to_string(dir.path().join(AER_ROUTE_FILENAME)).unwrap();
    let lines: Vec<&str> = image.lines().collect();
    assert_eq!(&lines[..3], ["8001", "8002", "C002"]);
}

#[test]
fn over_budget_and_dangling_plans_are_typed_refusals() {
    let graph = canonical_graph();

    // 0 -> 1 -> 0 cycle never terminates.
    let cyclic = AerRouteTable::from_words(&{
        let mut w = [0xC000u16; 16];
        w[0] = 0x8001;
        w[1] = 0x8000;
        w
    })
    .unwrap();
    match FpgaParameterExporter::aer_export_config_from_nir(&graph, Some(cyclic)) {
        Err(MapError::Route(AerRouteError::OverBudget { start: 0 })) => {}
        other => panic!("expected Route(OverBudget), got {other:?}"),
    }

    // 0 forwards into an invalid entry.
    let dangling = AerRouteTable::from_words(&{
        let mut w = [0xC000u16; 16];
        w[0] = 0x8005;
        w[5] = 0x0000;
        w
    })
    .unwrap();
    match FpgaParameterExporter::aer_export_config_from_nir(&graph, Some(dangling)) {
        Err(MapError::Route(AerRouteError::DanglingRoute { start: 0, at: 5 })) => {}
        other => panic!("expected Route(DanglingRoute), got {other:?}"),
    }
}

#[test]
fn identity_derivation_requires_sixteen_input_lanes() {
    // Same chain shape, but only 8 inputs.
    let mut w = vec![0.0f32; 16 * 8];
    w[0] = 1.0;
    let graph = chain(vec![
        (
            "in",
            NirNode::Input(Input {
                shape: vec![8],
                metadata: Default::default(),
            }),
        ),
        (
            "fc",
            NirNode::Linear(Linear {
                weight: t(&[16, 8], &w),
                metadata: Default::default(),
            }),
        ),
        (
            "lif",
            NirNode::Lif(Lif {
                tau: t(&[16], &[0.0025; 16]),
                r: t(&[16], &[1.0; 16]),
                v_leak: t(&[16], &[0.0; 16]),
                v_threshold: t(&[16], &[0.5; 16]),
                v_reset: None,
                metadata: Default::default(),
            }),
        ),
        (
            "out",
            NirNode::Output(Output {
                shape: vec![16],
                metadata: Default::default(),
            }),
        ),
    ]);
    match FpgaParameterExporter::aer_export_config_from_nir(&graph, None) {
        Err(MapError::InvalidParameter { field, .. }) => assert_eq!(field, "identity"),
        other => panic!("expected InvalidParameter(identity), got {other:?}"),
    }
}

#[test]
fn unsupported_graphs_still_refuse_before_routing() {
    use silicon_bridge::nir::nodes::CubaLif;
    let graph = chain(vec![
        (
            "in",
            NirNode::Input(Input {
                shape: vec![16],
                metadata: Default::default(),
            }),
        ),
        (
            "lif",
            NirNode::CubaLif(CubaLif {
                tau_syn: t(&[16], &[0.0025; 16]),
                tau_mem: t(&[16], &[0.0025; 16]),
                r: t(&[16], &[1.0; 16]),
                v_leak: t(&[16], &[0.0; 16]),
                v_threshold: t(&[16], &[0.5; 16]),
                v_reset: None,
                w_in: None,
                metadata: Default::default(),
            }),
        ),
        (
            "out",
            NirNode::Output(Output {
                shape: vec![16],
                metadata: Default::default(),
            }),
        ),
    ]);
    match FpgaParameterExporter::aer_export_config_from_nir(&graph, None) {
        Err(MapError::Unsupported { .. }) => {}
        other => panic!("expected Unsupported, got {other:?}"),
    }
}
