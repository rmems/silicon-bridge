<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Consumer examples

These binaries use only the **public** `silicon-bridge` API. After Cargo
dependency resolution they run offline: no FPGA, no serial device, no private
paths, and no sibling checkouts — **except** `uart_host_smoke`, which opens a
named serial port when built with `--features uart` (maintainer Basys 3 evidence
only; not for default CI or casual `cargo run --example …`).

| Example | What it shows |
|---|---|
| `generic_mem_export` | Default **generic-dense-q88** path: 4×6 checked export, `write_generic`, negative weights, shape/non-finite/overflow errors |
| `dense_codec` | SiliconBridge v3 **example layout** and a host-only dense-8 frame (golden bytes, never opens a port) |
| `spikenaut_profile_export` | Opt-in synthetic 16-neuron bundle + signed readout under the historical `Spikenaut-v2` layout tag |
| `uart_host_smoke` | **Live UART** (`--features uart` only): one Basys 3 host-session exchange on an explicit port; see `scripts/smoke-hardware-uart.sh` |

```bash
cargo run --example generic_mem_export
cargo run --example dense_codec
cargo run --example spikenaut_profile_export
```

Do not pass `--features uart` to the offline examples above. Live UART is
limited to `uart_host_smoke` and the maintainer smoke script documented in the
README feature matrix.
The `ping()` method sends a real 16-channel stimulus and is not part of
these examples.
