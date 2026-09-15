<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Rust↔HDL golden export contracts

Independent fixtures for [silicon-bridge #53](https://github.com/rmems/silicon-bridge/issues/53).
Expected words, `.mem` lines, signedness, dimensions, flattening, rounding,
overflow policy, and schema version are specified here. They are **not**
produced by `encode_q88_*` or `write_mem_files`.

This extends the #22 writer tests and the #49 encoding table. It does not
reopen those issues. UART request/response golden bytes wait for #52.

## What is claimed

- Exported 16-bit words match these independently specified hex values.
- Hidden-weight addressing is dense row-major
  (`addr = neuron * channels + input`), the layout silicon-hdl
  `LifNeuronArray` consumes. Not CSR.
- A signed HDL reader (`$signed` + saturate-add) treats `FF00` as `-1.0`
  and a negative weight subtracts. See `hdl/SIMULATION.md`.

## What is not claimed

- Software-vs-FPGA SNN trajectory parity
- Board programming or live UART traffic
- That fixture weights are trained or useful
- That #50 generic/Spikenaut profile APIs exist (they do not yet; both
  bundles still carry the current `Spikenaut-v2` writer tag)

## Layout

| Path | Role |
|---|---|
| `provenance.json` | Schema/profile notes, rounding, overflow, HDL pin, UART deferral |
| `q88_signed.json` | `-1→FF00`, `-0.5→FF80`, `0→0000`, `0.5→0080`, `1→0100`, `-128→8000`, `127.99609375→7FFF` |
| `q88_unsigned.json` | Full unsigned range and the dual meaning of `FFFF` |
| `generic_4x6/` | Non-square 4×6 signed layer, no readout |
| `spikenaut_16/` | Synthetic 16×16 hidden + 3×16 signed readout |
| `hdl/` | Vendored `WeightRam` + `$readmemh` testbench |

`checksums.sha256` pins every fixture file in this tree.

## Readout mismatch (recorded)

`spikenaut_16/parameters_output_weights.mem` is the K×N image the current
exporter writes. `spikenaut_16/hdl_readout_neuron_major.mem` is the same
values addressed the way silicon-hdl `OutputLayer` indexes RAM. They are
not the same byte stream. See `provenance.json` → `peer_hdl.readout_mismatch`.
