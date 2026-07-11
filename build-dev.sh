#!/usr/bin/env bash
# Build Dim in debug mode (fast compile, slower runtime)
set -euo pipefail
cd "$(dirname "$0")/dim-master"

# Ensure ffmpeg/ffprobe symlinks exist
mkdir -p utils
[ -L utils/ffmpeg ]  || ln -nfs "$(which ffmpeg)"  utils/ffmpeg
[ -L utils/ffprobe ] || ln -nfs "$(which ffprobe)" utils/ffprobe

echo "==> Building Dim (debug)..."
cargo build 2>&1
echo "==> Done. Binary at: target/debug/dim"
