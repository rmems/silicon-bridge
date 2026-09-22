# Requirements Document

## Introduction

`hardware-publish-gate` converts the externally-tracked GitHub #84 / Linear
RM-1679 deferral ("a silicon-bridge UART host session on the reference Basys 3
gates the 0.3.1 publish") into an **in-repo, actionable, recorded publish
gate**. Today the gate exists only as prose scattered across `README.md`,
`docs/consumer.md`, and `docs/release-readiness.md` that points at an external
issue. This feature makes the gate concrete by adding a fill-in evidence note, a
runnable `uart`-gated smoke harness that exercises the already-public
`FpgaBridge` API against a maintainer-selected serial port, a maintainer-facing
smoke script, and a formal numbered step in the release checklist that blocks
the authorized `cargo publish` until the note records a passing session plus an
explicit maintainer go-ahead.

This is a **documentation + tooling + release-gate** feature. It adds one
example binary, one shell script, one evidence note, and edits to existing docs.
It makes **no changes to `src/` library logic**: the example is a thin consumer
of the already-public `FpgaBridge` surface, and the script is modeled on the
existing `scripts/smoke-packaged-consumer.sh`. The design preserves the house
rule that the *ordinary* release checklist never flashes an FPGA; the new gate
is a separate, maintainer-run hardware step. Throughout, the crate's hedged
wording house style is honored: what is proved ("a live silicon-bridge UART host
session on one named board") is kept strictly distinct from what is not proved
(multi-board support, timing closure, crates.io / docs.rs registry state), and
the silicon-hdl LED-heartbeat smoke (#68) is kept distinct from a UART host
session everywhere.

The requirements below are derived from the approved design's six components and
eight correctness properties. Each requirement is written to be testable so the
design's correctness properties can reference specific acceptance criteria.

## Glossary

- **Evidence_Note**: The file `docs/hardware-smoke-note.md`, a fill-in template
  that becomes the single recorded proof of a silicon-bridge UART host session
  on the reference board.
- **Smoke_Harness**: The file `examples/uart_host_smoke.rs`, a `uart`-gated
  runnable consumer of the public `FpgaBridge` API that performs one host
  exchange and prints a PASS/FAIL line.
- **Smoke_Script**: The file `scripts/smoke-hardware-uart.sh`, the
  maintainer-facing wrapper that requires an explicit port and runs the
  Smoke_Harness under the `uart` feature.
- **Release_Checklist**: The file `docs/release-readiness.md`, the numbered
  release-readiness document.
- **Hardware_Gate**: The new numbered hardware-evidence section inserted into
  the Release_Checklist between the current §4 and §5, which blocks the
  authorized `cargo publish`.
- **Reference_Board**: The Digilent Basys 3 development board carrying a Xilinx
  Artix-7 `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`), running the silicon-hdl top
  `spikenaut_soc_basys3_top`.
- **UART_Host_Session**: A live exchange over a serial port between the
  Smoke_Harness (via `FpgaBridge`) and a flashed Reference_Board.
- **LED_Heartbeat_Smoke**: The separate silicon-hdl smoke tracked as #68; it is
  not a UART_Host_Session.
- **Explicit_Port**: A serial device path supplied only via the environment
  variable `SILICON_BRIDGE_PORT` or a command-line argument; never enumerated or
  guessed.
- **Publish_Gate_Condition**: The condition under which the Hardware_Gate is
  satisfied — the Evidence_Note records `PASS` for the current candidate AND a
  maintainer records explicit go-ahead.
- **Doc_Pointers**: The Reference-hardware evidence pointers added to `README.md`
  and `docs/consumer.md`.
- **Guidance_Pointers**: The brief cross-references added to `CHANGELOG.md`,
  `AGENTS.md`, `CLAUDE.md`, and `REVIEW.md`.
- **SPDX_Header**: The line `SPDX-License-Identifier: MIT OR Apache-2.0`,
  formatted per file type (markdown comment for `.md`, a `#` comment after the
  shebang for `.sh`, `//!` or `//` for `.rs`).

## Requirements

### Requirement 1: Evidence note structure and reference-hardware identity

**User Story:** As a maintainer, I want a fill-in evidence note that captures the reference hardware identity, so that a recorded UART host session is reproducible and unambiguous.

#### Acceptance Criteria

1. THE Evidence_Note SHALL record the Reference_Board identity as Digilent Basys 3, FPGA `XC7A35T-1CPG236C` (`xc7a35tcpg236-1`), and silicon-hdl top `spikenaut_soc_basys3_top`.
2. THE Evidence_Note SHALL provide a session-record section containing at least these labeled fields: silicon-bridge Candidate SHA (the commit to be tagged and published), silicon-hdl HEAD SHA, serial port, baud rate, exact commands run, a harness result line (the verbatim `PASS: ...` / `FAIL: ...` line), a result field whose value is one of `PASS` or `FAIL`, and a maintainer go-ahead field. Additional labeled fields are permitted; the list is a minimum, not an exhaustive set.
3. THE Evidence_Note SHALL describe the evidence flow as an ordered sequence of exactly four steps in this order: `write_generic` output, then silicon-hdl `$readmemh` banks, then a bitstream, then a `uart`-feature UART_Host_Session.
4. THE Evidence_Note SHALL contain a two-part structure in which both a "What this proves" part and a "does not prove" part are present and each contains at least one non-empty line of content.
5. THE Evidence_Note SHALL reference GitHub #84 as the tracking issue.
6. THE Evidence_Note SHALL contain the markdown-comment SPDX_Header as its first line, before any other content.
7. IF the session-record section is submitted with any of its labeled fields left blank or with the result field set to a value other than `PASS` or `FAIL`, THEN THE Evidence_Note SHALL be treated as incomplete for reproducibility purposes.

### Requirement 2: Reference-hardware documentation pointers

**User Story:** As a reader of the project docs, I want the Reference-hardware sections to point at the evidence note, so that I can find where host-session evidence is recorded without being told a proof exists prematurely.

#### Acceptance Criteria

1. THE README Reference-hardware section SHALL contain a reference to `docs/hardware-smoke-note.md` identifying it as the location where #84 host-session evidence is recorded.
2. THE `docs/consumer.md` Reference-hardware section SHALL contain an evidence pointer to `docs/hardware-smoke-note.md` that states whether a UART_Host_Session has been recorded.
3. WHILE the Evidence_Note has not recorded a `PASS` result, THE Doc_Pointers SHALL state that no host-session proof exists yet.
4. WHEN the Evidence_Note records a `PASS` result, THE Doc_Pointers SHALL state that a UART_Host_Session host-session proof has been recorded and reference the Evidence_Note as its location.
5. THE Doc_Pointers SHALL preserve the existing hedged wording present in `README.md` and `docs/consumer.md` regarding the board-agnostic default, the single named reference board, and #68 not being a UART_Host_Session, with no removal or alteration of those three statements.
6. IF the Evidence_Note referenced by a Doc_Pointer is absent or contains no recorded UART_Host_Session result, THEN THE Doc_Pointers SHALL state that no host-session proof exists yet and SHALL NOT assert that a proof exists.

### Requirement 3: Smoke harness explicit-port resolution

**User Story:** As a maintainer, I want the smoke harness to use only an explicit serial port, so that it never talks to an unintended device by auto-probing.

#### Acceptance Criteria

1. THE Smoke_Harness SHALL resolve the serial port only from a command-line argument or the environment variable `SILICON_BRIDGE_PORT`, and SHALL perform no serial-port enumeration or auto-probing.
2. WHERE a non-empty command-line argument is supplied AND `SILICON_BRIDGE_PORT` is set to a non-empty value, THE Smoke_Harness SHALL use the command-line argument.
3. IF no command-line argument is supplied AND `SILICON_BRIDGE_PORT` is unset or empty, THEN THE Smoke_Harness SHALL print guidance to standard error, return exit code 2, and construct no `FpgaBridge`.
4. THE Smoke_Harness SHALL contain no reference to `FpgaBridge::new`, `find_fpga_ports`, or `list_serial_ports`.
5. WHERE `SILICON_BRIDGE_BAUD` is unset, empty, or set to a value that does not parse as a positive integer, THE Smoke_Harness SHALL use the default baud rate of 115200.

### Requirement 4: Smoke harness exchange, recovery, and result reporting

**User Story:** As a maintainer, I want the harness to run one exchange over the public API and report a clear result, so that I can copy a single PASS/FAIL line into the evidence note.

#### Acceptance Criteria

1. WHEN a non-empty Explicit_Port is resolved, THE Smoke_Harness SHALL open the port through the public `FpgaBridge` API and perform exactly one `exchange` on the `silicon_bridge_v3` layout using a stimuli slice whose length equals the layout's `input_channels()` and whose values are all finite.
2. WHEN the `exchange` returns success, THE Smoke_Harness SHALL print exactly one line beginning with `PASS` to standard output and return exit code 0.
3. IF opening the port or the `exchange` fails, THEN THE Smoke_Harness SHALL print exactly one line beginning with `FAIL` to standard output, indicating the failing stage (open or exchange), and return exit code 1.
4. IF an `exchange` failure occurs AND `needs_recovery` reports true, THEN THE Smoke_Harness SHALL call `recover`, emit one status line to standard error indicating whether `recover` returned success or failure, and return exit code 1 regardless of the `recover` outcome.
5. THE Smoke_Harness SHALL write exactly one copy-pasteable result line beginning with either `PASS` or `FAIL` (never both) to standard output, and SHALL write all other status lines to standard error.
6. IF no non-empty Explicit_Port is resolved from the SILICON_BRIDGE_PORT environment variable or the command-line argument, THEN THE Smoke_Harness SHALL print one line beginning with `FAIL` to standard output indicating that no port was provided and return exit code 2 (the usage-error status, consistent with Requirement 3.3; no `FpgaBridge` is constructed in this case).
7. WHERE the Smoke_Harness is built without the `uart` feature, THE Smoke_Harness SHALL print to standard error a message stating that the `uart` feature must be enabled to run the harness and return exit code 2.
8. THE Smoke_Harness SHALL carry the SPDX_Header as a `//!` or `//` comment.

### Requirement 5: Smoke harness offline testability

**User Story:** As a developer, I want the harness exchange logic to be testable without hardware, so that its behavior is verifiable in default builds.

#### Acceptance Criteria

1. THE Smoke_Harness SHALL expose its exchange-and-report logic as a unit callable with a `FpgaBridge` constructed via `FpgaBridge::from_port` from a supplied mock serial port, such that no operating-system serial device is opened during the call.
2. WHEN the exchange-and-report logic is driven by a mock serial port scripted to return a single 36-byte v3 reply, THE Smoke_Harness SHALL complete the exchange and terminate with a success outcome (process exit status 0).
3. IF the exchange-and-report logic is driven by a mock serial port scripted to return a short read (fewer than 36 bytes) or end-of-file (0 bytes) for the reply, THEN THE Smoke_Harness SHALL terminate with a failure outcome (non-zero process exit status) accompanied by an error indication describing the read failure, and SHALL NOT report success.
4. IF the reply read fails with a short read or end-of-file, THEN THE Smoke_Harness SHALL invoke `FpgaBridge::recover` exactly once before terminating.

### Requirement 6: Smoke script behavior and disclaimer

**User Story:** As a maintainer, I want a wrapper script that requires an explicit port and prints a hedged disclaimer, so that I can run and correctly frame the hardware smoke.

#### Acceptance Criteria

1. THE Smoke_Script SHALL set `set -euo pipefail`, resolve its `ROOT` directory from its own location, and register a cleanup trap on `EXIT`.
2. IF `SILICON_BRIDGE_PORT` is unset or empty, THEN THE Smoke_Script SHALL print guidance naming the `SILICON_BRIDGE_PORT` variable to standard error and exit with a non-zero status.
3. WHERE `SILICON_BRIDGE_BAUD` is unset, THE Smoke_Script SHALL use the default baud rate of 115200.
4. WHEN `SILICON_BRIDGE_PORT` is set to a non-empty value, THE Smoke_Script SHALL run the Smoke_Harness via `cargo run --features uart --example uart_host_smoke` and forward the explicit port.
5. IF the Smoke_Harness run exits non-zero, THEN THE Smoke_Script SHALL propagate a non-zero exit status.
6. THE Smoke_Script SHALL print a final disclaimer containing three distinct statements: that the run proves a live UART_Host_Session on one board; that it is distinct from the LED_Heartbeat_Smoke (#68); and that it is not a crates.io / docs.rs publication proof.
7. THE Smoke_Script SHALL carry the SPDX_Header as a `#` comment on the line immediately after the shebang.

### Requirement 7: Formal hardware-evidence publish gate

**User Story:** As a maintainer, I want a formal numbered gate in the release checklist, so that the authorized publish is blocked until a passing host session is recorded.

#### Acceptance Criteria

1. THE Release_Checklist SHALL contain a new numbered Hardware_Gate section inserted between the current §4 and the current §5, such that the Hardware_Gate is numbered §5.
2. THE Hardware_Gate SHALL state that the authorized `cargo publish` is blocked until BOTH of the following are true: the Evidence_Note records a passing UART_Host_Session on the Reference_Board, AND a maintainer records explicit written go-ahead in the Evidence_Note.
3. IF the Evidence_Note does not record a passing UART_Host_Session on the Reference_Board, THEN THE Hardware_Gate SHALL block the authorized `cargo publish`.
4. IF a maintainer has not recorded explicit written go-ahead, THEN THE Hardware_Gate SHALL block the authorized `cargo publish`.
5. THE Hardware_Gate SHALL name `scripts/smoke-hardware-uart.sh` and `examples/uart_host_smoke.rs` as the two tools that produce the UART_Host_Session evidence recorded in the Evidence_Note.
6. WHEN the Hardware_Gate section is inserted, THE Release_Checklist SHALL renumber the sections previously numbered §5, §6, and §7 to §6, §7, and §8 respectively, such that all section numbers from §1 through §8 are present exactly once with no gaps or duplicates.
7. THE Release_Checklist §4 SHALL retain the rule that the ordinary checklist does not flash an FPGA.
8. THE Release_Checklist "Out of scope here" section SHALL update its Basys 3 entry to reference the Hardware_Gate section (§5) instead of referencing only the external issue.
9. THE Hardware_Gate SHALL state that a passing LED_Heartbeat_Smoke (#68) does not satisfy the Hardware_Gate.
10. IF the Evidence_Note is empty, THEN THE Hardware_Gate SHALL treat the gate as not satisfied and SHALL block the authorized `cargo publish`.

### Requirement 8: Publish-gate condition definition

**User Story:** As a maintainer, I want the gate condition defined precisely, so that an incomplete or failing note never counts as clearing the gate.

#### Acceptance Criteria

1. THE Publish_Gate_Condition SHALL be satisfied only when ALL of the following hold: the Evidence_Note records a `PASS` result, the recorded result is for the current publish candidate, AND a maintainer has recorded explicit go-ahead.
2. WHILE any required Evidence_Note field is at its template placeholder or empty, THE Publish_Gate_Condition SHALL remain unsatisfied.
3. IF the Evidence_Note records a `FAIL` result, THEN THE Publish_Gate_Condition SHALL remain unsatisfied.
4. IF the Evidence_Note records a `PASS` result whose candidate identifier does not match the current publish candidate, THEN THE Publish_Gate_Condition SHALL remain unsatisfied.
5. WHERE only a draft pull request or green CI exists without a recorded `PASS` and maintainer go-ahead, THE Publish_Gate_Condition SHALL remain unsatisfied.

### Requirement 9: Exclusion from default CI and from checklist §4

**User Story:** As a maintainer, I want the harness and script kept out of default CI and out of the ordinary checklist, so that automated runs never attempt to flash or reach hardware.

#### Acceptance Criteria

1. THE Smoke_Harness SHALL be gated behind the `uart` feature such that a build invoked without the `uart` feature enabled excludes all Smoke_Harness hardware-path code from compilation.
2. WHEN a default build is invoked without the `uart` feature enabled, THE Smoke_Harness SHALL produce no attempt to open, flash, or communicate over any hardware or serial interface.
3. THE `.github/workflows` files SHALL contain zero occurrences of the literal strings `uart_host_smoke` and `smoke-hardware-uart.sh`.
4. THE Release_Checklist §4 SHALL contain zero occurrences of the terms Smoke_Script and Smoke_Harness.
5. THE Smoke_Script and Smoke_Harness SHALL be referenced only within the Hardware_Gate section of the Release_Checklist.
6. IF a `.github/workflows` file or Release_Checklist §4 is found to reference the Smoke_Script or Smoke_Harness, THEN THE verification check SHALL fail and indicate which file and term violated the exclusion.

### Requirement 10: Announcements and agent/reviewer guidance

**User Story:** As an agent or reviewer, I want brief pointers to the release checklist, so that publish-related work routes to the gate without duplicating checklist content.

#### Acceptance Criteria

1. THE `CHANGELOG.md` `[Unreleased]` `### Added` section SHALL contain exactly one bullet that names all four terms: Evidence_Note, Smoke_Script, Smoke_Harness, and Hardware_Gate.
2. THE `CHANGELOG.md` bullet SHALL state that `cargo publish` of the current candidate is held until both a recorded Basys 3 UART_Host_Session (#84) and explicit maintainer go-ahead are present, using the same declarative phrasing style as the existing `[0.3.0]` entry.
3. THE `AGENTS.md` file SHALL contain a note that references `docs/release-readiness.md` and states that `cargo publish` requires both recorded hardware smoke evidence (`docs/hardware-smoke-note.md`, #84) and explicit maintainer authorization.
4. THE `CLAUDE.md` file SHALL contain a note that references `docs/release-readiness.md` and states that `cargo publish` requires both recorded hardware smoke evidence (`docs/hardware-smoke-note.md`, #84) and explicit maintainer authorization.
5. THE `REVIEW.md` file SHALL contain a pointer instructing reviewers to confirm the Hardware_Gate before approving any publish-related change.
6. WHERE a Guidance_Pointer references `docs/release-readiness.md`, THE Guidance_Pointer SHALL link or cross-reference that document rather than restating any checklist item found in it.

### Requirement 11: No premature proof claim across documents

**User Story:** As a reader, I want no document to claim the hardware proof exists before it is recorded, so that project claims stay honest.

#### Acceptance Criteria

1. WHILE the Evidence_Note's `Result` field is not exactly equal to `PASS`, THE README, `docs/consumer.md`, and Evidence_Note SHALL each display, in every location that references the UART_Host_Session proof status, the literal text "not yet recorded" or a conditional statement that explicitly indicates the proof has not been recorded.
2. WHILE the Evidence_Note is an unfilled template (its `Result` field is empty, absent, or contains only placeholder text), THE README, `docs/consumer.md`, and Evidence_Note SHALL NOT contain any statement asserting that the UART_Host_Session proof exists, has passed, or has been recorded.
3. IF the Evidence_Note's `Result` field contains any value other than `PASS` or a recognized not-yet-recorded indicator (empty, absent, or placeholder text), THEN THE README, `docs/consumer.md`, and Evidence_Note SHALL treat the proof status as not recorded and display the "not yet recorded" indicator described in criterion 1.

### Requirement 12: LED-heartbeat smoke kept distinct from a UART host session

**User Story:** As a reader, I want #68 kept distinct from a UART host session everywhere it appears, so that the two forms of evidence are never conflated.

#### Acceptance Criteria

1. WHERE any new artifact (docs/hardware-smoke-note.md, scripts/smoke-hardware-uart.sh, the release-readiness Hardware_Gate section, or the CHANGELOG bullet) mentions #68, THE artifact SHALL state that #68 is the LED_Heartbeat_Smoke and is not a silicon-bridge UART_Host_Session.
2. IF a new artifact references #68 without stating that it is the LED_Heartbeat_Smoke and not a UART_Host_Session, THEN THE distinctness verification check SHALL fail and indicate the offending artifact.
