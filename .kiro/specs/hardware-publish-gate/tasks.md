<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Implementation Plan: hardware-publish-gate

## Overview

This plan implements the in-repo publish gate for `silicon-bridge` as a
documentation + tooling + release-gate feature. It makes **no changes to
`src/` library logic** — the harness is a thin `uart`-gated consumer of the
already-public `FpgaBridge` API, and the script is modeled on the existing
`scripts/smoke-packaged-consumer.sh`.

The tasks are organized around the three CodeRabbit tasks from the design:

- **Task 1 — Evidence & docs:** the fill-in evidence note plus the
  Reference-hardware evidence pointers in `docs/consumer.md` and `README.md`.
- **Task 2 — Runnable harness:** the `uart`-gated example (with offline unit
  tests) plus the maintainer-facing smoke script.
- **Task 3 — Formal gate, changelog, guidance:** the release-readiness
  hardware-evidence gate (new §5 + renumbering), the changelog announcement,
  and the agent/reviewer guidance pointers.

Ordering rule: the note, harness, and script are created first so the
release-readiness, changelog, and guidance edits that reference them point at
existing artifacts. The plan ends with a single verification task covering the
required checks and the static grep checks for the design's correctness
properties. The one truly hardware-dependent step (running against a wired
Basys 3) is out of scope for this coded workflow; the harness and script are
designed to be verified offline (no real board needed for the coded tests).

## Tasks

- [x] 1. Task 1 — Evidence note and reference-hardware pointers
  - [x] 1.1 Create the evidence note `docs/hardware-smoke-note.md`
    - Create the fill-in template per design Component 1 / LLD (implicit in Component 1 structure).
    - Put the markdown-comment SPDX header (`<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->`) as the literal first line, before any other content.
    - Add the one-paragraph scope statement: a live silicon-bridge UART host session on ONE named board; tracked under #84; state **not yet recorded**.
    - Add the `## Reference hardware` table with Digilent Basys 3, FPGA `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`), and silicon-hdl top `spikenaut_soc_basys3_top`.
    - Add the `## Session record` section with exactly these labeled fields: silicon-bridge HEAD SHA, silicon-hdl HEAD SHA, serial port, baud rate, exact commands run, and a result field valued `PASS`/`FAIL`.
    - Add the `## Flow (in order)` section listing exactly four ordered steps: `write_generic` output → silicon-hdl `$readmemh` banks → bitstream → `uart`-feature UART host session.
    - Add the two-part `## What this proves` / "This does not prove" structure, each with at least one non-empty line; in the "does not prove" list state that the LED-heartbeat smoke (#68) is NOT a UART host session.
    - Add the `## Tracking` section referencing GitHub #84 and noting raw logs may be linked there.
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 11.1, 11.2, 12.1_

  - [x] 1.2 Add the evidence pointer to `docs/consumer.md` Reference-hardware section
    - Add an evidence line/row pointing to `docs/hardware-smoke-note.md` that states whether a UART host session has been recorded, using **Not yet recorded** (#84) while the note is unfilled.
    - Preserve the existing hedged wording (board-agnostic default; single named reference board, not multi-board; #68 not being a UART host session) with no removal or alteration.
    - Do not alter the existing sentence that says the guide does not flash a board and cites #84.
    - _Requirements: 2.2, 2.3, 2.5, 2.6, 11.1, 11.2_

  - [x] 1.3 Add the evidence pointer to `README.md` Reference-hardware section
    - Add a sentence pointing to `docs/hardware-smoke-note.md` as the recording location for #84 host-session evidence, stating that until it is filled in no such host-session proof exists (raw logs may also be linked on #84).
    - Keep the existing paragraph and its hedges intact; do not claim a proof exists.
    - _Requirements: 2.1, 2.3, 2.5, 2.6, 11.1, 11.2_

- [x] 2. Task 2 — Runnable smoke harness and maintainer script
  - [x] 2.1 Create the smoke harness `examples/uart_host_smoke.rs`
    - Add the SPDX header as a `//!` doc-comment line plus a short usage doc block (design LLD 1).
    - Add the `#[cfg(not(feature = "uart"))] fn main()` no-op that prints how to enable the feature to stderr and exits `2`, so `cargo build --examples` under default features stays green.
    - Add the `#[cfg(feature = "uart")] fn main()` that calls `std::process::exit(run())`.
    - In `run()`: resolve the port from an explicit source only — CLI arg (`args().nth(1)`) over env `SILICON_BRIDGE_PORT`; if neither is a non-empty value, print guidance to stderr and return `2`, constructing no `FpgaBridge`.
    - Parse `SILICON_BRIDGE_BAUD` as a positive integer or fall back to default `115200` (parse-or-default).
    - Open the port via `FpgaBridge::builder(&port).baud_rate(baud).open()`; on error print a `FAIL` line to stdout indicating the open stage and return `1`.
    - Run exactly one `exchange` on `DenseQ88Layout::silicon_bridge_v3()` with an all-finite stimuli slice whose length equals the layout's input channel count (16).
    - Route all scoped status lines to stderr (`eprintln!`) and write exactly one copy-pasteable result line beginning with `PASS` or `FAIL` (never both) to stdout (`println!`); return `0` on PASS, `1` on failure.
    - Factor the exchange-and-report logic into `report_exchange(&mut FpgaBridge, &DenseQ88Layout, &[f32]) -> Result<(), String>` so it can be driven offline via `FpgaBridge::from_port`.
    - In `report_exchange`, on exchange failure: if `needs_recovery()` is true, call `recover()` and emit a stderr status line indicating whether recover returned success or failure; return `Err(..)` regardless of the recover outcome.
    - Do NOT call `FpgaBridge::new`, `find_fpga_ports`, or `list_serial_ports` anywhere.
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 4.8, 9.1, 9.2_

  - [x]* 2.2 Write offline unit tests for the harness exchange logic
    - **Property 7: Offline testability**
    - **Validates: Requirements 5.1, 5.2, 5.3, 5.4**
    - Add a `#[cfg(all(test, feature = "uart"))] mod tests` block that drives `report_exchange` through a `FpgaBridge` built via `FpgaBridge::from_port` with a `MockPort`-style `Box<dyn SerialPort>`, opening no real OS serial device.
    - Success case: mock scripted to return a single 36-byte v3 reply → `report_exchange` returns `Ok(())` (success outcome).
    - Failure case: mock scripted to return a short read (< 36 bytes) or EOF (0 bytes) → `report_exchange` returns `Err(..)` with a read-failure indication and does not report success.
    - Assert that on the short-read/EOF failure path `recover()` is invoked exactly once before terminating.

  - [x] 2.3 Create the smoke script `scripts/smoke-hardware-uart.sh`
    - Add `#!/usr/bin/env bash` as line 1 and `# SPDX-License-Identifier: MIT OR Apache-2.0` as line 2 (immediately after the shebang), then `set -euo pipefail` (design LLD 2).
    - Resolve `ROOT` via `cd "$(dirname "$0")/.." && pwd` and `cd "$ROOT"`; create a `mktemp -d` `WORK` dir and register `trap cleanup EXIT` to `rm -rf` it.
    - Require `SILICON_BRIDGE_PORT`: if unset or empty, print guidance naming the variable to stderr and exit non-zero (no auto-probe).
    - Support an optional `SILICON_BRIDGE_BAUD` override defaulting to `115200`.
    - Run `cargo run --features uart --example uart_host_smoke -- "$SILICON_BRIDGE_PORT"`, forwarding the explicit port and baud, and `tee` to the temp file; propagate a non-zero exit via `set -o pipefail`.
    - Print scoped `==>` status lines and a final three-part disclaimer: proves a live UART host session on ONE board; distinct from the silicon-hdl LED-heartbeat smoke (#68); NOT a crates.io / docs.rs publication proof.
    - Do not reference this script from release-readiness §4 and do not wire it into any `.github/workflows/*` job.
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 6.7, 12.1_

- [x] 3. Task 3 — Formal publish gate, changelog, and guidance pointers
  - [x] 3.1 Insert the hardware-evidence gate into `docs/release-readiness.md`
    - Insert a NEW `## 5. Hardware-evidence gate (Basys 3 UART host session)` section between the current §4 and current §5, verbatim per design LLD 3.
    - State that the authorized `cargo publish` is blocked until BOTH the Evidence_Note records a passing UART host session on the reference Basys 3 AND a maintainer records explicit written go-ahead.
    - Name `scripts/smoke-hardware-uart.sh` and `examples/uart_host_smoke.rs` as the two tools that produce the evidence (only in this new §5).
    - Renumber the current §5/§6/§7 headings to §6/§7/§8 so numbering is contiguous §1–§8 with no gaps or duplicates; update any section-number cross-reference in the renumbered "After an authorized `cargo publish`" section.
    - Keep the §4 rule that the ordinary checklist does not flash an FPGA, and ensure §4 does not name the script/example.
    - In the renumbered §8 "Out of scope here", repoint the Basys 3 entry to the new §5 gate rather than only the external issue.
    - State that a passing LED-heartbeat smoke (#68) does not satisfy the gate, that an empty note does not satisfy it, and that this step is not part of default CI.
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7, 7.8, 7.9, 7.10, 8.1, 8.2, 8.3, 8.4, 8.5, 9.4, 9.5, 12.1_

  - [x] 3.2 Add the changelog announcement to `CHANGELOG.md`
    - Add exactly one bullet under `[Unreleased]` `### Added` that names all four artifacts: the evidence note (`docs/hardware-smoke-note.md`), the script (`scripts/smoke-hardware-uart.sh`), the example (`examples/uart_host_smoke.rs`), and the new release-readiness hardware-evidence gate.
    - Restate that `cargo publish` of the current 0.3.1 candidate is held until a recorded Basys 3 UART host session (#84) plus explicit maintainer go-ahead, matching the `[0.3.0]` entry's declarative wording style.
    - Note the harness reads an explicit port only (no auto-probe), stays out of default CI, and is distinct from the silicon-hdl LED-heartbeat smoke (#68).
    - Preserve the blank line after the `### Added` heading per the REVIEW.md markdownlint note.
    - _Requirements: 10.1, 10.2, 12.1_

  - [x] 3.3 Add publish-note pointers to `AGENTS.md` and `CLAUDE.md`
    - Add a brief publish note (subsection or single line) to each file pointing to `docs/release-readiness.md` and stating that `cargo publish` requires recorded hardware smoke evidence (`docs/hardware-smoke-note.md`, #84) plus explicit maintainer authorization, and that agents must not run `cargo publish`.
    - Keep it brief; cross-reference the checklist and do not duplicate checklist content.
    - _Requirements: 10.3, 10.4, 10.6_

  - [x] 3.4 Add the reviewer pointer to `REVIEW.md`
    - Add a pointer instructing reviewers to confirm the hardware-evidence gate (release-readiness §5) is satisfied before approving any publish-related change: a passing Basys 3 UART host session recorded in `docs/hardware-smoke-note.md` (#84) plus explicit maintainer go-ahead, noting green CI or a filled version string is not that proof.
    - Cross-reference `docs/release-readiness.md` rather than restating checklist items.
    - _Requirements: 10.5, 10.6_

- [x] 4. Verification — run required checks and static property checks
  - Run `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` and fix any regressions caused by the new example/tests.
  - Run `cargo test` and `cargo build --examples` under default features to confirm the `#[cfg(not(feature = "uart"))]` no-op `main` compiles and no hardware path is present.
  - Run `cargo test --features uart` to exercise the offline unit tests, and `cargo run --features uart --example uart_host_smoke` with no port/env to confirm the guidance + exit-2 path.
  - Static grep check (Property 1): confirm `examples/uart_host_smoke.rs` contains no `FpgaBridge::new`, `find_fpga_ports`, or `list_serial_ports`.
  - Static grep check (Property 2): confirm no `.github/workflows/*` file references `smoke-hardware-uart.sh` or runs the Smoke_Script / live Smoke_Harness; `uart_host_smoke` may appear only in an offline `cargo test --examples` step. Confirm release-readiness §4 does not name the script/example (they appear only in §5).
  - Static grep check (Property 6): confirm SPDX headers are present on the three new files (markdown comment first line for the note; `#` line 2 after the shebang for the script; `//!`/`//` for the example).
  - Note: the wired-Basys-3 run is a maintainer-only hardware step and is intentionally NOT part of this task; the harness and script are verified offline here.
  - _Requirements: 3.4, 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 5.1, 5.2, 5.3, 5.4, 9.1, 9.2, 9.3, 9.4, 9.5_

## Notes

- Tasks marked with `*` are optional and can be skipped for a faster MVP; per the workflow, optional sub-tasks are not auto-implemented.
- Each task references specific granular requirement clauses (X.Y) for traceability.
- The design has a Correctness Properties section, so the harness's offline behavior is covered by a property-style unit-test task (2.2) placed next to the implementation (2.1) to catch errors early. The remaining correctness properties (explicit-port-only, out-of-CI/§4, no premature proof claim, hedged wording, #68 distinctness, SPDX headers) are enforced by document structure and the static grep checks in the verification task (4).
- No `src/` library logic changes and no new crate dependencies are introduced.
- The single hardware-dependent step (running against a physical Basys 3) is out of scope for this coded workflow; only the offline-verifiable parts are implemented and checked here.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "2.1", "2.3"] },
    { "id": 1, "tasks": ["1.2", "1.3", "2.2", "3.1", "3.3", "3.4"] },
    { "id": 2, "tasks": ["3.2"] },
    { "id": 3, "tasks": ["4"] }
  ]
}
```
