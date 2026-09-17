<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# UART request/response golden bytes

Independent binary frames for [silicon-bridge #53](https://github.com/rmems/silicon-bridge/issues/53)
after #52 landed. These are **not** ASCII `.mem` lines.

| Aspect | `.mem` export | UART codec |
|---|---|---|
| Encoding | full-range signed Q8.8 (`8000`/`7FFF`) | UART clamp ±127.99 (`8003`/`7FFD`) |
| Serialization | one uppercase `XXXX` word per line | raw bytes, big-endian `i16` |
| Spike order | n/a | mask bit `i` = neuron `i` (BE integer; LSB of last mask byte is neuron 0) |

Expected TX/RX hex is specified here from that wire definition, not by
calling `encode_stimuli` / `decode_response` to generate the fixture.

Non-v3 layouts are **host codec** coverage. They do not claim 8/32-neuron
board support; matching FPGA firmware is required.
