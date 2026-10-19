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

CI runs the script in the `hdl-golden` job (`.github/workflows/ci.yml`,
ubuntu-latest, `apt-get install iverilog`). Before compiling, `run.sh` calls
`gen_expected_checks.py` (python3, standard library only) to turn
`generic_4x6/expected.json` (all 24 `row_major_weight_hex` words) and
`spikenaut_16/expected.json` (the 5 `pinned_hidden_addresses`) into
`check_hex` calls on the words `$readmemh` loaded. The testbench pulls them in
under `-DEXPECTED_JSON_CHECKS`. A drift between `expected.json` and the
committed `.mem` images fails the run, and so does a missing pass marker or
any `FAIL` line, even on an Icarus build whose `vvp` exits 0 after `$fatal`.

## Result recorded with this contract

Icarus Verilog 12.0 (`iverilog -g2012` / `vvp`) on 2026-09-15, repo root
working directory, peer pin `d45163f38ac1cd88f8a3918e3793a08ace85e132`:

```text
TB_READMEMH_CONTRACT: ALL TESTS PASSED
peer silicon-hdl commit d45163f38ac1cd88f8a3918e3793a08ace85e132
evidence: HDL simulation (Icarus/Verilator), not board-measured parity
```

Re-recorded with the `expected.json` checks on 2026-10-19 (Icarus Verilog
12.0, same pin):

```text
gen_expected_checks: 29 word checks from expected.json
TB_READMEMH_CONTRACT: ALL TESTS PASSED (29 expected.json word checks)
peer silicon-hdl commit d45163f38ac1cd88f8a3918e3793a08ace85e132
```

Negative control: with `generic_4x6/expected.json` `row_major_weight_hex[7]`
changed from `7FFF` to `7FFE` in a scratch copy, the run prints
`FAIL: expected.json generic_4x6 row_major_weight_hex[7] (got 0x7fff, expected 0x7ffe)`
and exits 1.

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
not silently edit expected hex. Tracked as item 24 in
[`docs/tech-debt-register.md`](../../../docs/tech-debt-register.md).
