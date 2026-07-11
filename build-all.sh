#!/usr/bin/env bash
# Full clean build: UI + Rust release binary
set -euo pipefail
cd "$(dirname "$0")/dim-master"

# Ensure ffmpeg/ffprobe symlinks exist
mkdir -p utils
[ -L utils/ffmpeg ]  || ln -nfs "$(which ffmpeg)"  utils/ffmpeg
[ -L utils/ffprobe ] || ln -nfs "$(which ffprobe)" utils/ffprobe

echo "==> Installing UI dependencies..."
yarn --cwd ui/

echo "==> Building UI..."
yarn --cwd ui/ build

echo "==> Building Dim (release)..."
cargo build --release 2>&1

echo "==> Done."
echo "    Binary: target/release/dim"
echo "    UI:     ui/build/"
echo "    Run:    ./run-release.sh"
