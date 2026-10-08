#!/usr/bin/env bash
# Cursor Cloud Agent install script for silicon-bridge (`install` in .cursor/environment.json).
#
# Cursor runs this from the repository root during every Build, on its default
# Ubuntu base image (CPU only: cloud agents have no GPU), then snapshots the disk.
# It must be idempotent. Shell exports don't survive into agent runs, so the tools
# it installs are exposed through /etc/profile.d and /usr/local/bin.
# See https://cursor.com/docs/cloud-agent/setup
#
# Installs only what this repo's CI and manifests need:
#   - apt: build-essential, pkg-config, libudev-dev, curl, ca-certificates
#   - Rust stable (+rustfmt, clippy, llvm-tools-preview) [default]
#   - Rust 1.88.0
#   - cargo check --all-targets --all-features
#   - tool directories exposed to later shells (/etc/profile.d + /usr/local/bin links)
#
# It ends with a dependency fetch/prebuild, not a test run.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

# Install apt packages that are not already present.
apt_install() {
  local missing=() pkg
  for pkg in "$@"; do
    if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "install ok installed"; then
      missing+=("$pkg")
    fi
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    ${SUDO} apt-get -o Acquire::Retries=5 update -qq
    ${SUDO} env DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=5 install -y --no-install-recommends "${missing[@]}"
  fi
}

# --- System packages (libudev-dev for the uart feature (ci.yml, quality.yml); curl for the installers) ---
apt_install build-essential pkg-config libudev-dev curl ca-certificates

# --- Rust (rustup) ---
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --default-toolchain none --profile minimal
fi
# ci.yml lint/test/feature jobs use stable.
rustup toolchain install stable --profile minimal --component rustfmt --component clippy --component llvm-tools-preview
rustup default stable
# ci.yml MSRV job pins 1.88.0 (rust-version).
rustup toolchain install 1.88.0 --profile minimal

# --- Prefetch and prebuild crates (no tests) ---
# Cargo fetch has no feature-selection flags, so check every target with every
# feature to cache the optional uart and nir dependencies used by CI.
cargo check --all-targets --all-features

# --- Expose the tools to later shells ---
# The PATH exports above last only for this script; Cursor starts the agent's shells
# separately. Login shells get these directories from /etc/profile.d, and every other
# shell finds the entry points through symlinks in /usr/local/bin (on the default PATH).
tool_dirs=("$HOME/.cargo/bin")
# shellcheck disable=SC2016 # $PATH must expand when the profile is sourced, not now.
printf 'export PATH="%s:$PATH"\n' "$(IFS=:; echo "${tool_dirs[*]}")" |
  $SUDO tee /etc/profile.d/cursor-env-silicon-bridge.sh >/dev/null
for dir in "${tool_dirs[@]}"; do
  [ -d "$dir" ] || continue
  for tool in "$dir"/*; do
    name="${tool##*/}"
    case "$name" in
      python* | pip* | activate* | deactivate | Activate.ps1) continue ;;
    esac
    if [ -f "$tool" ] && [ -x "$tool" ]; then
      $SUDO ln -sfn "$tool" "/usr/local/bin/$name"
    fi
  done
done

echo "Cursor install for silicon-bridge finished."
