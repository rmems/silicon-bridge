# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `encode_q88_unsigned` — free-function unsigned Q8.8 encoder shared by
  `FixedPointEncode::encode_q88`, `.mem` export, and `format_q88_hex` (#23).
- `encode_q88_signed`, `q88_signed_to_f32`, `STIMULUS_Q88_MIN`, and
  `STIMULUS_Q88_MAX` — signed Q8.8 host-stimulus helpers used by the UART TX/RX
  path (#23).

### Changed

- License switched from GPL-3.0-or-later to dual MIT/Apache-2.0 (#6).
- Documented the two coexisting Q8.8 conventions (unsigned `u16` export vs
  signed `i16` UART stimuli) with a side-by-side table in the crate, module, and
  README docs, plus tests covering both clamp boundaries (#23).
- Declared `rust-version = "1.98.1"` (current stable), not `"1.85"`, the
  dependency-derived floor — edition 2024 and `getrandom` 0.4 (via the
  `tempfile` dev-dependency) both only require 1.85.0. This is a deliberate
  policy choice to track stable rather than the minimum the graph needs, and
  it narrows compatibility versus a bare 1.85 declaration: Cargo will refuse
  to build this crate on any toolchain from 1.85.0 up to (but excluding)
  1.98.1, even though every such toolchain can actually compile it. A
  toolchain older than 1.85 still gets the pre-existing `edition = "2024"`
  parse error instead, because Cargo rejects that while parsing the manifest,
  before `rust-version` is consulted.
