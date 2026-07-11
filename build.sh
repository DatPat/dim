#!/bin/bash
set -e

IMAGE="trashcorpinc/dim:latest"

docker buildx build \
  --platform linux/arm64,linux/amd64 \
  -t "$IMAGE" \
  --push \
  /Volumes/misc/dim
