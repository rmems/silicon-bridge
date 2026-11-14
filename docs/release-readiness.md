<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Public-release readiness

Maintainer checklist for a first (or subsequent) crates.io release of
`silicon-bridge`. Completing this file does **not** publish the crate.

`cargo publish` is a separately authorized action. Do not run it from a
docs/examples ticket, a CI job, or an agent session unless a maintainer
explicitly authorized that exact version.

## 1. Choose the version from registry state

1. Fetch `https://crates.io/api/v1/crates/silicon-bridge`.
2. **404** means nothing is published. Do not treat the README crates.io
   badge, a `version` in `Cargo.toml`, or docs.rs links as proof of a live
   registry crate until `cargo publish` has actually succeeded.
3. If a version exists, pick the next semver and confirm it is not already
   yanked/occupied.
4. Set `[package].version` to that number on the **exact candidate commit**.
5. **Rewrite installation docs on that same commit** before `cargo package`
   / `cargo publish --dry-run`: README and crate-level rustdoc must say
   `silicon-bridge = "<that version>"` (and drop “not published” language).
   crates.io tarballs and the docs.rs build for that version are immutable;
   a later git commit cannot correct them.

Registry state at release: crates.io previously served **0.3.0** only
(published 2026-09-17, tag `v0.3.0` / `bf5b20d`, not yanked; the docs.rs
build succeeded); **0.4.0** is the release this checklist gates. The version
is 0.4.0, not 0.3.1: since 0.3.0 the tree removed
`FpgaMetrics::load_from_project`, changed the types of
`ExportConfig::profile()` and `ExportReport::profile`, and turned
`ExportError::MissingRequiredReadout` from a unit into a struct variant.
Cargo resolves `silicon-bridge = "0.3.0"` as `>=0.3.0, <0.4.0`, so a 0.3.1
carrying those changes would have reached every existing consumer on their
next `cargo update` and broken their build; the minor bump keeps them on
0.3.0 until they opt in. Apply the same rule in future: for a 0.x crate, any
removal or type change of a public item bumps the minor version. README,
crate rustdoc, and `docs/consumer.md` were rewritten to
`silicon-bridge = "0.4.0"` on the release commit per step 5 above; this
file ships in the tarball too. Do not treat a crates.io badge or an in-tree
`0.4.0` string as proof of a live registry version — confirm the registry
after the authorized publish (step 7).

The reference FPGA named in README / `docs/consumer.md` is Digilent Basys 3
(Artix-7 `XC7A35T-1CPG236C`, `xc7a35tcpg236-1`) with silicon-hdl
`spikenaut_soc_basys3_top`. silicon-hdl LED heartbeat smoke (#68) is **not**
that host proof.

## 2. Supported Rust version

- **MSRV:** `[package].rust-version` is `1.88.0` (unchanged from 0.3.0) for
  default features and `uart`; edition 2024 plus `if`/`let` chains set that
  floor. CI enforces it with a dedicated **1.88.0** job (`cargo check
  --all-targets`, `cargo test`, and `--features uart` with `libudev-dev` on
  Linux).
- **NIR floor:** `nir` / `nir-io` pin `nir-rs = 0.4.5`, which declares
  `rust-version = "1.98.1"`. CI enforces that with a **1.98.1** `msrv-nir`
  job (`--features nir`). `nir-io` needs native libhdf5 and runs on the
  1.99.0 dev lane.
- **Dev toolchain:** CI's non-MSRV jobs pin Rust **1.99.0** via
  `rust-toolchain.toml` (formatting, clippy, multi-OS default-feature tests,
  UART/rustdoc, coverage); the fuzz lane uses `nightly`, and the `msrv` /
  `msrv-nir` jobs override the pin with `RUSTUP_TOOLCHAIN`. That lane is not
  a second MSRV pin.
- **Raising MSRV:** Do not bump `rust-version` to match the newest stable
  without an explicit compatibility decision. When MSRV increases, update
  `Cargo.toml`, README, this section, and add a **Changed** entry under
  `[Unreleased]` in `CHANGELOG.md` before release.
- Build and test default features on 1.88.0 or newer, and `nir` on 1.98.1
  or newer. `nir-io` also needs native libhdf5 and stays out of both MSRV
  jobs.
- Record the toolchain and enabled features in the release notes.

## 3. License and package contents

- Dual MIT / Apache-2.0; both `LICENSE-MIT` and `LICENSE-APACHE` must ship.
- Review `include` in `Cargo.toml`. Agent instruction files and `docs/logo.png`
  stay out of the tarball on purpose.
- `cargo package --list` on the candidate commit: confirm examples you intend
  to ship are listed, and that `.github/`, `AGENTS.md`, and secrets are not.

## 4. Checks on the candidate commit

Run on the commit you will tag, not a dirty tree:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --features nir
# Needs libhdf5-dev:
cargo test --features nir-io
# Linux with libudev-dev:
cargo check --features uart
cargo test --features uart
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --features uart
cargo run --example generic_mem_export
cargo run --example spikenaut_profile_export
cargo run --example dense_codec
cargo package --list
cargo publish --dry-run
```

CI matrix coverage is GitHub [#24](https://github.com/rmems/silicon-bridge/issues/24)
(`.github/workflows/ci.yml`). Do not open a duplicate CI or “release umbrella”
issue for this checklist.

Do **not** flash an FPGA or send UART stimuli as part of this checklist.
The Basys 3 UART host session that gates 0.4.0 publish is the separate,
maintainer-run hardware-evidence gate in §5 below, still tracked under GitHub
[#84](https://github.com/rmems/silicon-bridge/issues/84).

## 5. Hardware-evidence gate (Basys 3 UART host session)

Separate from the §4 checklist, which does **not** flash an FPGA. This is a
maintainer-run hardware step, still tracked under GitHub
[#84](https://github.com/rmems/silicon-bridge/issues/84).

Before the authorized `cargo publish` in §7, all of the following must hold:

1. `docs/hardware-smoke-note.md` records a **passing** silicon-bridge UART host
   session on the reference Basys 3 (Artix-7 `XC7A35T-1CPG236C`, silicon-hdl
   `spikenaut_soc_basys3_top`), with every session-record field filled in and a
   `Result` of exactly `PASS`.
2. The note's recorded **Candidate SHA** is the commit being tagged and
   published in §7 (not the note's own commit; see the note for why "current
   `HEAD`" is the wrong reference). A recorded `PASS` whose Candidate SHA is not
   the publish candidate does **not** satisfy this gate.
3. The note's Status line and the "Host-session evidence" row in
   `docs/consumer.md` are updated to agree with the recorded result, so no
   document claims "not yet recorded" once a `PASS` is on file.
4. A maintainer records explicit go-ahead in that note.

Produce the evidence with an explicit serial port:

```bash
SILICON_BRIDGE_PORT=/dev/ttyUSB0 bash scripts/smoke-hardware-uart.sh
```

That script builds and runs `examples/uart_host_smoke.rs` (`uart` feature)
against the caller's port and prints a `PASS`/`FAIL` line to copy into
`docs/hardware-smoke-note.md`.

An empty note does **not** satisfy this gate. The silicon-hdl LED-heartbeat
smoke ([#68](https://github.com/rmems/silicon-hdl/issues/68)) does **not**
satisfy it either; #68 is the LED-heartbeat smoke and is not a silicon-bridge
UART host session. This step is not part of default CI.

## 6. Packaged-crate smoke test (not registry proof)

```bash
bash scripts/smoke-packaged-consumer.sh
```

That builds a tiny out-of-tree crate against the **unpacked** `cargo package`
artifact. It does not prove crates.io or docs.rs.

## 7. After an authorized `cargo publish`

Only then:

- Confirm `https://crates.io/crates/silicon-bridge` shows the version.
- Confirm `https://docs.rs/silicon-bridge` builds with no rustdoc warnings.
- Repeat the smoke test with `silicon-bridge = "<published version>"` from
  the registry, not a path. Do not call a package-source test a registry test.

Do **not** wait until this step to rewrite README/rustdoc installation;
that rewrite belongs on the candidate commit in step 1.5 / item 5 above.

## 8. Out of scope here

- A local NIR HDF5 decoder (GitHub [#15](https://github.com/rmems/silicon-bridge/issues/15)):
  `.nir` reading is delegated to `nir-rs` via the opt-in `nir-io` feature
- Flashing silicon-hdl / building the Basys 3 bitstream (owned by silicon-hdl +
  GitHub [#84](https://github.com/rmems/silicon-bridge/issues/84)). The
  silicon-bridge UART host session that gates publish is the hardware-evidence
  gate in §5, not this list.
- Changing the GitHub Actions matrix (#24 already owns it)
