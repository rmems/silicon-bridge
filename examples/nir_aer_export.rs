// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lower an in-memory `NirGraph` to a `silicon-hdl-v3 + aer-route-v1` bundle.
//!
//! Builds the canonical Spikenaut-shaped graph
//! `Input[16] → Linear[16×16] → LIF[16] → Linear[3×16] → Output[3]`, derives
//! the identity `aer-route-v1` table from the canonical input ordering, and
//! writes `aer_routes.mem` plus `parameters.json` provenance
//! (`aer_routing`, `artifact_hashes`) under `./nir_aer_out/`.
//!
//! Passing this example is **not** board evidence: consuming the bundle on
//! Basys 3 is the silicon-hdl-side acceptance step recorded outside this
//! repository.
//!
//! Run: `cargo run --features nir --example nir_aer_export`

use silicon_bridge::FpgaParameterExporter;
use silicon_bridge::nir::nodes::{Input, Lif, Linear, NirNode, Output};
use silicon_bridge::nir::{NirGraph, Tensor};

fn t(shape: &[usize], data: &[f32]) -> Tensor {
    Tensor::from_f32(shape.to_vec(), data.to_vec()).expect("tensor")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut w = vec![0.0f32; 16 * 16];
    for i in 0..16 {
        w[i * 16 + i] = 1.0;
    }
    let mut ro = vec![0.0f32; 3 * 16];
    ro[0] = 1.0;
    ro[16 + 1] = 0.5;
    ro[2 * 16 + 15] = 1.0;

    let mut g = NirGraph::new();
    g.insert_node(
        "in",
        NirNode::Input(Input {
            shape: vec![16],
            metadata: Default::default(),
        }),
    )?;
    g.insert_node(
        "fc",
        NirNode::Linear(Linear {
            weight: t(&[16, 16], &w),
            metadata: Default::default(),
        }),
    )?;
    g.insert_node(
        "lif",
        NirNode::Lif(Lif {
            tau: t(&[16], &[0.0025; 16]),
            r: t(&[16], &[1.0; 16]),
            v_leak: t(&[16], &[0.0; 16]),
            v_threshold: t(&[16], &[0.5; 16]),
            v_reset: None,
            metadata: Default::default(),
        }),
    )?;
    g.insert_node(
        "ro",
        NirNode::Linear(Linear {
            weight: t(&[3, 16], &ro),
            metadata: Default::default(),
        }),
    )?;
    g.insert_node(
        "out",
        NirNode::Output(Output {
            shape: vec![3],
            metadata: Default::default(),
        }),
    )?;
    g.add_edge("in", "fc");
    g.add_edge("fc", "lif");
    g.add_edge("lif", "ro");
    g.add_edge("ro", "out");

    // `None` derives the identity table from canonical input ordering; an
    // `Some(table)` explicit hardware plan is validated instead — ordinary
    // NIR edges never become FPGA hops.
    // `allow_replace` keeps the demo idempotent across runs.
    let config = FpgaParameterExporter::aer_export_config_from_nir(&g, None)?.allow_replace();
    let exporter = FpgaParameterExporter::from_nir(&g)?;

    let dir = std::path::Path::new("nir_aer_out");
    std::fs::create_dir_all(dir)?;
    let report = exporter.write_with_config(dir, &config)?;
    println!("wrote {} files to {}", report.written.len(), dir.display());
    for name in &report.written {
        println!("  {name}");
    }
    Ok(())
}
