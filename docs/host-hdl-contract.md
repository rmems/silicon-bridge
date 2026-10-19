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
| Contract id | `silicon-bridge-silicon-hdl-v3-contract-v2` |
| Metadata schema | `silicon-hdl-v3-profile-v2` |
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

## Q8.8 encode/decode sites

Every place in the crate that turns an `f32` into a Q8.8 word, or a word back
into an `f32`. All encoders compute `value × 256` and truncate toward zero; no
site rounds to nearest, and no site wraps (saturation always preserves sign).
`tests/q88_cross_path.rs` checks all of them against each other and against an
independent `f64` reference model. It covers every one of the 65,536 words
(decode agreement and decode→encode identity) and a dense `f32` grid of
about 730,000 points: ±0, ±1 LSB, ±127.99, ±128, values one ulp past each
saturation edge, `f32::MIN`/`MAX`, NaN payloads, ±∞, and a strided walk over
all 2³² bit patterns.

### Encoders

| Site | Family | Rounding | Saturation (finite out-of-range) | NaN | ±∞ |
|---|---|---|---|---|---|
| `encode_q88_signed_full` | signed parameter | trunc toward 0 | `i16::MIN`/`MAX` (`8000`/`7FFF`), i.e. `[-128, 127.99609375]` | `0` | saturates to `8000`/`7FFF` |
| `FixedPointEncode::encode_q88` / `FpgaParameterExporter::to_q88` | signed parameter | = `encode_q88_signed_full` | same | `0` | saturates |
| `format_q88_hex` | signed parameter (hex text) | = `encode_q88_signed_full` | same | `"0000"` | saturates |
| `ParameterExport::export` (legacy, infallible) | signed parameter, ignores `set_encoding` | = `encode_q88_signed_full` | same, silent | `0` | saturates |
| `try_export` / `CheckedParameterExport::try_export` (`RangePolicy::Reject`, default) | per-block `Q88Encoding` | trunc toward 0 | **rejected** (`OutOfRange`) | **rejected** (`NonFinite`) | **rejected** (`NonFinite`) |
| `try_export_with_report` (`RangePolicy::Saturate`) | per-block `Q88Encoding` | trunc toward 0 | clamped to the encoding range, each clamp recorded in `SaturationReport` (same words as the legacy path) | **rejected** | **rejected** |
| `MemFileWriter::write_mem_files` / `write_generic` / `write_with_config` | signed only (`Unsigned` refused) | via `try_export` | **rejected**, no partial files | **rejected** | **rejected** |
| `encode_q88_signed` | legacy UART clamp | trunc toward 0 | unscaled clamp to `±127.99` → raw `±32765` (`7FFD`/`8003`) | `0` | saturates to `±32765` |
| `encode_stimuli_legacy_v3` | legacy UART clamp | = `encode_q88_signed` | same | `0` | saturates |
| `encode_stimuli` (checked) | legacy UART clamp | = `encode_q88_signed` | same (clamped, not rejected) | **rejected** (`CodecError::NonFinite`) | **rejected** |
| `encode_request_v4` (checked) | legacy UART clamp | = `encode_stimuli` | same | **rejected** | **rejected** |
| `encode_q88_unsigned` | unsigned magnitude | trunc toward 0 | `0`/`65535`, i.e. `[0, 255.99609375]`; negatives → `0` | `0` | saturates to `0`/`FFFF` |
| `try_export` with `Q88Encoding::Unsigned` | unsigned magnitude | = `encode_q88_unsigned`, stored `u16 as i16` | **rejected** | **rejected** | **rejected** |

The signed parameter and UART families agree bit-for-bit whenever
`|v| <= 127.99`. They differ only in the last three words on each side
(`±32766`, `±32767`, `-32768`), which the UART helper never emits.

### Decoders

| Site | Interpretation | Agreement |
|---|---|---|
| `q88_signed_to_f32` | signed, `raw / 256` | exact for every `i16` |
| `decode_response` (v3 / dense) | signed, big-endian | bit-identical to `q88_signed_to_f32` for all 65,536 words |
| `decode_response_v4` | signed, big-endian, after CRC/id checks | bit-identical to `q88_signed_to_f32` for all 65,536 words |
| `q88_to_f32` | unsigned, `raw / 256` | equals the signed decode for `raw >= 0` and is exactly `+256.0` for `raw < 0` (by design) |

Decode→`f32`→re-encode is the identity on every signed parameter path for all
65,536 words, on the UART paths for `-32765..=32765` (outside that, the word
saturates to `±32765`), and on the unsigned paths for all 65,536 `u16`
patterns.

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

- `contract_id: "silicon-bridge-silicon-hdl-v3-contract-v2"`
- `reference.name: "silicon-hdl"`
- `reference.repository: "https://github.com/rmems/silicon-hdl"`
- `reference.revision: "d45163f38ac1cd88f8a3918e3793a08ace85e132"`
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
