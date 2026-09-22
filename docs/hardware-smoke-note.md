<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Basys 3 UART host-session smoke note

This note records a live **silicon-bridge UART host session** on one named
reference board. It is the recorded evidence for the 0.3.1 publish gate tracked
in GitHub [#84](https://github.com/rmems/silicon-bridge/issues/84).

**Status: not yet recorded.** No silicon-bridge UART host session has been run
and recorded here. Until the Session record below is filled in with a `PASS`
result, no such host-session proof exists.

## Reference hardware

| Field | Value |
|---|---|
| Board | Digilent Basys 3 |
| FPGA | Xilinx Artix-7 `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`) |
| silicon-hdl top | `spikenaut_soc_basys3_top` |

## Session record

Fill in every field when a session is run. Copy the harness `PASS`/`FAIL` line
verbatim into the Result row. A blank field, or a Result other than `PASS` or
`FAIL`, means the note is incomplete for reproducibility purposes.

| Field | Value |
|---|---|
| silicon-bridge HEAD SHA | _<fill in: `git -C silicon-bridge rev-parse HEAD`>_ |
| silicon-hdl HEAD SHA | _<fill in: `git -C silicon-hdl rev-parse HEAD`>_ |
| Serial port | _<fill in: e.g. `/dev/ttyUSB0`>_ |
| Baud rate | _<fill in: e.g. `115200`>_ |
| Exact commands run | _<fill in: copied verbatim>_ |
| Result | _<fill in: `PASS` or `FAIL`>_ |
| Maintainer go-ahead | _<fill in: name + date of explicit publish go-ahead>_ |

## Flow (in order)

The evidence chain runs in this order. Only the final host-session step is
exercised from this repository; the steps to its left are performed by a
maintainer with silicon-hdl and Vivado.

1. `write_generic` (or the agreed export profile) produces silicon-hdl
   `$readmemh` banks (`parameters*.mem`).
2. The banks are built into a bitstream (Vivado, `spikenaut_soc_basys3_top`).
3. The bitstream is flashed onto the Basys 3 board.
4. The flashed board is exercised by a `uart`-feature silicon-bridge UART host
   session on an explicit serial port
   (`examples/uart_host_smoke.rs` via `scripts/smoke-hardware-uart.sh`).

## What this proves

- A live silicon-bridge UART host session on the one named Basys 3 board: the
  host opened the port, sent one SiliconBridge v3.0 stimulus frame, and decoded
  one v3.0 response frame.

## This does not prove

- multi-board support (this is one named reference, not an "any board" claim);
- timing closure or any FPGA implementation quality metric;
- crates.io / docs.rs registry state (publication is a separate authorized step);
- that the silicon-hdl LED-heartbeat smoke
  ([#68](https://github.com/rmems/silicon-hdl/issues/68)) is a UART host
  session. #68 is the LED-heartbeat smoke and is **not** a silicon-bridge UART
  host session.

## Tracking

GitHub [#84](https://github.com/rmems/silicon-bridge/issues/84). Raw session
logs (full serial transcript, Vivado provenance) may also be linked on that
issue.
