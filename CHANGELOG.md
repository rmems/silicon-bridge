# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `FpgaBridge::open` and `is_fpga_port_name` (`uart` feature) — open a serial
  port by name, and classify a port name across Linux, macOS, and Windows.
- `encode_q88_unsigned` — free-function unsigned Q8.8 encoder shared by
  `FixedPointEncode::encode_q88`, `.mem` export, and `format_q88_hex` (#23).
- `encode_q88_signed`, `q88_signed_to_f32`, `STIMULUS_Q88_MIN`, and
  `STIMULUS_Q88_MAX` — signed Q8.8 host-stimulus helpers used by the UART TX/RX
  path (#23).

### Changed

- `find_fpga_ports` now matches macOS `cu.usb*` / `tty.usb*` nodes and Windows
  `COM<n>` ports as well as Linux `ttyUSB` / `ttyACM`; it previously filtered on
  a `ttyUSB` substring and so returned an empty list off Linux. `FpgaBridge::new`
  probes the discovered ports, falling back to `/dev/ttyUSB0..2` only when
  enumeration finds nothing.
- License switched from GPL-3.0-or-later to dual MIT/Apache-2.0 (#6).
- Documented the two coexisting Q8.8 conventions (unsigned `u16` export vs
  signed `i16` UART stimuli) with a side-by-side table in the crate, module, and
  README docs, plus tests covering both clamp boundaries (#23).
- The crate root now carries `#![forbid(unsafe_code)]` and
  `#![deny(missing_docs)]`, and `FpgaMetadata` and its fields gained the `///`
  comments they were missing.
