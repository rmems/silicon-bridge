# Vendored NIR fixtures

Real `.nir` (HDF5) files written by the Python NIR ecosystem, used only by
`tests/nir_import.rs` (`nir-io` feature) to prove that `load_nir_to_mem` maps
or refuses genuine third-party exports. Nothing in `src/` depends on them, and
they are not part of the published crate.

| File | Producer | Upstream path | What it exercises |
|------|----------|---------------|-------------------|
| `lif_norse.nir` | Norse | `paper/01_lif/lif_norse.nir` | `Input` / `Affine` / `LIF` / `Output`: mapped |
| `lif_rockpool.nir` | Rockpool | `paper/01_lif/lif_rockpool.nir` | `Linear` + `LIF` with `R != 1`: mapped, gain folded |
| `two_lif_neurons.nir` | Norse | `paper/01_lif/debug_spike_representation/two_lif_neurons.nir` | two spiking layers, `v_leak != 0`: refused |
| `braille_noDelay_bias_zero.nir` | braille RNN | `paper/03_rnn/braille_noDelay_bias_zero.nir` | `CubaLIF` + recurrence: refused |
| `cnn_sinabs.nir` | Sinabs | `paper/02_cnn/cnn_sinabs.nir` | `Conv2d` / `SumPool2d`: refused |

## Provenance

- Source: [neuromorphs/NIR](https://github.com/neuromorphs/NIR) at commit
  `7883c3c85f1be27ed113ccc9e8d6ab47ab541df4`.
- Obtained from the `nir-rs` 0.4.5 crate's `tests/fixtures/` corpus
  ([Limen-Neural/nir-rs](https://github.com/Limen-Neural/nir-rs)), which vendors
  them byte-identical to upstream. **Unmodified.**
- No snnTorch export is vendored: the upstream paper corpus contains none.
  These files cover the same `Input` / `Affine`|`Linear` / `LIF` / `Output`
  wire shape that snnTorch's NIR exporter emits.

SHA-256 (verified by `tests/nir_import.rs`):

```
0dd9143ef624892d4a6461f474d93653a337b3490a62e23ec9ec5d18fde9b7b6  lif_norse.nir
f493a2cd00e20be6305acd7808faa1d4558993c91f3af43454bf637371e543d6  lif_rockpool.nir
806c7c1cfae72b5be92ce15a670f37014b82a5d4fba8a077efce46d193c21a82  two_lif_neurons.nir
f1aab3ce74024e7feac508a03b04c58d483ee3a01e293dba4b53d0b519b8650e  braille_noDelay_bias_zero.nir
e2fa55bda7aab5a772485e1b690358bcb825b303eca7dc426e3973937fcb5bcb  cnn_sinabs.nir
```

## License of the vendored files

The upstream NIR project is BSD 3-Clause licensed. That license, reproduced in
full as required by clause 2, governs the `.nir` files in this directory only
(not the rest of silicon-bridge, which is MIT OR Apache-2.0).

```
BSD 3-Clause License

Copyright (c) 2023, NIR Team.
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
