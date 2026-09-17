<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Host/HDL contract

This document pins the `silicon-bridge 0.3.1` host export contract for the
reference `silicon-hdl` profile. It is a reproducible host-to-HDL interface,
not a physical-board claim and not a statement that arbitrary dense shapes are
FPGA-compatible.

## Contract identity

| Field | Value |
|---|---|
| Profile API | `ExportConfig::silicon_hdl_v3()` |
| Profile id | `silicon-hdl-v3-compatible` |
| Contract id | `silicon-bridge-silicon-hdl-v3-contract-v1` |
| Metadata schema | `silicon-hdl-v3-profile-v1` |
| `silicon-bridge` version | `0.3.1` |
| Supported `silicon-hdl` revision | `d45163f38ac1cd88f8a3918e3793a08ace85e132` |
| Supported dimensions | 16 input channels, 16 hidden neurons, 3 output classes |

The profile records the producer crate version and the build-time
`silicon-bridge` Git revision in `parameters.json` under
`metadata.compatibility`. When the source revision is unavailable in a packaged
build, the revision field is `unknown`; release artifacts should still name the
exact committing SHA in their external provenance.

## Numeric and memory contract

All `.mem` words are uppercase 16-bit hex patterns, one word per line, for
Vivado/SystemVerilog `$readmemh`.

| Property | Contract |
|---|---|
| Word format | signed two's-complement Q8.8 |
| Width | 16 total bits, 8 fractional bits |
| Quantization | `raw = trunc(value * 256)` toward zero |
| Overflow | rejected by default before writing |
| Non-finite values | rejected before writing |
| Hidden weights | row-major `N×M`: `addr = neuron * input_channels + input` |
| Generic readout | class-major `K×N`: `addr = class * hidden_neurons + neuron` |
| silicon-hdl readout | neuron-major `N×K`: `addr = neuron * output_classes + class` |

The generic readout remains `parameters_output_weights.mem`. The HDL-native
readout for `silicon-hdl` is `hdl_readout_neuron_major.mem`. These files are
intentionally different for an asymmetric readout; do not relabel one as the
other.

## Emitted files

| File | Use |
|---|---|
| `parameters.mem` | hidden-neuron thresholds |
| `parameters_weights.mem` | hidden `N×M` weights |
| `parameters_decay.mem` | hidden-neuron decay rates |
| `parameters_output_weights.mem` | generic class-major `K×N` readout |
| `hdl_readout_neuron_major.mem` | silicon-hdl `OutputLayer` neuron-major `N×K` readout |
| `parameters.json` | full bundle plus contract metadata |

For the pinned `silicon-hdl` profile, the hidden RAMs read the first three
`.mem` files. The reference `OutputLayer` must read
`hdl_readout_neuron_major.mem`, not `parameters_output_weights.mem`.

## UART v3.0 boundary

The profile uses the existing SiliconBridge v3.0 codec layout. It does not
change the wire protocol.

| Property | Value |
|---|---|
| Sync byte | `0xAA` |
| Input channels | 16 |
| Request frame | 33 bytes: sync byte plus 16 big-endian signed Q8.8 words |
| Response frame | 36 bytes |
| Response values | 16 big-endian signed Q8.8 membrane-potential words, spike mask, switch field |

The UART stimulus helper intentionally clamps to the v3 wire range. Parameter
`.mem` export uses the full signed parameter range, so boundary values such as
`-128.0` have different UART and `.mem` encodings.

## Cold-start recipe

```rust
use silicon_bridge::{ExportConfig, FpgaParameterExporter};

let mut exporter = FpgaParameterExporter::from_params(
    thresholds, // Vec<f32>, length 16
    weights,    // Vec<Vec<f32>>, 16 rows x 16 input columns
    decay,      // Vec<f32>, length 16
);
exporter.set_output_weights(readout); // Vec<Vec<f32>>, 3 rows x 16 hidden columns

let report = exporter.write_with_config(
    "fpga_output",
    &ExportConfig::silicon_hdl_v3(),
)?;
assert_eq!(report.written[4], "hdl_readout_neuron_major.mem");
```

Verify the export before handing it to HDL tooling:

```bash
sha256sum fpga_output/*.mem fpga_output/parameters.json
jq '.metadata.compatibility' fpga_output/parameters.json
```

The manifest should show:

- `contract_id: "silicon-bridge-silicon-hdl-v3-contract-v1"`
- `silicon_hdl_revision: "d45163f38ac1cd88f8a3918e3793a08ace85e132"`
- `supported_dimensions.input_channels: 16`
- `supported_dimensions.hidden_neurons: 16`
- `supported_dimensions.output_classes: 3`
- `readout_source_layout: "class_major_kxn"`
- `readout_hdl_layout: "neuron_major_nxk"`
- `uart.protocol: "SiliconBridge v3.0"`
- `uart.request_bytes: 33`
- `uart.response_bytes: 36`

## What this proves

This profile proves that `silicon-bridge` can emit a versioned, checked,
simulation-oriented parameter bundle for the pinned `silicon-hdl` reference
contract, including the readout transpose that the HDL `OutputLayer` expects.
The Rust tests pin the file layout and metadata. The HDL contract checks should
be run from the sibling `silicon-hdl` checkout for the named revision.

This profile does not prove:

- physical Basys 3 replay;
- board programming, pin constraints, clocks, or timing closure;
- arbitrary dense dimensions on the FPGA;
- a new UART command, checksum, retry policy, or output-class byte;
- NIR import support or a crate split.

A release note for `0.3.1` may safely say: `Adds a simulation-validated,
version-pinned silicon-hdl v3 compatibility export profile that preserves the
generic dense Q8.8 default while emitting an explicit HDL-native neuron-major
readout image for the pinned reference contract. Physical-board replay remains
separate evidence.`
