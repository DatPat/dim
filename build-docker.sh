#!/usr/bin/env bash
set -euo pipefail

IMAGE="trashcorpinc/dim"
DOCKERFILE="Dockerfile.allfeatures"
PLATFORM="${1:-arm64}"   # pass "amd64" as first arg to cross-build

if [[ "$PLATFORM" != "arm64" && "$PLATFORM" != "amd64" ]]; then
    echo "Usage: $0 [arm64|amd64]"
    exit 1
fi

# Check docker login
DOCKER_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}"
if ! grep -qs "index.docker.io\|docker.io" "$DOCKER_CONFIG/config.json" 2>/dev/null; then
    echo "WARNING: You don't appear to be logged in to Docker Hub."
    echo "Run 'docker login' first, then re-run this script."
    exit 1
fi

# Resolve git info for the version display
GIT_TAG=$(cd dim-master && git describe --abbrev=0 2>/dev/null || echo "unknown")
GIT_SHA=$(cd dim-master && git rev-parse HEAD 2>/dev/null || echo "unknown")
SHORT_SHA="${GIT_SHA:0:8}"

# Generate docker tags
DATE_TAG=$(date +%Y%m%d)
if [[ "$SHORT_SHA" != "unknown" ]]; then
    VERSION_TAG="${DATE_TAG}-${SHORT_SHA}"
else
    VERSION_TAG="${DATE_TAG}"
fi

TAG_LATEST="${IMAGE}:latest"
TAG_VERSION="${IMAGE}:${VERSION_TAG}"

echo ""
echo "Building image for linux/${PLATFORM}..."
echo "  Tags: ${TAG_LATEST}, ${TAG_VERSION}"
echo ""

docker build \
    --platform "linux/${PLATFORM}" \
    --file "$DOCKERFILE" \
    --build-arg "GIT_TAG=${GIT_TAG}" \
    --build-arg "GIT_SHA=${GIT_SHA}" \
    --tag "$TAG_LATEST" \
    --tag "$TAG_VERSION" \
    --no-cache \
    .

echo ""
echo "Pushing..."
docker push "$TAG_LATEST"
docker push "$TAG_VERSION"

echo ""
echo "Successfully pushed:"
echo "  ${TAG_LATEST}"
echo "  ${TAG_VERSION}"
