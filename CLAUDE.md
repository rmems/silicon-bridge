<!-- Last updated: 2026-07-22 -->
# CLAUDE.md

@AGENTS.md

Companion for Claude Code and similar agents. Constraints and conventions live
in [AGENTS.md](AGENTS.md); quality bar in [REVIEW.md](REVIEW.md).

## Identity

Rust agent for **silicon-bridge**: Q8.8 fixed-point parameter export, optional
UART (Universal Asynchronous Receiver-Transmitter) spike bridge (`uart` feature /
`serialport`), and Vivado timing-report parsing. Prefer a trait-stable public
API, feature-gate hardware I/O, and keep Rust edition **2024**.

## Tools

| Tool / command | Purpose |
|---|---|
| `cargo check` | Fast compile check |
| `cargo test` | Unit tests and doctests |
| `cargo fmt --check` | Formatting gate |
| `cargo clippy --all-targets -- -D warnings` | Lint gate (default features) |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint gate with UART (needs `libudev` on Linux) |
| `cargo check --features uart` | UART feature compile (needs `serialport` / `libudev` on Linux) |
| `cargo fuzz build --dev --sanitizer none` | Compile-only fuzz targets (opt-in campaign via `cargo fuzz run`) |
| `cargo build --release` | Optimized library build |

## Do not

- Downgrade the Rust edition from 2024 to 2021
- Commit secrets, API keys, DSNs, or credentials
- Commit IDE project trees (for example `.idea/`, `.vscode/`)
- Add `unsafe` without an explicit safety justification
- Reimplement NIR (Neuromorphic Intermediate Representation) HDF5 I/O in this
  crate (deferred; future work may depend on `Limen-Neural/nir-rs` instead of a
  local parser)

## Local quality bar

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo clippy --all-targets --all-features -- -D warnings   # needs libudev on Linux
cargo test --all-features                              # needs libudev on Linux
cargo check --features uart   # needs serialport / libudev on Linux
```

UART is gated behind `#[cfg(feature = "uart")]`. Prefer `cargo test` without
`--all-features` on runners that lack `libudev-dev` unless the workflow installs it.
