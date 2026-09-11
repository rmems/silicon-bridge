<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Technical debt register

Ranked inventory of known debt in `silicon-bridge`, with the file that carries
it and the scope of the PR that clears it. Companion to
[boundary-matrix.md](boundary-matrix.md) (what this crate owns) and
[REVIEW.md](../REVIEW.md) (the quality bar a change must clear).

Ranking is by **return on a small PR**: severity of the failure mode, divided by
how much has to change to remove it. Items with an open PR are listed with it;
items marked *blocked* name what they wait on, because touching the same lines
would conflict with work already in review.

Line references are against `main` at the time of writing and drift as PRs land;
the file and symbol names are the durable part.

---

## Ranked

| # | Debt | Where | Severity | State |
|---|---|---|---|---|
| 1 | Project-board sync targets the pre-transfer org and fails every run | `.github/workflows/project-hardware.yml` | High | PR #38 |
| 2 | `uart` feature is unusable off Linux | `src/fpga_bridge.rs` | High | PR #39 |
| 3 | Ragged weight rows silently produce a misaligned `.mem` | `src/fpga_export.rs` | High | PR #41 |
| 4 | `FpgaMetrics::synthesis_ok` is hard-coded `true` | `src/fpga_metrics.rs:65,76` | High | blocked by #33 |
| 5 | No required status checks — a red PR can merge | branch protection on `main` | High | settings change |
| 6 | CI never runs on a stacked PR | `.github/workflows/ci.yml:6-7` | High | PR #46 |
| 7 | Published crate shipped agent instructions; no MSRV | `Cargo.toml` | Medium | PR #40 |
| 8 | Doc and `unsafe` conventions unenforced | `src/lib.rs`, `src/fpga_export.rs:104` | Medium | PR #42 |
| 9 | A library writes 20 lines to stdout on every export | `src/fpga_export.rs:195`, `src/fpga_bridge.rs:46` | Medium | queued |
| 10 | No dependency automation | repo-wide | Medium | PR #43 |
| 11 | `load_from_project` hard-codes an Eagle-Lander path | `src/fpga_metrics.rs:58` | Medium | blocked by #33 |
| 12 | No `cargo doc` gate, though docs.rs breakage is a stated concern | `.github/workflows/ci.yml` | Medium | blocked by #31 |
| 13 | README documents the Linux-only probe list as current | `README.md:29-33` | Medium | blocked by #33 |
| 14 | No MSRV job, so `rust-version` can rot | `.github/workflows/ci.yml` | Low-med | blocked by #31 + #40 |
| 15 | README examples are never compiled | `README.md`, `src/lib.rs` | Low-med | queued |
| 16 | Report paths are `&str`, not `AsRef<Path>` | `src/fpga_metrics.rs:70` | Low-med | blocked by #33 |
| 17 | Neuron counts across the three vectors are unchecked | `src/fpga_export.rs` | Low-med | queued |
| 18 | `ping()` latches the bridge inactive forever | `src/fpga_bridge.rs:108-117` | Low-med | queued |
| 19 | `LICENSE-MIT` still names the pre-transfer org | `LICENSE-MIT:3` | Low | **owner decision** |
| 20 | 2.4 MB logo; the `imgbot` branch that shrinks it is unmergeable | `docs/logo.png` | Low | queued |
| 21 | 12 stale remote branches | remote refs | Low | cleanup |
| 22 | No `CONTRIBUTING`, `SECURITY`, templates, or `CODEOWNERS` | `.github/` | Low | queued |
| 23 | No vulnerability audit in CI | `.github/workflows/ci.yml` | Low | blocked by #31 |

---

## Detail

### 1. Project-board sync targets the pre-transfer org and fails every run

`.github/workflows/project-hardware.yml` carried `PROJECT_ID:
PVT_kwDOEEyGS84BfB8y`, which resolves to **Limen-Neural org project #8
"Hardware"** — a board this repository left during the transfer to `rmems`
(#27). `STATUS_FIELD_ID` and the `Ready` option id belonged to that board too.
Separately, no project-scoped token is configured, so `GH_TOKEN` fell through to
`GITHUB_TOKEN`, which cannot mutate ProjectsV2 at all: every run since the
transfer ended in `gh: Resource not accessible by integration`, putting a red X
on the repository for each issue opened or closed.

**Fix (PR #38)** — repoint at rmems user project #5 (`PVT_kwHODs2Zb84BfcTG`),
drop the stale `secrets.LIMEN_NEURAL_GITHUB_TOKEN` fallback, and preflight the
token so `issues` events skip with a warning while `workflow_dispatch` still
fails loudly.

### 2. `uart` feature is unusable off Linux

`find_fpga_ports` filtered on `port_name.contains("ttyUSB")` and
`FpgaBridge::new` probed a hard-coded `/dev/ttyUSB0..2`. Both are Linux
spellings, so the whole feature was dead on macOS (`cu.usbserial-*`) and Windows
(`COM3`) — and no entry point accepted a port name, so there was no workaround.
#31 adds macOS and Windows CI runners for a feature that cannot work there.

**Fix (PR #39)** — `is_fpga_port_name` covering all three platforms,
`FpgaBridge::open(port_name)`, discovery-driven `new()` with the legacy list as
a fallback, and the module's first eight tests.

### 3. Ragged weight rows silently produce a misaligned `.mem`

`ParameterExport::export` flattens `Vec<Vec<f32>>` row-major and takes
`num_channels` from row 0 alone. Rows of `[2, 1, 2]` yield a 5-word buffer
described as a 3×2 matrix, so `WeightRam` addressed as `row * num_channels +
channel` reads the wrong words from row 1 on and overruns the buffer on row 2.
The `.mem` file looks entirely valid; the corruption only appears as wrong
spikes on hardware.

**Fix (PR #41)** — `ParameterShapeError` + `FpgaParameterExporter::validate`,
enforced by `write_mem_files` before anything is written to disk.

### 4. `FpgaMetrics::synthesis_ok` is hard-coded `true`

Both loaders set `synthesis_ok: true` whenever a report *parses*
(`src/fpga_metrics.rs:65` and `:76`). A CI gate reading the field therefore gets
a fabricated pass from any file containing a `WNS(ns)` header — including one
from a failed implementation run. `lut_utilization: 0.0` is the same shape of
lie, and is what #33 fixes.

**Fix scope (blocked by #33, which rewrites this file)** — derive
`synthesis_ok` from the report (an implementation run that errored says so), or
remove the field rather than ship a value that is always `true`. One PR,
`src/fpga_metrics.rs` only.

### 5. No required status checks — a red PR can merge

`main`'s branch protection has `required_linear_history` and blocks force-pushes
and deletions, but has **no `required_status_checks`** and
`required_approving_review_count: 0`. Nothing mechanically stops a merge with CI
red. `required_conversation_resolution` is also `false`, so unresolved review
threads do not block either. Today's green record is a matter of discipline, not
enforcement.

**Fix scope** — a settings change, not a PR: mark the #31 job names (`fmt
(ubuntu-latest)`, `test (ubuntu-latest)`, `test (macos-latest)`, `test
(windows-latest)`, `uart (ubuntu-latest)`) as required once #31 lands, and turn
on required conversation resolution. Worth doing right after #31 merges, so the
required names match the jobs that actually exist.

### 6. CI never runs on a stacked PR

`.github/workflows/ci.yml:6-7` filters `pull_request` to `branches: [main]`, so
the workflow is skipped entirely for any PR based on another branch. Because a
base retarget fires `pull_request: edited` — not one of the default trigger
types — the checks do not appear when the parent merges either. A stacked change
can reach `main` having never been built.

Observed directly while filing this register: #42 (based on
`fix/uart-cross-platform-ports`) and #45 (based on `ci/multi-os-uart`) show only
the third-party review apps in their check lists. No `fmt`, no `test`, no
`uart` — those jobs never ran. The review apps respond because they subscribe to
`pull_request` webhooks themselves; the repository's own workflow does not. A
stacked PR that looks green is green on nothing.

**Fix (PR #46)** — drop the `branches` filter from `pull_request`, keeping it on
`push` so post-merge builds stay limited to `main`.

### 7. Published crate shipped agent instructions; no MSRV

`exclude = ["docs/"]` is a deny-list, so `AGENTS.md`, `CLAUDE.md`, `REVIEW.md`,
`.codacy.yml`, `.gitignore` and `.github/` all shipped to crates.io — 19 files,
99.5 KiB. There was also no `rust-version`, so a pre-1.85 toolchain hit an
edition-2024 parse error instead of a clean MSRV message.

**Fix (PR #40)** — `include` allow-list (12 files, 81.5 KiB) plus
`rust-version = "1.85"`, verified against the 1.85.0 toolchain.

### 8. Doc and `unsafe` conventions unenforced

AGENTS.md marks "public items get `///` doc comments" and "no `unsafe` without
justification" as mandatory, but nothing checked either. `FpgaMetadata` and all
six of its public fields (`src/fpga_export.rs:104-111`) reached `main`
undocumented — the struct describing the `.mem` layout renders on docs.rs as a
bare field list.

**Fix (PR #42)** — `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`, and the
missing docs. Stacked on #39, which supplies the `FpgaBridge` doc the lint needs
under `--features uart`.

### 9. A library writes 20 lines to stdout on every export

`print_export_summary` (`src/fpga_export.rs:195`) issues twenty `println!`s and
is called unconditionally from `MemFileWriter::write_mem_files`
(`src/fpga_export.rs:301`). `FpgaBridge::new` prints on connect
(`src/fpga_bridge.rs:46`). A library has no business owning its caller's
stdout — any tool that emits JSON or pipes `.mem` output gets it interleaved
with export chatter, with no way to turn it off.

**Fix scope** — return the summary as a `Display` type, or gate it behind an
opt-in `FpgaParameterExporter::with_summary(bool)` defaulting to silent. Pre-1.0
and CHANGELOG-able. One PR, `src/fpga_export.rs`; fold the bridge `println!` in
or take it with #39's follow-up. Sequence after #41 — same file, overlapping
region.

### 10. No dependency automation

`Cargo.lock` is gitignored, so the `Cargo.toml` requirement is the only place a
floor can be raised, and `ci.yml` pins `dtolnay/rust-toolchain` by SHA — the pin
most likely to rot precisely because nothing nudges it.

**Fix (PR #43)** — weekly grouped Dependabot for `cargo` and `github-actions`.

### 11. `load_from_project` hard-codes an Eagle-Lander path

`src/fpga_metrics.rs:58` reads
`fpga-project/ship_ssn_logic.runs/impl_1/Basys3_Top_timing_summary_routed.rpt`.
That is one specific private project's directory layout baked into a
general-purpose crate: it can only ever succeed inside Eagle-Lander's tree.

**Fix scope (blocked by #33, which rewrites the loaders)** — take the project
root and top-module name as parameters, keeping the current string as a
documented default. One PR, `src/fpga_metrics.rs`.

### 12. No `cargo doc` gate

AGENTS.md bans relative links to licence files in doc comments *because they
break on docs.rs* — a rule that exists precisely because doc rendering is not
checked. Nothing in CI runs `cargo doc`, so a broken intra-doc link or a
malformed doc table reaches docs.rs unnoticed.

**Fix scope (blocked by #31, which restructures `ci.yml`)** — add
`RUSTDOCFLAGS: -D warnings` + `cargo doc --no-deps --features uart` as a step on
#31's `uart` job, which already installs `libudev-dev`. A five-line diff.

### 13. README documents the Linux-only probe list as current

`README.md:29-33` and the UART section state that `FpgaBridge::new()` probes only
`/dev/ttyUSB0..2` "not ttyACM, Windows COM ports, or arbitrary USB paths". True
today, false the moment #39 lands.

**Fix scope (blocked by #33, whose README hunk overlaps these lines)** — rewrite
the two passages around cross-platform discovery and `FpgaBridge::open`. Docs
only.

### 14. No MSRV job

`rust-version = "1.85"` (PR #40) is only as good as what checks it. Nothing
builds on the declared minimum, so the floor silently rises the first time
someone uses a newer API.

**Fix scope (blocked by #31 and #40)** — one `msrv` job pinning the toolchain to
the `rust-version` value, running `cargo check --all-targets`.

### 15. README examples are never compiled

The crate has no `#![doc = include_str!("../README.md")]`, so none of the
README's four `rust` blocks are doctested. They can drift from the API without
any signal — the `FpgaMetrics` example is already describing behaviour #33
changes.

**Fix scope** — include the README as crate docs, marking the `?`-using UART
example `no_run` or `ignore` as needed. One PR touching `src/lib.rs` and the
README's fences. Sequence after #31 and #33 clear the README.

### 16. Report paths are `&str`, not `AsRef<Path>`

`FpgaMetrics::load_from_path(report_path: &str)` (`src/fpga_metrics.rs:70`)
rejects non-UTF-8 paths and forces callers holding a `Path` or `PathBuf` to
convert. `fpga_export.rs` already uses `impl AsRef<Path>` — the two modules
disagree with each other.

**Fix scope (blocked by #33)** — widen to `impl AsRef<Path>`. Source-compatible
for every `&str` caller.

### 17. Neuron counts across the three vectors are unchecked

Separate from #3: nothing requires `thresholds.len()`, `weights.len()` and
`decay_rates.len()` to agree. Sixteen thresholds with four weight rows exports
`num_neurons: 16` beside four rows of weights, and `NeuronParamRam` is loaded
short. Deliberately excluded from PR #41, because a partial export (thresholds
only, no weights yet) is a plausible intermediate use and turning it into an
error is a behaviour break that deserves its own discussion.

**Fix scope** — extend `ParameterShapeError` with a count-mismatch variant once
the partial-export question is settled. `#[non_exhaustive]` on the enum was
chosen with this in mind.

### 18. `ping()` latches the bridge inactive forever

`src/fpga_bridge.rs:108-117` sets `self.active = false` on *any* error, including
a single 100 ms read timeout, and nothing ever sets it back. One slow reply
permanently bricks the handle; every later `process_stimuli` returns "FPGA
bridge not active" without touching the port.

**Fix scope** — either drop the latch, or add a `reconnect()`. Small,
`src/fpga_bridge.rs` only. Sequence after #39.

### 19. `LICENSE-MIT` still names the pre-transfer org

`LICENSE-MIT:3` reads `Copyright (c) 2025 Limen-Neural`. Issue #27's acceptance
criteria explicitly cover licences, and this is the last file that still names
the old org in a load-bearing way.

**This one is not an agent's call.** Who holds copyright after the transfer is a
decision for the owner; the register flags it, and deliberately proposes no
patch. The other `Limen-Neural` mentions in the tree were checked and are
**correct**: `neuromod` and `nir-rs` really do still live under that org, and
`docs/boundary-matrix.md` refers to the stack by name, not by URL.

### 20. 2.4 MB logo and an unmergeable `imgbot` branch

`docs/logo.png` is 2,407,973 bytes — roughly 30× the entire published crate. It
is excluded from the package, but every clone pays for it. `origin/imgbot`
carries a recompression to 1,806,830 bytes, but it branched from an ancient
`main` and its diff would revert ~800 lines of subsequent work, so it cannot be
merged.

**Fix scope** — redo the recompression standalone (lossless `oxipng`/`zopflipng`,
or resize: nothing displays it above 220 px, which is what the README requests)
and close the `imgbot` branch.

### 21. 12 stale remote branches

Two are fully merged into `main` and safe to delete: `upgrade/silicon-bridge-renames`,
`viktor/add-ci-workflow`. Nine more are squash-merge artifacts — the work landed
on `main` as a squashed commit, so git still reports them unmerged:
`chore/migration-hygiene`, `cursor/docs-readme-accuracy-6d9b`,
`cursor/fix-q88-signed-unsigned-6d9b`, `cursor/test-mem-file-writer-6d9b`,
`docs/readme-accuracy`, `fix/q88-signed-unsigned-docs`,
`fix/remediation-7df322ef-9bc91a`, `test/fpga-metrics-wns`, `test/mem-file-writer`.
Plus `imgbot` (#20).

**Fix scope** — verify each against its closed PR, then delete. Listed rather
than deleted here because branch deletion is not reversible from a script.
Enabling "automatically delete head branches" on the repository stops the
backlog re-forming.

### 22. No `CONTRIBUTING`, `SECURITY`, templates, or `CODEOWNERS`

The repo is public and dual-licensed but offers no contribution guide, no
vulnerability-reporting route, and no PR/issue templates. AGENTS.md holds
conventions that only agents read.

**Fix scope** — one docs PR adding `CONTRIBUTING.md` (pointing at AGENTS.md for
conventions), `SECURITY.md`, and `.github/PULL_REQUEST_TEMPLATE.md` carrying the
`cargo fmt/clippy/test` checklist from CLAUDE.md's quality bar.

### 23. No vulnerability audit in CI

Nothing runs `cargo audit` or `cargo deny`. `serialport` pulls in `libudev-sys`
and `nix`, so the `uart` feature has real native surface area.

**Fix scope (blocked by #31)** — an `audit` job running
`rustsec/audit-check`, or `cargo deny check advisories` if licence checking is
wanted too. Note that with `Cargo.lock` uncommitted, an audit inspects freshly
resolved versions rather than what a consumer pins.
