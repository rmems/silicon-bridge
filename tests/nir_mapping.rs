// SPDX-License-Identifier: MIT OR Apache-2.0
//! In-memory `NirGraph` → `FpgaParameterExporter` mapping tests (#15).
//!
//! Needs only the `nir` feature (no libhdf5). Expected Q8.8 words are
//! hand-computed from the documented forward-Euler mapping (`dt = 1 ms`
//! unless stated), not produced by the mapper.
#![cfg(feature = "nir")]

use silicon_bridge::nir::nodes::{
    Affine, AvgPool2d, Conv1d, Conv2d, CubaLi, CubaLif, Delay, Flatten, I, If, Input, Li, Lif,
    Linear, NirNode, Output, Padding, Scale, SumPool2d, Threshold,
};
use silicon_bridge::nir::{NirGraph, Tensor};
use silicon_bridge::{FpgaParameterExporter, MapError, NirMapOptions};

fn t(shape: &[usize], data: &[f32]) -> Tensor {
    Tensor::from_f32(shape.to_vec(), data.to_vec()).unwrap()
}

fn input(n: usize) -> NirNode {
    NirNode::Input(Input {
        shape: vec![n],
        metadata: Default::default(),
    })
}

fn output(n: usize) -> NirNode {
    NirNode::Output(Output {
        shape: vec![n],
        metadata: Default::default(),
    })
}

fn affine(rows: usize, cols: usize, w: &[f32], bias: f32) -> NirNode {
    NirNode::Affine(Affine {
        weight: t(&[rows, cols], w),
        bias: t(&[rows], &vec![bias; rows]),
        metadata: Default::default(),
    })
}

fn linear(rows: usize, cols: usize, w: &[f32]) -> NirNode {
    NirNode::Linear(Linear {
        weight: t(&[rows, cols], w),
        metadata: Default::default(),
    })
}

/// LIF with per-neuron `tau`, `r`, threshold; zero leak; no reset.
fn lif(tau: &[f32], r: &[f32], v_th: &[f32]) -> Lif {
    let n = tau.len();
    Lif {
        tau: t(&[n], tau),
        r: t(&[n], r),
        v_leak: t(&[n], &vec![0.0; n]),
        v_threshold: t(&[n], v_th),
        v_reset: None,
        metadata: Default::default(),
    }
}

/// Build a linear chain `names[0] → names[1] → …`.
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

/// `Input(M) → Affine(N×M) → neuron → Output(N)`.
fn layer(m: usize, n: usize, w: &[f32], neuron: NirNode) -> NirGraph {
    chain(vec![
        ("in", input(m)),
        ("fc", affine(n, m, w, 0.0)),
        ("neuron", neuron),
        ("out", output(n)),
    ])
}

fn export(g: &NirGraph) -> silicon_bridge::FpgaParameters {
    FpgaParameterExporter::from_nir(g)
        .expect("graph maps")
        .try_export()
        .expect("mapped values are rectangular, finite, in range")
}

/// `FpgaParameterExporter` is not `Debug`, so `unwrap_err` is unavailable.
fn refused(result: Result<FpgaParameterExporter, MapError>) -> MapError {
    match result {
        Err(err) => err,
        Ok(_) => panic!("expected the graph to be refused"),
    }
}

fn unsupported(err: MapError) -> (String, &'static str) {
    match err {
        MapError::Unsupported {
            node, node_type, ..
        } => (node, node_type),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn lif_maps_per_neuron_threshold_decay_and_gain() {
    // 2 neurons, 3 inputs. dt = 1 ms.
    //   n0: tau 2.5 ms -> alpha 0.4, decay 0.6, R=1   -> gain 0.4
    //   n1: tau 5 ms   -> alpha 0.2, decay 0.8, R=2.5 -> gain 0.5
    let node = NirNode::Lif(lif(&[0.0025, 0.005], &[1.0, 2.5], &[0.5, 1.0]));
    let g = layer(3, 2, &[1.0, -1.0, 0.5, 2.0, 0.0, -2.0], node);
    let p = export(&g);

    // trunc(0.5*256)=128, trunc(1.0*256)=256.
    assert_eq!(p.thresholds, vec![0x0080, 0x0100]);
    // trunc(0.6*256)=153=0x99, trunc(0.8*256)=204=0xCC.
    assert_eq!(p.decay_rates, vec![0x0099, 0x00CC]);
    // row0 = [1,-1,.5]*0.4 = [.4,-.4,.2] -> [102,-102,51]
    // row1 = [2,0,-2]*0.5  = [1,0,-1]    -> [256,0,-256]
    assert_eq!(p.weights, vec![102, -102, 51, 256, 0, -256]);
    assert_eq!(p.output_weights, None);
}

#[test]
fn scalar_neuron_parameters_broadcast() {
    let node = NirNode::Lif(Lif {
        tau: Tensor::scalar_f32(0.0025),
        r: Tensor::scalar_f32(1.0),
        v_leak: Tensor::scalar_f32(0.0),
        v_threshold: Tensor::scalar_f32(0.5),
        v_reset: None,
        metadata: Default::default(),
    });
    let p = export(&layer(2, 2, &[1.0, 0.0, 0.0, 1.0], node));
    assert_eq!(p.thresholds, vec![128, 128]);
    assert_eq!(p.decay_rates, vec![0x0099, 0x0099]);
}

#[test]
fn if_neuron_is_a_perfect_integrator() {
    // decay 1.0 = 0x0100; gain = R*dt = 250 * 1e-3 = 0.25 -> 64.
    let node = NirNode::If(If {
        r: t(&[1], &[250.0]),
        v_threshold: t(&[1], &[1.0]),
        v_reset: Some(t(&[1], &[0.0])),
        metadata: Default::default(),
    });
    let p = export(&layer(1, 1, &[1.0], node));
    assert_eq!(p.decay_rates, vec![0x0100]);
    assert_eq!(p.weights, vec![64]);
    assert_eq!(p.thresholds, vec![0x0100]);
}

#[test]
fn threshold_is_stateless() {
    let node = NirNode::Threshold(Threshold {
        threshold: t(&[1], &[0.75]),
        metadata: Default::default(),
    });
    let p = export(&layer(2, 1, &[0.5, -0.5], node));
    assert_eq!(p.decay_rates, vec![0], "no carried state");
    assert_eq!(p.weights, vec![128, -128], "no folded gain");
    assert_eq!(p.thresholds, vec![192]);
}

#[test]
fn linear_weights_and_readout_map_without_scaling() {
    // Input(2) -> Linear(3x2) -> LIF(3) -> Linear(2x3 readout) -> Output(2).
    let g = chain(vec![
        ("in", input(2)),
        ("fc1", linear(3, 2, &[1.0; 6])),
        ("lif", NirNode::Lif(lif(&[0.0025; 3], &[1.0; 3], &[1.0; 3]))),
        ("fc2", affine(2, 3, &[0.5, -0.5, 1.0, -1.0, 0.25, 0.0], 0.0)),
        ("out", output(2)),
    ]);
    let mapper = FpgaParameterExporter::from_nir(&g).unwrap();
    let p = mapper.try_export().unwrap();
    assert_eq!(p.weights, vec![102; 6]);
    // Readout is K x N, row-major, and NOT scaled by the neuron gain.
    assert_eq!(p.output_weights, Some(vec![128, -128, 256, -256, 64, 0]));
    assert_eq!((p.metadata.num_neurons, p.metadata.num_channels), (3, 2));
}

#[test]
fn flatten_is_transparent() {
    let node = NirNode::Threshold(Threshold {
        threshold: t(&[1], &[1.0]),
        metadata: Default::default(),
    });
    let g = chain(vec![
        ("in", input(2)),
        (
            "flat",
            NirNode::Flatten(Flatten {
                start_dim: 1,
                end_dim: -1,
                input_type: None,
                metadata: Default::default(),
            }),
        ),
        ("fc", affine(1, 2, &[1.0, 1.0], 0.0)),
        ("neuron", node),
        ("out", output(1)),
    ]);
    assert_eq!(export(&g).weights, vec![256, 256]);
}

#[test]
fn multi_axis_input_requires_flatten() {
    let build = |flatten: bool| {
        let mut nodes = vec![(
            "in",
            NirNode::Input(Input {
                shape: vec![2, 3],
                metadata: Default::default(),
            }),
        )];
        if flatten {
            nodes.push((
                "flat",
                NirNode::Flatten(Flatten {
                    start_dim: 0,
                    end_dim: -1,
                    input_type: None,
                    metadata: Default::default(),
                }),
            ));
        }
        nodes.push(("fc", affine(1, 6, &[1.0; 6], 0.0)));
        nodes.push((
            "neuron",
            NirNode::Threshold(Threshold {
                threshold: t(&[1], &[1.0]),
                metadata: Default::default(),
            }),
        ));
        nodes.push(("out", output(1)));
        chain(nodes)
    };
    match refused(FpgaParameterExporter::from_nir(&build(false))) {
        MapError::InvalidParameter { node, field, .. } => {
            assert_eq!((node.as_str(), field), ("in", "shape"));
        }
        other => panic!("expected InvalidParameter, got {other:?}"),
    }
    assert_eq!(export(&build(true)).weights, vec![256; 6]);
}

#[test]
fn zero_bias_and_zero_reset_are_accepted() {
    let mut neuron = lif(&[0.0025], &[1.0], &[1.0]);
    neuron.v_reset = Some(t(&[1], &[0.0]));
    let g = layer(1, 1, &[1.0], NirNode::Lif(neuron));
    assert!(FpgaParameterExporter::from_nir(&g).is_ok());
}

/// One node of every wire type except the three that are structural/mapped
/// (`Input`, `Output`, `Linear`/`Affine`, neurons, `Flatten`), which the tests
/// above cover. Each must be refused by name and wire type.
#[test]
fn every_unsupported_wire_type_is_refused_by_name() {
    let one = || t(&[1], &[1.0]);
    let i64s = || Tensor::from_i64(vec![2], vec![2, 2]).unwrap();
    let cases: Vec<(&str, NirNode)> = vec![
        (
            "CubaLIF",
            NirNode::CubaLif(CubaLif {
                tau_syn: one(),
                tau_mem: one(),
                r: one(),
                v_leak: t(&[1], &[0.0]),
                v_threshold: one(),
                v_reset: None,
                w_in: None,
                metadata: Default::default(),
            }),
        ),
        (
            "CubaLI",
            NirNode::CubaLi(CubaLi {
                tau_syn: one(),
                tau_mem: one(),
                r: one(),
                v_leak: t(&[1], &[0.0]),
                w_in: None,
                metadata: Default::default(),
            }),
        ),
        (
            "LI",
            NirNode::Li(Li {
                tau: one(),
                r: one(),
                v_leak: t(&[1], &[0.0]),
                metadata: Default::default(),
            }),
        ),
        (
            "I",
            NirNode::I(I {
                r: one(),
                metadata: Default::default(),
            }),
        ),
        (
            "Scale",
            NirNode::Scale(Scale {
                scale: one(),
                metadata: Default::default(),
            }),
        ),
        (
            "Delay",
            NirNode::Delay(Delay {
                delay: one(),
                metadata: Default::default(),
            }),
        ),
        (
            "Conv1d",
            NirNode::Conv1d(Conv1d {
                weight: t(&[1, 1, 1], &[1.0]),
                stride: vec![1],
                padding: Padding::single(0),
                dilation: vec![1],
                groups: 1,
                bias: one(),
                input_shape: None,
                metadata: Default::default(),
            }),
        ),
        (
            "Conv2d",
            NirNode::Conv2d(Conv2d {
                weight: t(&[1, 1, 1, 1], &[1.0]),
                stride: vec![1, 1],
                padding: Padding::Valid,
                dilation: vec![1, 1],
                groups: 1,
                bias: one(),
                input_shape: None,
                metadata: Default::default(),
            }),
        ),
        (
            "SumPool2d",
            NirNode::SumPool2d(SumPool2d {
                kernel_size: i64s(),
                stride: i64s(),
                padding: Tensor::from_i64(vec![2], vec![0, 0]).unwrap(),
                metadata: Default::default(),
            }),
        ),
        (
            "AvgPool2d",
            NirNode::AvgPool2d(AvgPool2d {
                kernel_size: i64s(),
                stride: i64s(),
                padding: Tensor::from_i64(vec![2], vec![0, 0]).unwrap(),
                metadata: Default::default(),
            }),
        ),
        ("NIRGraph", NirNode::Graph(Box::new(NirGraph::new()))),
    ];
    assert_eq!(cases.len(), 11);
    for (wire, node) in cases {
        let g = chain(vec![("in", input(1)), ("x", node), ("out", output(1))]);
        let err = refused(FpgaParameterExporter::from_nir(&g));
        assert_eq!(unsupported(err), ("x".to_string(), wire));
    }
}

#[test]
fn cuba_lif_policy_names_the_dual_tau_reason() {
    let g = chain(vec![
        ("in", input(1)),
        (
            "cuba",
            NirNode::CubaLif(CubaLif {
                tau_syn: t(&[1], &[1.0]),
                tau_mem: t(&[1], &[1.0]),
                r: t(&[1], &[1.0]),
                v_leak: t(&[1], &[0.0]),
                v_threshold: t(&[1], &[1.0]),
                v_reset: None,
                w_in: None,
                metadata: Default::default(),
            }),
        ),
        ("out", output(1)),
    ]);
    let err = refused(FpgaParameterExporter::from_nir(&g));
    assert!(err.to_string().contains("dual-tau"), "{err}");
}

#[test]
fn nonzero_bias_leak_and_reset_are_refused() {
    let biased = chain(vec![
        ("in", input(1)),
        ("fc", affine(1, 1, &[1.0], 0.5)),
        ("neuron", NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0]))),
        ("out", output(1)),
    ]);
    let err = refused(FpgaParameterExporter::from_nir(&biased));
    assert_eq!(unsupported(err), ("fc".into(), "Affine"));

    let mut leaky = lif(&[0.0025], &[1.0], &[1.0]);
    leaky.v_leak = t(&[1], &[1.2]);
    let err = refused(FpgaParameterExporter::from_nir(&layer(
        1,
        1,
        &[1.0],
        NirNode::Lif(leaky),
    )));
    assert_eq!(unsupported(err), ("neuron".into(), "LIF"));

    let mut reset = lif(&[0.0025], &[1.0], &[1.0]);
    reset.v_reset = Some(t(&[1], &[0.3]));
    let err = refused(FpgaParameterExporter::from_nir(&layer(
        1,
        1,
        &[1.0],
        NirNode::Lif(reset),
    )));
    assert_eq!(unsupported(err), ("neuron".into(), "LIF"));
}

#[test]
fn second_spiking_layer_is_refused() {
    let n = || NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0]));
    let g = chain(vec![
        ("in", input(1)),
        ("fc1", affine(1, 1, &[1.0], 0.0)),
        ("lif1", n()),
        ("fc2", affine(1, 1, &[1.0], 0.0)),
        ("lif2", n()),
        ("out", output(1)),
    ]);
    let err = refused(FpgaParameterExporter::from_nir(&g));
    assert_eq!(unsupported(err), ("lif2".into(), "LIF"));
}

#[test]
fn neuron_without_weights_is_refused() {
    let g = chain(vec![
        ("in", input(1)),
        ("neuron", NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0]))),
        ("out", output(1)),
    ]);
    let err = refused(FpgaParameterExporter::from_nir(&g));
    assert_eq!(unsupported(err), ("neuron".into(), "LIF"));
}

#[test]
fn recurrence_and_fan_out_are_refused() {
    let mut g = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    g.add_edge("neuron", "fc");
    let err = refused(FpgaParameterExporter::from_nir(&g));
    assert_eq!(unsupported(err).0, "neuron", "fan-out at the neuron");

    let mut cycle = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    // Replace the exit edge with a back-edge so the walk revisits `fc`.
    cycle.edges.retain(|(from, _)| from != "neuron");
    cycle.add_edge("neuron", "fc");
    let err = refused(FpgaParameterExporter::from_nir(&cycle));
    assert_eq!(unsupported(err).0, "fc", "recurrent edge into fc");
}

#[test]
fn malformed_topology_is_reported() {
    let no_input = chain(vec![("fc", affine(1, 1, &[1.0], 0.0)), ("out", output(1))]);
    assert!(matches!(
        FpgaParameterExporter::from_nir(&no_input),
        Err(MapError::InvalidTopology { .. })
    ));

    let mut two_inputs = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    two_inputs.insert_node("in2", input(1)).unwrap();
    two_inputs.add_edge("in2", "fc");
    assert!(matches!(
        FpgaParameterExporter::from_nir(&two_inputs),
        Err(MapError::InvalidTopology { .. })
    ));

    let mut dangling = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    dangling.edges.retain(|(from, _)| from != "neuron");
    assert!(matches!(
        FpgaParameterExporter::from_nir(&dangling),
        Err(MapError::InvalidTopology { .. })
    ));

    let mut stray = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    stray.insert_node("loose", output(1)).unwrap();
    assert!(matches!(
        FpgaParameterExporter::from_nir(&stray),
        Err(MapError::InvalidTopology { .. })
    ));

    let mut broken = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    broken.add_edge("fc", "ghost");
    assert!(matches!(
        FpgaParameterExporter::from_nir(&broken),
        Err(MapError::Graph(_))
    ));
}

#[test]
fn shape_and_parameter_errors_name_the_field() {
    let field_of = |g: &NirGraph| match refused(FpgaParameterExporter::from_nir(g)) {
        MapError::InvalidParameter { node, field, .. } => (node, field),
        other => panic!("expected InvalidParameter, got {other:?}"),
    };
    let ok = || NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0]));

    // Input declares 2 elements but the weights expect 1.
    let g = chain(vec![
        ("in", input(2)),
        ("fc", affine(1, 1, &[1.0], 0.0)),
        ("n", ok()),
        ("out", output(1)),
    ]);
    assert_eq!(field_of(&g), ("in".into(), "shape"));

    // Output declares 3 elements but the layer has 1 neuron.
    let g = chain(vec![
        ("in", input(1)),
        ("fc", affine(1, 1, &[1.0], 0.0)),
        ("n", ok()),
        ("out", output(3)),
    ]);
    assert_eq!(field_of(&g), ("out".into(), "shape"));

    // Readout expects 2 hidden units but the layer has 1.
    let g = chain(vec![
        ("in", input(1)),
        ("fc", affine(1, 1, &[1.0], 0.0)),
        ("n", ok()),
        ("ro", affine(1, 2, &[1.0, 1.0], 0.0)),
        ("out", output(1)),
    ]);
    assert_eq!(field_of(&g), ("ro".into(), "weight"));

    // Rank-3 weight.
    let rank3 = NirNode::Linear(Linear {
        weight: t(&[1, 1, 1], &[1.0]),
        metadata: Default::default(),
    });
    let g = chain(vec![
        ("in", input(1)),
        ("fc", rank3),
        ("n", ok()),
        ("out", output(1)),
    ]);
    assert_eq!(field_of(&g), ("fc".into(), "weight"));

    // Non-positive tau.
    let g = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0], &[1.0], &[1.0])));
    assert_eq!(field_of(&g), ("neuron".into(), "tau"));

    // Wrong neuron-parameter length.
    let g = layer(
        1,
        1,
        &[1.0],
        NirNode::Lif(lif(&[0.0025, 0.0025], &[1.0, 1.0], &[1.0, 1.0])),
    );
    assert_eq!(field_of(&g), ("neuron".into(), "v_leak"));

    // Non-finite weight.
    let g = layer(1, 1, &[f32::NAN], ok());
    assert_eq!(field_of(&g), ("fc".into(), "weight"));
}

#[test]
fn invalid_dt_is_rejected() {
    let g = layer(1, 1, &[1.0], NirNode::Lif(lif(&[0.0025], &[1.0], &[1.0])));
    for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let err = refused(FpgaParameterExporter::from_nir_with(
            &g,
            &NirMapOptions::default().with_dt(dt),
        ));
        assert!(
            matches!(err, MapError::InvalidParameter { field: "dt", .. }),
            "dt={dt}: {err:?}"
        );
    }
}

#[test]
fn out_of_range_gain_is_caught_by_the_checked_export() {
    // R = 1e4 with alpha = 0.4 folds to 4000: outside signed Q8.8. The mapper
    // does not clamp; the existing checked export rejects it.
    let node = NirNode::Lif(lif(&[0.0025], &[1.0e4], &[1.0]));
    let exporter = FpgaParameterExporter::from_nir(&layer(1, 1, &[1.0], node)).unwrap();
    assert!(exporter.try_export().is_err());
}

#[test]
fn mapped_bundle_writes_generic_mem_files() {
    let g = chain(vec![
        ("in", input(2)),
        ("fc1", affine(2, 2, &[1.0, 0.0, 0.0, -1.0], 0.0)),
        ("lif", NirNode::Lif(lif(&[0.0025; 2], &[1.0; 2], &[0.5; 2]))),
        ("fc2", linear(1, 2, &[1.0, 1.0])),
        ("out", output(1)),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let report = FpgaParameterExporter::from_nir(&g)
        .unwrap()
        .write_generic(dir.path())
        .unwrap();
    assert_eq!(report.readout_shape.map(|s| (s.rows, s.cols)), Some((1, 2)));
    let weights = std::fs::read_to_string(dir.path().join("parameters_weights.mem")).unwrap();
    assert_eq!(
        weights.lines().collect::<Vec<_>>(),
        ["0066", "0000", "0000", "FF9A"]
    );
    let readout =
        std::fs::read_to_string(dir.path().join("parameters_output_weights.mem")).unwrap();
    assert_eq!(readout.lines().collect::<Vec<_>>(), ["0100", "0100"]);
}
