#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Maintainer-run Basys 3 UART host-session smoke (GitHub #84). Requires an
# EXPLICIT serial port via SILICON_BRIDGE_PORT. This does NOT auto-probe, is
# NOT part of default CI, and is NOT referenced from release-readiness section 4.
# It builds and runs the `uart`-feature example against the caller's port.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Match release-readiness §4: hardware evidence must bind to the commit under
# test, not a dirty worktree of the smoke paths.
SMOKE_PATHS=(Cargo.toml Cargo.lock examples/uart_host_smoke.rs src/)
if ! git diff --quiet HEAD -- "${SMOKE_PATHS[@]}" \
  || ! git diff --cached --quiet HEAD -- "${SMOKE_PATHS[@]}"; then
  echo "==> refusing: uncommitted changes under ${SMOKE_PATHS[*]}" >&2
  echo "    commit or stash the publish candidate before recording hardware evidence" >&2
  exit 1
fi

if [[ -z "${SILICON_BRIDGE_PORT:-}" ]]; then
  echo "==> SILICON_BRIDGE_PORT is not set (this smoke does not auto-probe)" >&2
  echo "    set an explicit serial port, e.g.:" >&2
  echo "    SILICON_BRIDGE_PORT=/dev/ttyUSB0 bash scripts/smoke-hardware-uart.sh" >&2
  echo "    optional baud override: SILICON_BRIDGE_BAUD=115200" >&2
  exit 1
fi
BAUD="${SILICON_BRIDGE_BAUD:-115200}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/silicon-bridge-hw-uart-smoke.XXXXXX")"
cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "==> reference: Digilent Basys 3 / XC7A35T-1CPG236C / spikenaut_soc_basys3_top"
echo "==> serial port ${SILICON_BRIDGE_PORT} @ ${BAUD} baud (explicit, no probe)"
echo "==> cargo run --features uart --example uart_host_smoke"

# Capture the example's exit status while still printing the disclaimer below,
# even on FAIL (the maintainer records a FAIL result too). `pipefail` makes the
# pipeline reflect cargo's status, and we defer aborting until after the notes.
status=0
SILICON_BRIDGE_BAUD="$BAUD" \
  cargo run --locked --features uart --example uart_host_smoke -- "$SILICON_BRIDGE_PORT" \
  | tee "$WORK/smoke.out" || status=$?

if [[ "$status" -eq 0 ]]; then
  echo "==> PASS: proved a live silicon-bridge UART host session on ONE named board."
else
  echo "==> FAIL (exit ${status}): no live silicon-bridge UART host session was proved."
fi
echo "==> A PASS proves only a live UART host session on ONE named board. It is"
echo "    distinct from the silicon-hdl LED-heartbeat smoke (#68), which is NOT a"
echo "    silicon-bridge UART host session, and is NOT a crates.io / docs.rs"
echo "    publication proof."
echo "==> Record the PASS/FAIL line above in docs/hardware-smoke-note.md (#84)."

exit "$status"
