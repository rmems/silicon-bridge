#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Smoke-test a tiny out-of-tree consumer against the unpacked `cargo package`
# artifact. This does **not** prove crates.io publication.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
if [[ -z "$VERSION" ]]; then
  echo "could not read package version from Cargo.toml" >&2
  exit 1
fi

echo "==> packaging silicon-bridge ${VERSION}"
cargo package --no-verify --allow-dirty

CRATE_DIR="${ROOT}/target/package/silicon-bridge-${VERSION}"
if [[ ! -d "$CRATE_DIR" ]]; then
  echo "expected unpacked crate at ${CRATE_DIR}" >&2
  exit 1
fi

CONSUMER="$(mktemp -d "${TMPDIR:-/tmp}/silicon-bridge-packaged-consumer.XXXXXX")"
cleanup() {
  rm -rf "$CONSUMER"
}
trap cleanup EXIT

mkdir -p "${CONSUMER}/src"
cat > "${CONSUMER}/Cargo.toml" <<EOF
[package]
name = "silicon-bridge-packaged-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
silicon-bridge = { path = "${CRATE_DIR}" }
EOF

cat > "${CONSUMER}/src/main.rs" <<'EOF'
//! Tiny out-of-tree consumer. Public API only; no sibling checkout.
use silicon_bridge::{
    CheckedParameterExport, DenseQ88Layout, FpgaParameterExporter, encode_stimuli, format_q88_hex,
};

fn main() {
    let mut exporter = FpgaParameterExporter::from_params(
        vec![1.0, 0.5, 1.5, 0.75],
        vec![
            vec![0.5, -1.0, 0.25, 1.0, -0.5, 0.0],
            vec![-128.0, 127.99609375, 1.0 / 256.0, -1.0 / 256.0, 2.0, -2.0],
            vec![1.0; 6],
            vec![-0.5, 0.5, -0.5, 0.5, -0.5, 0.5],
        ],
        vec![0.5, 0.75, 0.25, 1.0],
    );
    exporter.set_format_version("generic-dense-q88");
    exporter.set_timestamp("1970-01-01T00:00:00Z");
    let params = CheckedParameterExport::try_export(&exporter).expect("checked");
    assert_eq!(params.metadata.version, "generic-dense-q88");
    assert!(!params.metadata.version.contains("Spikenaut"));
    assert_eq!(format_q88_hex(-1.0), "FF00");
    assert_eq!(params.weights[1], -256);

    let v3 = DenseQ88Layout::silicon_bridge_v3();
    let tx = encode_stimuli(&v3, &[0.0; 16]).expect("v3 host codec");
    assert_eq!(tx.len(), 33);
    println!("packaged-crate consumer ok (not a crates.io proof)");
}
EOF

echo "==> building unpackaged crate examples against public API"
cargo build --examples --manifest-path "${CRATE_DIR}/Cargo.toml"

echo "==> building out-of-tree consumer against unpacked package"
cargo build --manifest-path "${CONSUMER}/Cargo.toml"
cargo run --manifest-path "${CONSUMER}/Cargo.toml"

echo "==> package-source smoke test passed (registry publication is a separate step)"
