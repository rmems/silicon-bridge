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
- `FpgaMetrics::tns_ns` field plus `parse_tns_from_report`, `parse_lut_utilization`,
  and `load_from_reports` — TNS from the timing summary data row and LUT
  utilization from `report_utilization` output. Absent values degrade to `0.0`
  instead of failing the parse, and `tns_ns` is `#[serde(default)]` so metrics
  serialized before it existed still deserialize (#21).
  New public field: code outside the crate that builds `FpgaMetrics` with a
  struct literal must add `tns_ns` (or `..Default::default()`). Serialized
  payloads and field-access code are unaffected.

### Fixed

- `FpgaMetrics::parse_from_report` now skips the rule of dashes Vivado prints
  under the `WNS(ns)` column headers, so a verbatim timing summary parses
  instead of returning `None` (#21).

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
