<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Consumer examples

These binaries use only the **public** `silicon-bridge` API. They run offline
after Cargo dependency resolution: no FPGA, no serial device, no private
paths, and no sibling checkouts.

| Example | What it shows |
|---|---|
| `generic_mem_export` | 4×6 checked export, custom output dir, deterministic metadata, negative weights, shape/non-finite/overflow errors |
| `spikenaut_profile_export` | Synthetic 16-neuron bundle + signed readout under the `Spikenaut-v2` **layout** tag |
| `dense_codec` | SiliconBridge v3 and a host-only dense-8 frame; never opens a port |

```bash
cargo run --example generic_mem_export
cargo run --example spikenaut_profile_export
cargo run --example dense_codec
```

Do not pass `--features uart` to send live stimuli. Opening a named serial
device is a separate, explicit step documented in the README feature matrix.
The `ping()` method sends a real 16-channel stimulus and is not part of
these examples.
