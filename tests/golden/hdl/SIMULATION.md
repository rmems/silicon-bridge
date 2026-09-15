<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# HDL simulation evidence (silicon-bridge #53)

This directory is **simulation evidence**, not board-measured parity and not
validated supervision.

## Peer pin

| Item | Value |
|---|---|
| Repository | https://github.com/rmems/silicon-hdl |
| Commit | `d45163f38ac1cd88f8a3918e3793a08ace85e132` |
| Vendored RTL | `WeightRam.sv` (unsigned `logic [15:0]` `$readmemh` RAM) |
| Consumer arithmetic | `LifNeuron.sv` / `OutputLayer.sv` `$signed` extend, saturate-add, `$signed` compare (GH#73) |

`WeightRam` storage type is an unsigned container. Signedness is applied by
the neuron / readout consumers. The testbench therefore sign-extends loaded
words and accumulates with the same guard-bit saturate idiom, including a
negative (Dale-inhibitory) contribution.

## How to run

From the silicon-bridge repository root, with Icarus Verilog installed:

```bash
bash tests/golden/hdl/run.sh
```

A passing run prints `TB_READMEMH_CONTRACT: ALL TESTS PASSED` and the peer
commit. Ordinary `cargo test` does **not** invoke this script and does not
require Vivado, Icarus, Verilator, or a board.

## Result recorded with this contract

Icarus Verilog 12.0 (`iverilog -g2012` / `vvp`) on 2026-09-15, repo root
working directory, peer pin `d45163f38ac1cd88f8a3918e3793a08ace85e132`:

```text
TB_READMEMH_CONTRACT: ALL TESTS PASSED
peer silicon-hdl commit d45163f38ac1cd88f8a3918e3793a08ace85e132
evidence: HDL simulation (Icarus/Verilator), not board-measured parity
```

`$readmemh` warns when the image is shorter than `2**ADDR_WIDTH` (24 words
into a 32-deep generic bank; 48 words into a 64-deep readout bank). That is
the documented silicon-hdl load rule (`min(file lines, depth)`); unused
addresses are not part of the contract. A missing simulator is not a
Rust-test failure.

## Honest mismatch

Hidden weights: both sides use dense row-major
`addr = neuron * num_channels + input`.

Readout: silicon-bridge writes `K` class rows × `N` hidden columns
(class-major). silicon-hdl `OutputLayer` indexes
`addr = neuron * NUM_CLASSES + class` (neuron-major). Both images are
committed (`parameters_output_weights.mem` vs
`hdl_readout_neuron_major.mem`). The testbench checks both and requires
them to differ for this fixture. Neither file is rewritten to hide the
gap. A coordinated silicon-hdl or #50 profile repair should bump the pin,
not silently edit expected hex.
