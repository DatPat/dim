#!/usr/bin/env bash
# Rebuild the React frontend
set -euo pipefail
cd "$(dirname "$0")/dim-master"

echo "==> Installing UI dependencies..."
yarn --cwd ui/

echo "==> Building UI..."
yarn --cwd ui/ build

echo "==> Done. UI output at: ui/build/"
