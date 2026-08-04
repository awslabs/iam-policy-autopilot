#!/usr/bin/env bash
#
# Build and package the paper-benchmarks artifact for submission (Zenodo upload).
#
# Produces: ipa-paper-benchmarks.tar.gz containing:
#   ipa-paper-benchmarks/
#   ├── README.md
#   ├── REQUIREMENTS.md
#   ├── STATUS.md
#   ├── LICENSE.md
#   ├── deploy_realworld_app.sh
#   └── ipa-paper-benchmarks-image.tar.gz
#
# Usage:
#   ./paper-benchmarks/scripts/package_artifact.sh [output-dir]
#
# The image is built for linux/amd64 (as REQUIREMENTS.md promises). Building on
# an arm64 host therefore needs QEMU/binfmt emulation; override with
# PLATFORM=linux/arm64 only for local testing, never for a submission artifact.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$BENCH_DIR")"
OUTPUT_DIR="${1:-$(pwd)}"

IMAGE_NAME="ipa-paper-benchmarks"
PLATFORM="${PLATFORM:-linux/amd64}"
ARTIFACT_DIR="$OUTPUT_DIR/ipa-paper-benchmarks"
ARTIFACT_ARCHIVE="$OUTPUT_DIR/ipa-paper-benchmarks.tar.gz"

echo "==> Building Docker image for $PLATFORM ..."
docker build --platform "$PLATFORM" -f "$BENCH_DIR/Dockerfile" -t "$IMAGE_NAME" "$REPO_ROOT"

# The Dockerfile hardcodes amd64 Go and AWS CLI downloads, so a build that
# silently resolved to another architecture produces a broken mixed-arch image
# rather than failing outright. Fail loudly instead.
EXPECTED_ARCH="${PLATFORM#linux/}"
ACTUAL_ARCH="$(docker image inspect "$IMAGE_NAME" --format '{{.Architecture}}')"
if [[ "$ACTUAL_ARCH" != "$EXPECTED_ARCH" ]]; then
    echo "error: built image is '$ACTUAL_ARCH' but '$EXPECTED_ARCH' was requested." >&2
    echo "       On an arm64 host, install QEMU/binfmt emulation:" >&2
    echo "         docker run --privileged --rm tonistiigi/binfmt --install amd64" >&2
    exit 1
fi

rm -rf "$ARTIFACT_DIR"
mkdir -p "$ARTIFACT_DIR"

echo "==> Exporting Docker image (this may take a few minutes) ..."
docker save "$IMAGE_NAME" | gzip > "$ARTIFACT_DIR/ipa-paper-benchmarks-image.tar.gz"

echo "==> Copying documentation and scripts ..."
cp "$BENCH_DIR/README.md" "$ARTIFACT_DIR/"
cp "$BENCH_DIR/REQUIREMENTS.md" "$ARTIFACT_DIR/"
cp "$BENCH_DIR/STATUS.md" "$ARTIFACT_DIR/"
cp "$BENCH_DIR/LICENSE.md" "$ARTIFACT_DIR/"
cp "$BENCH_DIR/scripts/deploy_realworld_app.sh" "$ARTIFACT_DIR/"

echo "==> Creating final archive ..."
tar -czf "$ARTIFACT_ARCHIVE" -C "$OUTPUT_DIR" ipa-paper-benchmarks/

rm -rf "$ARTIFACT_DIR"

SIZE=$(du -h "$ARTIFACT_ARCHIVE" | cut -f1)
echo
echo "Done: $ARTIFACT_ARCHIVE ($SIZE)"
echo "Upload this file to Zenodo."
