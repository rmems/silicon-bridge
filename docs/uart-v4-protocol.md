<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# SiliconBridge dense-Q8.8 UART v4 host contract

UART v4 adds bounded framing, request correlation, and an integrity check to
the existing dense signed-Q8.8 payload. This document specifies the host codec
only. **V4 is not compatible with the current Basys 3 SiliconBridge v3.0
firmware.** Selecting it on hardware requires a separately pinned matching
`silicon-hdl` implementation; this crate does not claim board parity.

## Frame

Requests and replies use the same envelope. Multi-byte integers and Q8.8 words
are big-endian.

| Offset | Size | Field | Contract |
|---:|---:|---|---|
| 0 | 1 | sync | `0xAA` |
| 1 | 1 | version | `0x04` |
| 2 | 2 | payload length | unsigned byte count of payload only |
| 4 | 4 | request id | caller-selected unsigned request/tick id |
| 8 | length | payload | request or reply payload below |
| 8 + length | 2 | checksum | CRC-16/CCITT-FALSE |

The checksum covers `version`, payload length, request id, and payload (offset
1 through the final payload byte). It excludes sync and the checksum itself.
CRC parameters are polynomial `0x1021`, initial value `0xFFFF`, no input or
output reflection, and final XOR `0x0000`. The check value for ASCII
`123456789` is `0x29B1`.

A request payload is exactly `input_channels` signed Q8.8 words, with the
existing UART ±127.99 clamp. A reply payload is exactly `output_neurons`
signed Q8.8 potentials, followed by `ceil(output_neurons / 8)` spike-mask
bytes and, when selected by the layout, a big-endian 16-bit switch field.
Spike bit `i` denotes neuron `i`; neuron 0 is the least-significant bit of the
last mask byte.

`DenseQ88Layout` bounds both dimensions to 256. The decoder derives the one
valid payload and total frame length from that checked layout, compares the
advertised length before decoding or allocating response vectors, and rejects
both truncation and trailing bytes.

## Request-id and rejection semantics

The caller supplies an expected request id to `decode_response_v4`. A reply
with any other id is rejected, as are wrong sync/version/length and CRC
mismatch. The id is correlation and freshness metadata. It can identify a
leftover reply when ids are not reused, but neither it nor CRC provides
cryptographic authentication, replay security, or protection from a malicious
peer.

The pure codec performs no UART I/O and never automatically resends a request.
Runtime transport selection and resynchronizing a byte stream are outside this
host-only contract.
