# Contributing to silicon-bridge

Contributor entry point for this repository. Conventions for commits, API
style, and what not to change live in [AGENTS.md](AGENTS.md). The same local
quality bar is listed in [CLAUDE.md](CLAUDE.md).

## License

Dual-licensed under the [MIT](LICENSE-MIT) license or the
[Apache-2.0](LICENSE-APACHE) license, at your option. The `license` field in
`Cargo.toml` is `MIT OR Apache-2.0`.

## Toolchain

- Edition **2024** (`edition = "2024"`). Do not downgrade the edition.
- `[package].rust-version` is **1.99.0**. That is the MSRV. CI enforces it
  with the `msrv (1.99.0, ubuntu-latest)` job. Edition 2024 and `if`/`let`
  chains need only Rust 1.88.0, and the dependency graph would compile on
  1.85.0 without those language features. The advertised floor is still
  1.99.0. Raising it requires the compatibility decision in
  [docs/release-readiness.md](docs/release-readiness.md).
- CI also runs the moving GitHub Actions `stable` toolchain. That lane is
  not a second MSRV pin.
- The optional `nir` feature pins `nir-rs = 0.4.5`. `nir-rs` declares a
  higher MSRV than this crate, so the 1.99.0 job does not pass
  `--all-features` or `--features nir`. Validate `nir` on stable.

## `uart` system packages

The `uart` feature depends on `serialport`. On Linux, building it needs
`pkg-config` and `libudev-dev`. `--all-features` also enables `nir-io`, which
links native HDF5 and needs `libhdf5-dev` on Linux. [`.agents/setup`](.agents/setup)
installs these (with `build-essential`). macOS and Windows do not need `libudev`.

Enumeration (`list_serial_ports`) needs `libudev` on Linux. Opening a
caller-selected path does not. Default-feature builds do not link
`serialport`.

CI installs `libudev-dev` before the `uart` feature steps in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) (`msrv`, `uart`) and
before coverage in [`.github/workflows/quality.yml`](.github/workflows/quality.yml).

## Local checks

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo clippy --all-targets --all-features -- -D warnings   # needs libudev-dev, libhdf5-dev on Linux
cargo test --all-features                                  # needs libudev-dev, libhdf5-dev on Linux
cargo test --all-features --examples                       # example #[test]s; uart job
cargo check --features uart   # needs serialport / libudev on Linux
```

The `fmt` CI job also checks the fuzz manifest:

```bash
cargo fmt --check --manifest-path fuzz/Cargo.toml
```

CI lints with `cargo clippy --all-targets` (and `--all-features` on the
`uart` job). The root `Cargo.toml` has no `[workspace]` table, so
`cargo clippy --workspace` covers this package only and is not its own CI
job. `fuzz/Cargo.toml` is a separate workspace; CI formats that manifest
and compiles the targets with `cargo fuzz build`.

Ordinary `cargo test` does not run HDL simulation. Optional Icarus evidence:
`bash tests/golden/hdl/run.sh`.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on every pull
request and on pushes to `main`. The workflow does not use secrets.

| Job | What it runs |
| --- | --- |
| `msrv (1.99.0, ubuntu-latest)` | Rust **1.99.0**: `cargo check --all-targets`, `cargo test`, then both again with `--features uart` after installing `libudev-dev`. `nir` stays off this job. |
| `fmt (ubuntu-latest)` | stable + rustfmt: `cargo fmt --check` and `cargo fmt --check --manifest-path fuzz/Cargo.toml` |
| `test` (`ubuntu-latest`, `macos-latest`, `windows-latest`) | stable + clippy, default features: `cargo clippy --all-targets -- -D warnings`, `cargo build`, `cargo test`. `fail-fast: false`. |
| `uart (ubuntu-latest)` | `libudev-dev` and `libhdf5-dev`, then `cargo clippy --all-targets --all-features -- -D warnings`, `cargo check --features uart`, `cargo test --all-features`, `cargo test --all-features --examples`, and `cargo doc --no-deps --features uart` with `RUSTDOCFLAGS=-D warnings` |
| `fuzz-build (ubuntu-latest)` | nightly, `g++`, `cargo install cargo-fuzz --locked`, then `cargo +nightly fuzz build --dev --sanitizer none` (`CC=gcc`, `CXX=g++`, linker `g++`). Compile only. |

The `uart` job runs unit tests only. CI runners have no serial hardware.

## Coverage

[`.github/workflows/quality.yml`](.github/workflows/quality.yml) job
`coverage (ubuntu-latest)` installs `libudev-dev`, the `llvm-tools-preview`
component, and `cargo-llvm-cov`, then runs:

```bash
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info
```

Trusted pushes and same-repository pull requests upload that LCOV file to
Codecov and Qlty with GitHub Actions OIDC. Fork pull requests still generate
the report; Codecov uses its public-repository tokenless upload, and Qlty
skips the upload. Codacy coverage upload runs only when the
`CODACY_PROJECT_TOKEN` repository secret is set. The `Codacy analysis` job
runs on pushes to `main` only, and only when that secret is set.
[`.codecov.yml`](.codecov.yml) ignores `examples/**`, `fuzz/**`, and
`tests/**`. [`.codacy.yml`](.codacy.yml) excludes `AGENTS.md`, `REVIEW.md`,
and `CHANGELOG.md`.

## Fuzzing

Fuzz targets live under [`fuzz/`](fuzz/README.md). They exercise Q8.8 and
UART frame codecs. They do not enable the `uart` feature and they do not
open a serial port.

CI compiles them (`cargo +nightly fuzz build --dev --sanitizer none`). A long
campaign is opt-in, after `cargo install cargo-fuzz --locked`:

```bash
cargo fuzz run q88_codecs
cargo fuzz run encode_stimuli
cargo fuzz run decode_response
cargo fuzz run chunked_frames
```

If a local image exposes `clang` as `c++`, install `g++` and use the
`CC` / `CXX` / linker settings in [fuzz/README.md](fuzz/README.md). Promote a
crash from `fuzz/artifacts/<target>/` into `tests/fuzz_regressions.rs`.
Regenerate seeds with `python3 fuzz/seed_from_golden.py` after golden
fixtures change.

## Commits and pull requests

Conventional Commits, from [AGENTS.md](AGENTS.md):

- Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`
- Scopes: `fpga-export`, `fpga-metrics`, `fpga-bridge`, `ci`, `docs`

Branch prefixes in use: `feat/`, `fix/`, `chore/`, `docs/`.

In the pull request body:

- Link the GitHub issue with `Closes #N` when the PR resolves it, or
  `Refs #N` when it does not.
- Include the Linear issue key when one exists (keys in this project look
  like `RM-####`).

One issue per pull request. The checklist in
[`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md) is the
`cargo fmt` / clippy / `cargo test` bar from [CLAUDE.md](CLAUDE.md), plus the
fuzz-manifest format check that CI already runs.

Do not add `unsafe` without an explicit safety comment. Do not commit
secrets, API keys, or credentials. Keep UART host I/O behind
`#[cfg(feature = "uart")]`.

## Publishing

Do not run `cargo publish` as part of a contribution. Publishing is a
separate maintainer action. It requires a passing Basys 3 UART host session
recorded in [docs/hardware-smoke-note.md](docs/hardware-smoke-note.md) and
explicit maintainer authorization. See
[docs/release-readiness.md](docs/release-readiness.md).

## Vulnerabilities

Use the private route in [SECURITY.md](SECURITY.md). A public issue is for
bugs that are not vulnerabilities.
