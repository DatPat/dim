#!/usr/bin/env bash
# Build and run Dim in release mode
set -euo pipefail
cd "$(dirname "$0")/dim-master"

# Ensure ffmpeg/ffprobe symlinks exist
mkdir -p utils
[ -L utils/ffmpeg ]  || ln -nfs "$(which ffmpeg)"  utils/ffmpeg
[ -L utils/ffprobe ] || ln -nfs "$(which ffprobe)" utils/ffprobe

export RUST_LOG="${RUST_LOG:-info}"

echo "==> Building and running Dim (release) — http://0.0.0.0:8000"
cargo run --release 2>&1
