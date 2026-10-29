// SPDX-License-Identifier: MIT OR Apache-2.0
//! Map a [`nir_rs::NirGraph`] onto the dense Q8.8 FPGA parameter model.
//!
//! NIR parsing, the typed graph, and HDF5 `.nir` I/O belong to
//! [`nir-rs`](https://github.com/Limen-Neural/nir-rs). This module owns only
//! the FPGA-specific half: which NIR graphs the dense `.mem` layout can
//! realize, how their parameters become thresholds / weights / decay, and what
//! is refused with [`MapError::Unsupported`].
//!
//! # Supported topology
//!
//! The hardware image is one dense spiking layer with an optional readout, so
//! the graph must be a single chain:
//!
//! ```text
//! Input → [Flatten] → Linear|Affine → LIF|IF|Threshold → [Linear|Affine] → Output
//!                      (weights)        (thresholds,        (readout)
//!                                        decay)
//! ```
//!
//! A multi-axis `Input` shape is accepted only with a `Flatten` before the
//! weights (dense layers act on the last axis).
//!
//! Anything else that is valid NIR is reported as [`MapError::Unsupported`]
//! naming the offending node, never silently approximated.
//!
//! # Time discretization
//!
//! NIR neurons are continuous-time. The FPGA neuron is the discrete update
//! `v ← decay · v + Σ w · s`. The mapping uses forward Euler with the step
//! [`NirMapOptions::dt`], expressed in the same time unit as the NIR time
//! constants (seconds for the paper corpus). With `α = dt / τ`:
//!
//! | NIR node | decay | per-neuron input gain folded into weight row | threshold |
//! |---|---|---|---|
//! | `LIF` | `1 − α` | `R · α` | `v_threshold` |
//! | `IF` | `1` | `R · dt` | `v_threshold` |
//! | `Threshold` | `0` (stateless) | `1` | `threshold` |
//!
//! `LIF` requires `dt ≤ τ` (otherwise the decay would be negative).
//!
//! # Precision
//!
//! Q8.8 truncates toward zero with a step of 1/256, so a small folded gain
//! (for example `IF` with `R = 1` at `dt = 1 ms` gives `0.001`) can underflow.
//! Individual small weights quantizing to zero is ordinary Q8.8 behaviour and
//! is not an error, but the mapping fails with [`MapError::InvalidParameter`]
//! when a neuron's whole non-zero weight row, or a non-zero threshold,
//! quantizes to zero, because the exported neuron would be dead or always
//! firing. Raise [`NirMapOptions::dt`] or rescale the network in that case.
//!
//! # Refused parameters
//!
//! The dense hardware model has no bias, leak-voltage, or reset-voltage
//! storage, so non-zero `Affine` bias, `v_leak`, and `v_reset` are
//! [`MapError::Unsupported`]. An absent or all-zero `v_reset` is accepted.
//!
//! # Node coverage
//!
//! Every [`NirNode`] variant is handled by an exhaustive `match` (no wildcard):
//!
//! | Wire type | Outcome |
//! |---|---|
//! | `Input`, `Output` | graph endpoints; shapes cross-checked |
//! | `Flatten` | transparent (does not change dense values) |
//! | `Linear`, `Affine` | weights (and readout) |
//! | `LIF`, `IF`, `Threshold` | thresholds + decay |
//! | `CubaLIF`, `CubaLI` | `Unsupported`: dual-tau (synaptic + membrane) state has no FPGA storage |
//! | `LI`, `I` | `Unsupported`: non-spiking neuron, no threshold/decay pair to emit |
//! | `Scale`, `Delay` | `Unsupported`: no element-wise scale or delay line in the dense layout |
//! | `Conv1d`, `Conv2d`, `SumPool2d`, `AvgPool2d` | `Unsupported` until an FPGA path exists |
//! | `NIRGraph` (nested) | `Unsupported` |

use crate::{FpgaParameterExporter, encode_q88_signed_full};
use nir_rs::nodes::{Lif, NirNode};
use nir_rs::types::Tensor;
use nir_rs::{NirError, NirGraph, TensorData};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Default discretization step for [`NirMapOptions`]: 1 ms.
///
/// NIR time constants in the paper corpus are in seconds (for example
/// `tau = 0.0025`). A step that does not match the step the FPGA actually
/// runs at changes the decay and the folded input gain, so set
/// [`NirMapOptions::with_dt`] explicitly for a real deployment.
pub const DEFAULT_DT: f32 = 1.0e-3;

/// Options for [`FpgaParameterExporter::from_nir_with`].
///
/// Construct with [`NirMapOptions::default`] and refine with
/// [`NirMapOptions::with_dt`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct NirMapOptions {
    /// Discretization step, in the same time unit as the NIR time constants.
    pub dt: f32,
}

impl Default for NirMapOptions {
    fn default() -> Self {
        Self { dt: DEFAULT_DT }
    }
}

impl NirMapOptions {
    /// Set the discretization step. Must be finite and `> 0`; checked when the
    /// options are used.
    #[must_use]
    pub fn with_dt(mut self, dt: f32) -> Self {
        self.dt = dt;
        self
    }
}

/// Failure mapping a NIR graph onto the FPGA parameter model.
///
/// [`MapError::Unsupported`] means the graph is valid NIR that this FPGA path
/// cannot realize; the other variants mean the graph or options are malformed
/// for mapping.
#[derive(Debug)]
#[non_exhaustive]
pub enum MapError {
    /// `nir-rs` structural validation failed (missing endpoint, duplicate edge).
    Graph(NirError),
    /// Valid NIR with no FPGA realization (see the module table).
    Unsupported {
        /// Name of the node in the graph.
        node: String,
        /// Upstream wire type of that node (`"CubaLIF"`, `"Conv2d"`, …).
        node_type: &'static str,
        /// Why the node cannot be mapped.
        reason: String,
    },
    /// The graph is not an `Input → … → Output` chain.
    InvalidTopology {
        /// What is wrong with the graph shape.
        reason: String,
    },
    /// A parameter is malformed (wrong rank, size mismatch, non-finite,
    /// non-positive time constant, …).
    InvalidParameter {
        /// Name of the node (or `"<options>"` for [`NirMapOptions`]).
        node: String,
        /// Field within the node.
        field: &'static str,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(err) => write!(f, "invalid NIR graph: {err}"),
            Self::Unsupported {
                node,
                node_type,
                reason,
            } => write!(f, "unsupported NIR node `{node}` ({node_type}): {reason}"),
            Self::InvalidTopology { reason } => write!(f, "unsupported NIR topology: {reason}"),
            Self::InvalidParameter {
                node,
                field,
                reason,
            } => write!(f, "invalid NIR parameter `{node}.{field}`: {reason}"),
        }
    }
}

impl std::error::Error for MapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Graph(err) => Some(err),
            _ => None,
        }
    }
}

impl FpgaParameterExporter {
    /// Map a NIR graph onto thresholds, weights, decay, and optional readout
    /// using [`NirMapOptions::default`].
    ///
    /// See the `nir_map` module notes (this crate's `src/nir_map.rs`) for the
    /// supported topology and discretization. Quantization to Q8.8 and `.mem` generation then follow
    /// the usual exporter path ([`Self::try_export`], [`Self::write_generic`]).
    ///
    /// ```
    /// use silicon_bridge::FpgaParameterExporter;
    /// use silicon_bridge::nir::nodes::{Affine, Input, Lif, NirNode, Output};
    /// use silicon_bridge::nir::{NirGraph, Tensor};
    ///
    /// let mut g = NirGraph::new();
    /// let input = Input { shape: vec![2], metadata: Default::default() };
    /// g.insert_node("in", NirNode::Input(input))?;
    /// g.insert_node("fc", NirNode::Affine(Affine {
    ///     weight: Tensor::from_f32(vec![1, 2], vec![1.0, -1.0])?,
    ///     bias: Tensor::from_f32(vec![1], vec![0.0])?,
    ///     metadata: Default::default(),
    /// }))?;
    /// g.insert_node("lif", NirNode::Lif(Lif {
    ///     tau: Tensor::from_f32(vec![1], vec![0.0025])?,
    ///     r: Tensor::from_f32(vec![1], vec![1.0])?,
    ///     v_leak: Tensor::from_f32(vec![1], vec![0.0])?,
    ///     v_threshold: Tensor::from_f32(vec![1], vec![0.1])?,
    ///     v_reset: None,
    ///     metadata: Default::default(),
    /// }))?;
    /// let output = Output { shape: vec![1], metadata: Default::default() };
    /// g.insert_node("out", NirNode::Output(output))?;
    /// g.add_edge("in", "fc");
    /// g.add_edge("fc", "lif");
    /// g.add_edge("lif", "out");
    ///
    /// let params = FpgaParameterExporter::from_nir(&g)?.try_export()?;
    /// // dt = 1 ms, tau = 2.5 ms: alpha = 0.4, decay = 0.6, gain = R * alpha = 0.4.
    /// assert_eq!(params.decay_rates, vec![0x0099]); // 0.6 * 256, truncated
    /// assert_eq!(params.weights, vec![0x0066, -0x0066]); // +/-0.4 * 256, truncated
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`MapError`] when the graph is not a supported single-layer chain or a
    /// parameter cannot be represented.
    pub fn from_nir(graph: &NirGraph) -> Result<Self, MapError> {
        Self::from_nir_with(graph, &NirMapOptions::default())
    }

    /// [`Self::from_nir`] with an explicit discretization step.
    ///
    /// # Errors
    ///
    /// [`MapError`] as for [`Self::from_nir`]; additionally
    /// [`MapError::InvalidParameter`] when `options.dt` is not finite and
    /// positive.
    pub fn from_nir_with(graph: &NirGraph, options: &NirMapOptions) -> Result<Self, MapError> {
        let dt = f64::from(options.dt);
        if !dt.is_finite() || dt <= 0.0 {
            return Err(invalid("<options>", "dt", "must be finite and > 0"));
        }
        graph.validate_structure().map_err(MapError::Graph)?;

        // Reject unsupported kinds in insertion order first so the report
        // names the most fundamental problem (a CubaLIF, not the recurrence
        // that usually accompanies it).
        for (name, node) in &graph.nodes {
            if let Role::Unsupported(reason) = role(node) {
                return Err(unsupported(name, node, reason));
            }
        }

        let chain = walk_chain(graph)?;
        let layers = split_layers(&chain)?;

        let (hidden, input_count) = hidden_weights(layers.weights.0, layers.weights.1)?;
        let neuron_count = hidden.rows;
        // `Linear`/`Affine` act on the last axis, so a multi-axis input only
        // feeds one dense layer after a `Flatten`.
        let input_shape = input_shape(layers.input.1);
        if !layers.flattened && input_shape.len() > 1 {
            return Err(invalid(
                layers.input.0,
                "shape",
                &format!(
                    "multi-axis input {input_shape:?} needs a Flatten before the weights; \
                     Linear/Affine would otherwise act per row"
                ),
            ));
        }
        check_endpoint_shape(layers.input.0, input_shape, input_count, "input size")?;

        let neurons = map_neuron(layers.neuron.0, layers.neuron.1, neuron_count, dt)?;
        let weights = scale_rows(&hidden, &neurons.gain, layers.weights.0)?;
        let thresholds = to_f32_vec(layers.neuron.0, "threshold", &neurons.thresholds)?;
        check_thresholds(layers.neuron.0, &thresholds)?;
        let decay = to_f32_vec(layers.neuron.0, "decay", &neurons.decay)?;

        let mut exporter = Self::from_params(thresholds, weights, decay);
        let output_len = if let Some((name, node)) = layers.readout {
            let (readout, readout_in) = hidden_weights(name, node)?;
            if readout_in != neuron_count {
                return Err(invalid(
                    name,
                    "weight",
                    &format!(
                        "readout expects {readout_in} inputs but the neuron layer has \
                         {neuron_count} neurons"
                    ),
                ));
            }
            let rows = readout
                .values
                .chunks(readout.cols)
                .map(|row| to_f32_vec(name, "weight", row))
                .collect::<Result<Vec<_>, _>>()?;
            exporter.set_output_weights(rows);
            readout.rows
        } else {
            neuron_count
        };
        check_endpoint_shape(
            layers.output.0,
            output_shape(layers.output.1),
            output_len,
            "output size",
        )?;
        Ok(exporter)
    }
}

/// What a node means to the dense FPGA mapping.
enum Role {
    Input,
    Output,
    Flatten,
    Weights,
    Neuron,
    Unsupported(&'static str),
}

/// Exhaustive on purpose: a new upstream wire type fails to compile here
/// instead of falling into a wildcard arm.
fn role(node: &NirNode) -> Role {
    match node {
        NirNode::Input(_) => Role::Input,
        NirNode::Output(_) => Role::Output,
        NirNode::Flatten(_) => Role::Flatten,
        NirNode::Linear(_) | NirNode::Affine(_) => Role::Weights,
        NirNode::Lif(_) | NirNode::If(_) | NirNode::Threshold(_) => Role::Neuron,
        NirNode::CubaLif(_) | NirNode::CubaLi(_) => Role::Unsupported(
            "dual-tau (synaptic + membrane) dynamics need a second state variable per neuron; \
             the dense layout stores one decay",
        ),
        NirNode::Li(_) | NirNode::I(_) => Role::Unsupported(
            "non-spiking integrator has no threshold/decay pair in the dense layout",
        ),
        NirNode::Scale(_) => {
            Role::Unsupported("element-wise scale is not part of the dense layout")
        }
        NirNode::Delay(_) => Role::Unsupported("the dense layout has no delay line"),
        NirNode::Conv1d(_) | NirNode::Conv2d(_) => {
            Role::Unsupported("convolution has no FPGA path yet")
        }
        NirNode::SumPool2d(_) | NirNode::AvgPool2d(_) => {
            Role::Unsupported("pooling has no FPGA path yet")
        }
        NirNode::Graph(_) => Role::Unsupported("nested subgraphs have no FPGA path yet"),
    }
}

fn unsupported(name: &str, node: &NirNode, reason: &str) -> MapError {
    MapError::Unsupported {
        node: name.to_string(),
        node_type: node.type_name(),
        reason: reason.to_string(),
    }
}

fn invalid(node: &str, field: &'static str, reason: &str) -> MapError {
    MapError::InvalidParameter {
        node: node.to_string(),
        field,
        reason: reason.to_string(),
    }
}

fn topology(reason: impl Into<String>) -> MapError {
    MapError::InvalidTopology {
        reason: reason.into(),
    }
}

type Named<'a> = (&'a str, &'a NirNode);

/// Follow the single `Input → … → Output` path.
fn walk_chain(graph: &NirGraph) -> Result<Vec<Named<'_>>, MapError> {
    let mut inputs = graph
        .nodes
        .iter()
        .filter(|(_, node)| matches!(role(node), Role::Input));
    let Some((input_name, _)) = inputs.next() else {
        return Err(topology("no Input node"));
    };
    if inputs.next().is_some() {
        return Err(topology("more than one Input node"));
    }

    let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, to) in &graph.edges {
        successors.entry(from).or_default().push(to);
    }

    let mut chain: Vec<Named<'_>> = Vec::with_capacity(graph.nodes.len());
    let mut seen: HashSet<&str> = HashSet::new();
    let mut current = input_name.as_str();
    loop {
        // `validate_structure` guarantees every edge endpoint exists.
        let node = &graph.nodes[current];
        seen.insert(current);
        chain.push((current, node));
        match successors.get(current).map(Vec::as_slice).unwrap_or(&[]) {
            [] => break,
            [next] => {
                if seen.contains(next) {
                    return Err(unsupported(
                        next,
                        &graph.nodes[*next],
                        "recurrent connections are not supported",
                    ));
                }
                current = next;
            }
            _ => {
                return Err(unsupported(
                    current,
                    node,
                    "fan-out (more than one outgoing edge) is not supported",
                ));
            }
        }
    }

    let (last_name, last) = *chain.last().expect("chain starts with the Input node");
    if !matches!(role(last), Role::Output) {
        return Err(topology(format!(
            "path from Input ends at `{last_name}` ({}), not an Output node",
            last.type_name()
        )));
    }
    if let Some((name, _)) = graph.nodes.iter().find(|(n, _)| !seen.contains(n.as_str())) {
        return Err(topology(format!(
            "node `{name}` is not on the single Input → Output path"
        )));
    }
    Ok(chain)
}

struct Layers<'a> {
    input: Named<'a>,
    /// A `Flatten` sits between `Input` and the first weights.
    flattened: bool,
    weights: Named<'a>,
    neuron: Named<'a>,
    readout: Option<Named<'a>>,
    output: Named<'a>,
}

/// Split the chain into input / weights / neuron / optional readout / output.
fn split_layers<'a>(chain: &[Named<'a>]) -> Result<Layers<'a>, MapError> {
    let input = chain[0];
    let output = *chain.last().expect("non-empty chain");
    let flattened = chain[1..]
        .iter()
        .take_while(|(_, node)| !matches!(role(node), Role::Weights))
        .any(|(_, node)| matches!(role(node), Role::Flatten));
    let mut middle = chain[1..chain.len() - 1]
        .iter()
        .copied()
        .filter(|(_, node)| !matches!(role(node), Role::Flatten))
        .peekable();

    let weights = match middle.next() {
        Some(named) if matches!(role(named.1), Role::Weights) => named,
        Some((name, node)) => {
            return Err(unsupported(
                name,
                node,
                "expected Linear/Affine weights between Input and the neuron layer",
            ));
        }
        None => return Err(topology("no neuron layer between Input and Output")),
    };
    let neuron = match middle.next() {
        Some(named) if matches!(role(named.1), Role::Neuron) => named,
        Some((name, node)) => {
            return Err(unsupported(
                name,
                node,
                "expected LIF/IF/Threshold directly after the weights",
            ));
        }
        None => return Err(topology("weights are not followed by a neuron layer")),
    };
    let readout = match middle.next() {
        None => None,
        Some(named) if matches!(role(named.1), Role::Weights) => Some(named),
        Some((name, node)) => {
            return Err(unsupported(
                name,
                node,
                "only one spiking layer plus an optional Linear/Affine readout is supported",
            ));
        }
    };
    if let Some((name, node)) = middle.next() {
        return Err(unsupported(
            name,
            node,
            "extra layer after the readout; only one spiking layer is supported",
        ));
    }
    Ok(Layers {
        input,
        flattened,
        weights,
        neuron,
        readout,
        output,
    })
}

/// Dense row-major `rows × cols` matrix of `f64`.
struct Matrix {
    rows: usize,
    cols: usize,
    values: Vec<f64>,
}

/// Extract the `(out, in)` weight matrix of a `Linear`/`Affine` node, after
/// requiring a zero bias. Returns the matrix and its input width.
fn hidden_weights(name: &str, node: &NirNode) -> Result<(Matrix, usize), MapError> {
    let (weight, bias) = match node {
        NirNode::Linear(n) => (&n.weight, None),
        NirNode::Affine(n) => (&n.weight, Some(&n.bias)),
        _ => unreachable!("caller checked Role::Weights"),
    };
    if let Some(bias) = bias {
        let bias = tensor_values(name, "bias", bias)?;
        if bias.iter().any(|b| *b != 0.0) {
            return Err(unsupported(
                name,
                node,
                "non-zero bias: the dense layout has no bias storage",
            ));
        }
    }
    let &[rows, cols] = weight.shape() else {
        return Err(invalid(
            name,
            "weight",
            &format!(
                "expected a rank-2 (out, in) matrix, found shape {:?}",
                weight.shape()
            ),
        ));
    };
    if rows == 0 || cols == 0 {
        return Err(invalid(name, "weight", "matrix has a zero dimension"));
    }
    let values = tensor_values(name, "weight", weight)?;
    Ok((Matrix { rows, cols, values }, cols))
}

/// Per-neuron results of mapping the spiking node.
struct NeuronMap {
    thresholds: Vec<f64>,
    decay: Vec<f64>,
    /// Factor folded into each weight row.
    gain: Vec<f64>,
}

fn map_neuron(name: &str, node: &NirNode, n: usize, dt: f64) -> Result<NeuronMap, MapError> {
    match node {
        NirNode::Lif(lif) => map_lif(name, node, lif, n, dt),
        NirNode::If(neuron) => {
            require_zero_reset(name, node, neuron.v_reset.as_ref())?;
            let r = per_neuron(name, "r", &neuron.r, n)?;
            Ok(NeuronMap {
                thresholds: per_neuron(name, "v_threshold", &neuron.v_threshold, n)?,
                decay: vec![1.0; n],
                gain: r.iter().map(|r| r * dt).collect(),
            })
        }
        NirNode::Threshold(neuron) => Ok(NeuronMap {
            thresholds: per_neuron(name, "threshold", &neuron.threshold, n)?,
            decay: vec![0.0; n],
            gain: vec![1.0; n],
        }),
        _ => unreachable!("caller checked Role::Neuron"),
    }
}

fn map_lif(
    name: &str,
    node: &NirNode,
    lif: &Lif,
    n: usize,
    dt: f64,
) -> Result<NeuronMap, MapError> {
    let leak = per_neuron(name, "v_leak", &lif.v_leak, n)?;
    if leak.iter().any(|v| *v != 0.0) {
        return Err(unsupported(
            name,
            node,
            "non-zero v_leak: the dense layout has no leak-voltage storage",
        ));
    }
    require_zero_reset(name, node, lif.v_reset.as_ref())?;
    let tau = per_neuron(name, "tau", &lif.tau, n)?;
    let r = per_neuron(name, "r", &lif.r, n)?;
    let mut decay = Vec::with_capacity(n);
    let mut gain = Vec::with_capacity(n);
    for (tau, r) in tau.iter().zip(&r) {
        if *tau <= 0.0 {
            return Err(invalid(name, "tau", "time constant must be > 0"));
        }
        let alpha = dt / tau;
        if alpha > 1.0 {
            return Err(invalid(
                name,
                "tau",
                &format!("dt ({dt}) exceeds tau ({tau}): decay would be negative; lower dt"),
            ));
        }
        decay.push(1.0 - alpha);
        gain.push(r * alpha);
    }
    Ok(NeuronMap {
        thresholds: per_neuron(name, "v_threshold", &lif.v_threshold, n)?,
        decay,
        gain,
    })
}

fn require_zero_reset(
    name: &str,
    node: &NirNode,
    v_reset: Option<&Tensor>,
) -> Result<(), MapError> {
    if let Some(v_reset) = v_reset
        && tensor_values(name, "v_reset", v_reset)?
            .iter()
            .any(|v| *v != 0.0)
    {
        return Err(unsupported(
            name,
            node,
            "non-zero v_reset: the dense layout resets to zero",
        ));
    }
    Ok(())
}

/// Multiply weight row `i` by `gain[i]` and narrow to the public `f32` API.
///
/// Fails when a row with non-zero weights quantizes entirely to Q8.8 zero.
fn scale_rows(weights: &Matrix, gain: &[f64], name: &str) -> Result<Vec<Vec<f32>>, MapError> {
    weights
        .values
        .chunks(weights.cols)
        .zip(gain)
        .enumerate()
        .map(|(neuron, (row, gain))| {
            let scaled: Vec<f64> = row.iter().map(|w| w * gain).collect();
            let scaled = to_f32_vec(name, "weight", &scaled)?;
            let any_nonzero = row.iter().any(|w| *w != 0.0);
            if any_nonzero && scaled.iter().all(|w| encode_q88_signed_full(*w) == 0) {
                return Err(invalid(
                    name,
                    "weight",
                    &format!(
                        "every non-zero weight of neuron {neuron} quantizes to 0 in Q8.8 after \
                         folding the input gain {gain:e}; raise NirMapOptions::dt or rescale \
                         the network"
                    ),
                ));
            }
            Ok(scaled)
        })
        .collect()
}

/// Reject a non-zero threshold that Q8.8 truncates to 0 (always-firing neuron).
fn check_thresholds(name: &str, thresholds: &[f32]) -> Result<(), MapError> {
    for (neuron, threshold) in thresholds.iter().enumerate() {
        if *threshold != 0.0 && encode_q88_signed_full(*threshold) == 0 {
            return Err(invalid(
                name,
                "threshold",
                &format!("threshold {threshold:e} of neuron {neuron} quantizes to 0 in Q8.8"),
            ));
        }
    }
    Ok(())
}

fn to_f32_vec(name: &str, field: &'static str, values: &[f64]) -> Result<Vec<f32>, MapError> {
    values
        .iter()
        .map(|v| {
            let narrowed = *v as f32;
            if narrowed.is_finite() {
                Ok(narrowed)
            } else {
                Err(invalid(name, field, "value does not fit in f32"))
            }
        })
        .collect()
}

/// Read a tensor as finite `f64` values.
fn tensor_values(name: &str, field: &'static str, tensor: &Tensor) -> Result<Vec<f64>, MapError> {
    let values: Vec<f64> = match tensor.data() {
        TensorData::F32(v) => v.iter().map(|x| f64::from(*x)).collect(),
        TensorData::F64(v) => v.clone(),
        TensorData::I64(v) => v.iter().map(|x| *x as f64).collect(),
        TensorData::Bool(_) => {
            return Err(invalid(name, field, "boolean tensor is not numeric"));
        }
    };
    if values.iter().any(|v| !v.is_finite()) {
        return Err(invalid(name, field, "contains NaN or infinity"));
    }
    Ok(values)
}

/// Read a neuron parameter that is either one value per neuron or a scalar
/// broadcast across the layer.
fn per_neuron(name: &str, field: &'static str, t: &Tensor, n: usize) -> Result<Vec<f64>, MapError> {
    let values = tensor_values(name, field, t)?;
    match values.len() {
        len if len == n => Ok(values),
        1 => Ok(vec![values[0]; n]),
        len => Err(invalid(
            name,
            field,
            &format!("has {len} values but the layer has {n} neurons"),
        )),
    }
}

fn input_shape(node: &NirNode) -> &[usize] {
    match node {
        NirNode::Input(n) => &n.shape,
        _ => unreachable!("chain starts with Input"),
    }
}

fn output_shape(node: &NirNode) -> &[usize] {
    match node {
        NirNode::Output(n) => &n.shape,
        _ => unreachable!("chain ends with Output"),
    }
}

/// Compare the element count of an endpoint shape with the matrix dimension.
fn check_endpoint_shape(
    name: &str,
    shape: &[usize],
    expected: usize,
    what: &'static str,
) -> Result<(), MapError> {
    let actual = shape
        .iter()
        .try_fold(1usize, |acc, dim| acc.checked_mul(*dim));
    if actual != Some(expected) {
        return Err(invalid(
            name,
            "shape",
            &format!("{what} {expected} does not match shape {shape:?}"),
        ));
    }
    Ok(())
}

#[cfg(feature = "nir-io")]
pub use io::{LoadNirError, load_nir_to_mem, load_nir_to_mem_with};

#[cfg(feature = "nir-io")]
mod io {
    //! `.nir` file → `.mem` bundle, delegating all HDF5 work to `nir_rs::io`.

    use super::{MapError, NirMapOptions};
    use crate::{ExportError, ExportReport, FpgaParameterExporter};
    use nir_rs::NirError;
    use std::fmt;
    use std::path::Path;

    /// Failure from [`load_nir_to_mem`].
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum LoadNirError {
        /// `nir_rs::io::read` could not load the `.nir` file.
        Read(NirError),
        /// The graph loaded but cannot be mapped onto the FPGA model.
        Map(MapError),
        /// The mapped parameters failed validation or the `.mem` write failed.
        Export(ExportError),
    }

    impl fmt::Display for LoadNirError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Read(err) => write!(f, "failed to read .nir file: {err}"),
                Self::Map(err) => write!(f, "{err}"),
                Self::Export(err) => write!(f, "failed to export .mem files: {err}"),
            }
        }
    }

    impl std::error::Error for LoadNirError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::Read(err) => Some(err),
                Self::Map(err) => Some(err),
                Self::Export(err) => Some(err),
            }
        }
    }

    impl From<NirError> for LoadNirError {
        fn from(err: NirError) -> Self {
            Self::Read(err)
        }
    }

    impl From<MapError> for LoadNirError {
        fn from(err: MapError) -> Self {
            Self::Map(err)
        }
    }

    impl From<ExportError> for LoadNirError {
        fn from(err: ExportError) -> Self {
            Self::Export(err)
        }
    }

    /// Read a `.nir` file with `nir_rs::io::read`, map it with
    /// [`FpgaParameterExporter::from_nir`], and write the generic dense Q8.8
    /// `.mem` bundle into `out_dir` ([`FpgaParameterExporter::write_generic`]).
    ///
    /// Existing files in `out_dir` are never replaced
    /// ([`crate::OverwritePolicy`] of the generic profile).
    ///
    /// # Errors
    ///
    /// [`LoadNirError::Read`], [`LoadNirError::Map`], or
    /// [`LoadNirError::Export`]. Nothing is written unless mapping and
    /// validation succeed.
    pub fn load_nir_to_mem(
        path: impl AsRef<Path>,
        out_dir: impl AsRef<Path>,
    ) -> Result<ExportReport, LoadNirError> {
        load_nir_to_mem_with(path, out_dir, &NirMapOptions::default())
    }

    /// [`load_nir_to_mem`] with an explicit [`NirMapOptions`].
    ///
    /// # Errors
    ///
    /// As for [`load_nir_to_mem`].
    pub fn load_nir_to_mem_with(
        path: impl AsRef<Path>,
        out_dir: impl AsRef<Path>,
        options: &NirMapOptions,
    ) -> Result<ExportReport, LoadNirError> {
        let graph = nir_rs::io::read(path)?;
        let exporter = FpgaParameterExporter::from_nir_with(&graph, options)?;
        Ok(exporter.write_generic(out_dir)?)
    }
}
