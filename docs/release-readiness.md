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

The current crates.io-intended semver is **0.3.1**. Registry state must still
be checked immediately before publication. README and crate rustdoc keep
git/path install lines until a maintainer authorizes the exact registry
publish. Do not treat a crates.io badge or an in-tree `0.3.1` string as a
live registry crate.

The reference FPGA named in README / `docs/consumer.md` is Digilent Basys 3
(Artix-7 `XC7A35T-1CPG236C`, `xc7a35tcpg236-1`) with silicon-hdl
`spikenaut_soc_basys3_top`. silicon-hdl LED heartbeat smoke (#68) is **not**
that host proof.

## 2. Supported Rust version

- **MSRV:** `[package].rust-version` is `1.88.0` (language floor: edition 2024
  plus `if`/`let` chains). The dependency graph would compile on 1.85.0 without
  those language features. CI enforces this floor with a dedicated **1.88.0**
  job (`cargo check --all-targets`, `cargo test`, and `--features uart` with
  `libudev-dev` on Linux). The optional `nir` feature is validated on stable
  only because `nir-rs` declares a higher MSRV than this crate.
- **Stable:** CI also runs the moving GitHub Actions `stable` toolchain
  (formatting, clippy, multi-OS default-feature tests, UART/rustdoc, fuzz
  compile). That lane tracks current stable; it is not a second MSRV pin.
- **Raising MSRV:** Do not bump `rust-version` to match the newest stable
  without an explicit compatibility decision. When MSRV increases, update
  `Cargo.toml`, README, this section, and add a **Changed** entry under
  `[Unreleased]` in `CHANGELOG.md` before release.
- Build and test default features on 1.88.0 or newer stable. The optional
  `nir` feature pins `nir-rs = 0.4.4`; validate it on a toolchain new enough
  for `nir-rs` before claiming that feature.
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
The Basys 3 UART host session that gates 0.3.1 publish lives in GitHub
[#84](https://github.com/rmems/silicon-bridge/issues/84), not here.

## 5. Packaged-crate smoke test (not registry proof)

```bash
bash scripts/smoke-packaged-consumer.sh
```

That builds a tiny out-of-tree crate against the **unpacked** `cargo package`
artifact. It does not prove crates.io or docs.rs.

## 6. After an authorized `cargo publish`

Only then:

- Confirm `https://crates.io/crates/silicon-bridge` shows the version.
- Confirm `https://docs.rs/silicon-bridge` builds with no rustdoc warnings.
- Repeat the smoke test with `silicon-bridge = "<published version>"` from
  the registry, not a path. Do not call a package-source test a registry test.

Do **not** wait until this step to rewrite README/rustdoc installation;
that rewrite belongs on the candidate commit in step 1.5 / item 5 above.

## 7. Out of scope here

- NIR HDF5 I/O (GitHub [#15](https://github.com/rmems/silicon-bridge/issues/15))
- Flashing silicon-hdl / Basys 3 (owned by silicon-hdl + GitHub [#84](https://github.com/rmems/silicon-bridge/issues/84))
- Changing the GitHub Actions matrix (#24 already owns it)
