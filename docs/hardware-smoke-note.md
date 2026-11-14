<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Basys 3 UART host-session smoke note

This note records a live **silicon-bridge UART host session** on one named
reference board. It is the recorded evidence for the 0.4.0 publish gate tracked
in GitHub [#84](https://github.com/rmems/silicon-bridge/issues/84).

**Status: recorded PASS for candidate a8570b1 (2026-11-14).** A silicon-bridge
UART host session was run and recorded below and returned `PASS` at post-#141
`main` HEAD `a8570b1` (re-verified after the cargo-deny gate restore merged;
earlier PASSes at `97cc3da` and `996d8f3` had identical `src/` and
`Cargo.lock`). Release-readiness §5 items 1–3 are satisfied by this record;
the gate stays open until item 4 (maintainer go-ahead) and the authorized
publish are recorded.

When a session is recorded, the maintainer MUST update this Status line in the
same edit that fills the Session record (for example, to `**Status: recorded
PASS for candidate <SHA>.**`) and update the "Host-session evidence" row in
[`docs/consumer.md`](consumer.md) so no document contradicts the recorded
result. The release gate (release-readiness §5) treats the note as satisfied
only when this Status line, every Session-record field, and the maintainer
go-ahead all agree on a `PASS` for the current candidate.

## Reference hardware

| Field | Value |
|---|---|
| Board | Digilent Basys 3 |
| FPGA | Xilinx Artix-7 `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`) |
| silicon-hdl top | `spikenaut_soc_basys3_top` |

## Session record

Fill in every field when a session is run. The **Result** field records only
the status token — `PASS` or `FAIL` — while the harness's full result line
(e.g. `PASS: live silicon-bridge UART host session port=... baud=... (...)`)
goes verbatim in the **Harness result line** field. A blank field, or a Result
that is not exactly `PASS` or `FAIL`, means the note is incomplete for
reproducibility purposes.

The **Candidate SHA** is the silicon-bridge commit that will be tagged and
published — i.e. the commit the session was run against, recorded *before* the
edit that commits this note. It is not this note's own commit: committing the
filled-in note necessarily produces a later `HEAD`, so recording "current
`HEAD`" here would name a commit that no longer exists at publish time. The
release gate matches this recorded Candidate SHA against the publish candidate,
not against the note's commit.

| Field | Value |
|---|---|
| silicon-bridge Candidate SHA | `a8570b107092cb267cc966068050a97bf141cb4e` |
| silicon-hdl HEAD SHA | `e56663143fe5faaa9b6dddea69ffda66d0ad6602` |
| `Cargo.lock` SHA-256 | `0c923d8113854e183586f94f51acad3aba06d11cc288cb1319dbc3c530beb585` |
| Serial port | `COM3` (Windows host, "USB Serial Port (COM3)") |
| Baud rate | `115200` |
| Exact commands run | `SILICON_BRIDGE_PORT=COM3 bash scripts/smoke-hardware-uart.sh` (run twice at `996d8f3`, once at post-merge `97cc3da`, and once at post-#141 `a8570b1`, PASS every time). Board side: bitstream rebuilt from silicon-hdl `main` @ `e566631` via `vivado -mode batch -source scripts/build_soc.tcl` (Vivado 2026.1, WNS 0.669 ns / WHS 0.028 ns, DRC 0 errors) → `vivado_projects/spikenaut_soc/output/spikenaut_soc.bit`, programmed over JTAG (`program_hw_devices`, DONE HIGH), BTNC reset, SW15 + SW14 high |
| Harness result line | `PASS: live silicon-bridge UART host session port=COM3 baud=115200 (16 potentials, 16 spikes)` |
| Result | PASS |
| Maintainer go-ahead | _held — PASS evidence recorded 2026-11-14; explicit publish go-ahead pending_ |

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

## What a passing session proves

When Result is `PASS`, the recorded session demonstrates a live silicon-bridge
UART host session on the one named Basys 3 board: the host opened the port, sent
one SiliconBridge v3.0 stimulus frame, and decoded one v3.0 response frame
(16 potentials, 16 spikes). The earlier 2026-11-09 session against candidate
`1886113` timed out reading the reply because the board was not running the
matching `e566631` bitstream; re-building and re-programming that bitstream
produced this PASS.

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
