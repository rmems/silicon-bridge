#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Optional HDL simulation evidence for #53. Ordinary `cargo test` does not
# run this. Requires Icarus Verilog (`iverilog` / `vvp`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

if ! command -v iverilog >/dev/null 2>&1; then
  echo "iverilog not found; HDL simulation evidence was not produced."
  echo "Install Icarus Verilog and re-run tests/golden/hdl/run.sh"
  exit 2
fi

OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

iverilog -g2012 -s tb_readmemh_contract -o "$OUT/tb_readmemh_contract" \
  tests/golden/hdl/WeightRam.sv \
  tests/golden/hdl/tb_readmemh_contract.sv
vvp "$OUT/tb_readmemh_contract"
