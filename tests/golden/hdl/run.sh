#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# HDL simulation evidence for #53, run by the `hdl-golden` CI job. Ordinary
# `cargo test` does not run this. Requires Icarus Verilog (`iverilog` / `vvp`)
# and python3 (standard library only).
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

echo "simulator: $(iverilog -V 2>/dev/null | sed -n 1p || true)"

# Turn expected.json into check_hex() calls so the simulation compares the
# words $readmemh loaded against the independently specified words.
python3 tests/golden/hdl/gen_expected_checks.py tests/golden "$OUT/expected_checks.svh"

iverilog -g2012 -s tb_readmemh_contract -o "$OUT/tb_readmemh_contract" \
  -DEXPECTED_JSON_CHECKS -I "$OUT" \
  tests/golden/hdl/WeightRam.sv \
  tests/golden/hdl/tb_readmemh_contract.sv

# vvp exits 0 even after $fatal on some Icarus builds, so also require the
# pass marker and reject any FAIL line.
LOG="$OUT/vvp.log"
set +e
vvp -n "$OUT/tb_readmemh_contract" 2>&1 | tee "$LOG"
VVP_STATUS=${PIPESTATUS[0]}
set -e

if [ "$VVP_STATUS" -ne 0 ]; then
  echo "hdl-golden: vvp exited $VVP_STATUS" >&2
  exit 1
fi
if grep -q '^FAIL' "$LOG" || ! grep -q '^TB_READMEMH_CONTRACT: ALL TESTS PASSED' "$LOG"; then
  echo "hdl-golden: testbench did not pass" >&2
  exit 1
fi
