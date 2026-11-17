<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# aer-route-v1 — AER routing contract

Issue [#79](https://github.com/rmems/silicon-bridge/issues/79). This document
defines the `aer-route-v1` route-table image and runtime-write contract that
the `silicon-hdl-v3 + aer-route-v1` export profile emits. It mirrors the
hardware-consumer contract in silicon-hdl
[`docs/aer-routing-contract.md`](https://github.com/rmems/silicon-hdl/blob/main/docs/aer-routing-contract.md)
and `synapse-link-hdl/src/AerRouteTable.sv`, pinned at revision
`bcdf630c01016ea0ba4d4637af4911b81e30a592`
(`SILICON_HDL_AER_SUPPORTED_REVISION`).

## Entry bit layout

The table holds 16 entries of 16 bits. Address width is 4; the hop budget is
four lookups.

| Bits | Field | Meaning |
|---|---|---|
| 15 | `valid` | `0` drops the event and pulses `route_fault` |
| 14 | `terminal` | `1` delivers `next_addr`; `0` performs another lookup |
| 13:4 | reserved | must be `0`; any set bit fails closed |
| 3:0 | `next_addr` | delivered address (terminal) or next lookup index |

The identity entry for address `i` is `0xC000 | i`
(`AerRouteTable::identity()`). A `.mem` image is 16 `{:04X}` lines in
ascending address order — the same text format as parameter images; route
words are never Q8.8-quantized.

## Lookup rule

The initial access counts as lookup 1. Lookup 4 must be terminal; a route
that still forwards there — including a cycle — is dropped with
`route_fault` in hardware and refused by `AerRouteTable::validate()` as
`AerRouteError::OverBudget`. A valid non-terminal chain that reaches an
invalid entry is `AerRouteError::DanglingRoute`. A *start* entry with
`valid = 0` is allowed: it is a deliberately disabled route, not a malformed
one.

## Runtime write command (target 3)

Route entries are written at runtime with the existing five-byte `0xA5`
command; no new framing is introduced.

| Byte | Value |
|---|---|
| 0 | `0xA5` (`ROUTE_WRITE_COMMAND`) |
| 1 | `0x03` (`ROUTE_TABLE_TARGET`) |
| 2 | address (low four bits used; `≥ 16` is rejected) |
| 3 | entry `data_hi` (big-endian) |
| 4 | entry `data_lo` |

- Targets 0/1/2 (weight / threshold / leak RAM) are unchanged and outside
  this crate's scope; targets above 3 are invalid.
- The write is **never acknowledged**: `FpgaBridge::write_route_entry` /
  `write_route_table` (feature `uart`) send the frame(s) and return. There
  is no response field and no retry; a failed write latches
  `needs_recovery` like any other I/O failure.
- Reserved bits set in `data` or an address `≥ 16` are rejected with
  `route_fault` by the hardware and refused host-side with
  `AerRouteError` before any byte is written.
- The stimulus contract is unchanged: 33-byte `0xAA` request, 36-byte
  response (`SILICON_BRIDGE_V3_TX_LEN` / `SILICON_BRIDGE_V3_RX_LEN`).

## Generating the bundle

```rust
use silicon_bridge::{ExportConfig, FpgaParameterExporter};

// Pinned profile: 16 inputs, 16 hidden, 3 outputs, immutable filenames.
let config = ExportConfig::silicon_hdl_v3_aer_route_v1();
// Or an explicit hardware plan:
//   config.with_aer_route_table(table, AerRouteSource::ExplicitHardwarePlan)?;

let report = exporter.write_with_config("out/", &config)?;
// out/ now contains aer_routes.mem plus the usual parameter images.
```

With the `nir` feature,
`FpgaParameterExporter::aer_export_config_from_nir(&graph, plan)` validates
the canonical `Input → Linear → LIF → [readout] → Output` graph and derives
the identity table from canonical input ordering (`None`), or accepts an
explicit plan (`Some(table)`). Ordinary NIR edges never become FPGA hops.
`examples/nir_aer_export.rs` is a runnable demo
(`cargo run --features nir --example nir_aer_export`).

To write a table to the FPGA at runtime
(`uart` feature):

```rust,ignore
bridge.write_route_table(&AerRouteTable::identity())?;
bridge.write_route_entry(0, 0x8001)?; // one entry
```

## Manifest fields

`parameters.json` gains two fields only for this profile:

- `metadata.aer_routing`: `schema` (`aer-route-v1`), `filename`
  (`aer_routes.mem`), `entry_width` (16), `address_width` (4),
  `entry_count` (16), `max_hops` (4), `bit_layout`, `hdl_revision` (the pin
  above), `route_source` (`canonical_input_order` or
  `explicit_hardware_plan`), `nir_derived`.
- `metadata.artifact_hashes`: lowercase SHA-256 of each staged `.mem`
  artifact's exact bytes, keyed by filename. Covers every `.mem` in the
  bundle; excludes `parameters.json` itself.

## Evidence and claim limits

An export — even a NIR-derived identity bundle — does not prove board
success. Simulation and Basys 3 evidence for silicon-hdl #71 (routed
LIF/STDP addressing, fault behavior, sticky LD3) must record exact
revisions, artifact hashes, and UART traces, and is produced in the
silicon-hdl repository, not here. This repository does not claim board
success from its own tests.
