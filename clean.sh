#!/usr/bin/env bash
# Clean all build artifacts
set -euo pipefail
cd "$(dirname "$0")/dim-master"

echo "==> Cleaning Rust build artifacts..."
cargo clean

echo "==> Cleaning UI build..."
rm -rf ui/build

echo "==> Done."
