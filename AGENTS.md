# AGENTS.md

> **Priority order**: Constraints > Code style > PR instructions > Testing > Dev environment.
> Items marked mandatory must never be violated. Conventions should be followed unless there's a good reason not to. Workflows are recommended.

Instructions for AI coding agents working on this repository.

## Identity

You are a Rust-focused coding agent. Write idiomatic Rust. Follow the conventions below for every change.

## Constraints (mandatory)

- Do not commit secrets, API keys, DSNs, or credentials
- Do not add `unsafe` code without explicit safety justification
- Do not downgrade Rust edition from 2024 to 2021
- Do not add unused dependencies — check if a crate is actually imported before adding it
- Do not use relative links to license files in doc comments (they break on docs.rs)
- UART (Universal Asynchronous Receiver-Transmitter) feature is properly gated behind `#[cfg(feature = "uart")]`

## Code style (conventions)

- SPDX (Software Package Data Exchange) license header on every `.rs` file
- Module-level doc comments with `//!`
- Public items get `///` doc comments
- Use `serde` derive for serializable types
- Use `f32` for public API parameters (consistent with existing API)
- Conventional Commits for messages: `type(scope): description`

  - Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`
  - Scopes: `fpga-export`, `fpga-metrics`, `fpga-bridge`, `ci`, `docs`

## PR instructions (conventions)

- Branch naming: `feat/`, `fix/`, `chore/`, `docs/` prefix
- Run `cargo test` and `cargo check` before pushing
- One issue per PR — split multi-issue work into separate PRs
- Link PR to the issue it addresses

## Tools

- `cargo check` — compile check
- `cargo test` — run unit tests and doctests
- `cargo build --release` — optimized build
- `cargo build --features uart` — build with UART bridge (requires `serialport`)
- `cargo clippy` — lint

## Project overview

`silicon-bridge` is a Rust project for SNN (Spiking Neural Network)-to-FPGA (Field-Programmable Gate Array) deployment.

- `fpga_export` exports trained SNN parameters as Q8.8 fixed-point `.mem` files for Vivado synthesis.
- `fpga_metrics` parses Vivado timing reports for CI/CD gating.
- `fpga_bridge` provides a UART bridge for runtime spike exchange (`uart` feature, `serialport`).

- **License**: dual MIT (Massachusetts Institute of Technology) / Apache-2.0
- **Rust edition**: 2024
- **Crate type**: library (`silicon_bridge`)

## Dev environment tips (recommended)

```bash
cargo check                 # quick compile check
cargo test                  # run all tests + doctests
cargo build --release       # optimized build
cargo build --features uart # enable UART bridge (requires serialport)
```

Feature flags:

- `uart` — enables `FpgaBridge` and `find_fpga_ports` (requires `serialport` crate)

## Testing instructions (recommended)

Tests live inline in source files, plus the #53 golden-contract suite:

| Module | Tests | What's covered |
|--------|-------|----------------|
| `src/fpga_export.rs` | unit tests | Q8.8 conversion, checked export, `.mem` writer, signed encoding |
| `src/export_profile.rs` | unit tests | generic / legacy / signed-output profiles, determinism, filename and overwrite rejection |
| `src/fpga_codec.rs` | unit tests (default features) | v3 golden frames, checked vs legacy encode, 8/16/32 and non-multiple-of-8 masks |
| `src/fpga_bridge.rs` | unit tests (`uart` feature) | port-name heuristic, `SerialConfig` validation/defaults, open/enumerate/configure failures, USB-serial selection, mock-port injection, scripted mock transport errors / no-resend |
| `src/lib.rs` | doctest | Quick Start example |
| `tests/golden_export_contract.rs` | integration | Independent Rust↔HDL numeric and memory-layout fixtures (#53) |
| `tests/golden_uart_contract.rs` | integration | Independent UART v3 / dense-profile golden bytes (#53 after #52) |

`tests/golden/` is the committed expected output (not encoder-generated).
Ordinary `cargo test` does not run HDL simulation. Optional Icarus evidence:
`bash tests/golden/hdl/run.sh`.

Run `cargo test` and ensure all tests pass before pushing. Add tests for any new code you write.
