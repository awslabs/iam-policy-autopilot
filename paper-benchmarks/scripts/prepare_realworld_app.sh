#!/usr/bin/env bash
#
# Prepare the aws-genai-llm-chatbot app for the real-world benchmark:
#   1. Extract the pinned upstream tarball to real-world-apps/genai-chatbot-cdk/.
#   2. Overlay the deploy config: config.json -> bin/config.json, amplify/ -> amplify/.
#
# After this you still need to apply the build fixes and deploy — see
# real-world-apps/EVALUATION.md ("Build fixes required" and "Deployment").
# This script does NOT deploy anything to AWS.
#
# Usage:
#   ./paper-benchmarks/scripts/prepare_realworld_app.sh
#   FORCE=1 ./paper-benchmarks/scripts/prepare_realworld_app.sh   # re-extract over an existing dir
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
RWA_DIR="$BENCH_DIR/real-world-apps"

TARBALL="$RWA_DIR/genai-chatbot-cdk-50b6c6e.tar.gz"
EXTRACTED_TOP="aws-genai-llm-chatbot-50b6c6e"   # top-level dir inside the tarball
APP_DIR="$RWA_DIR/genai-chatbot-cdk"
CONFIG_SRC="$RWA_DIR/genai-chatbot-cdk-config"

if [[ ! -f "$TARBALL" ]]; then
  echo "error: pinned tarball not found: $TARBALL" >&2
  exit 1
fi

if [[ -d "$APP_DIR" ]]; then
  if [[ "${FORCE:-0}" == "1" ]]; then
    echo "==> Removing existing $APP_DIR (FORCE=1)"
    rm -rf "$APP_DIR"
  else
    echo "error: $APP_DIR already exists. Re-run with FORCE=1 to overwrite." >&2
    exit 1
  fi
fi

echo "==> Extracting $(basename "$TARBALL") ..."
tmp_extract="$(mktemp -d "${TMPDIR:-/tmp}/genai-chatbot.XXXXXX")"
trap 'rm -rf "$tmp_extract"' EXIT
tar xzf "$TARBALL" -C "$tmp_extract"
mv "$tmp_extract/$EXTRACTED_TOP" "$APP_DIR"

echo "==> Overlaying deploy config from $(basename "$CONFIG_SRC")/ ..."
# Upstream reads ./bin/config.json (see bin/config.ts); the amplify/ scaffolding
# lives at the app root.
cp "$CONFIG_SRC/config.json" "$APP_DIR/bin/config.json"
cp -r "$CONFIG_SRC/amplify" "$APP_DIR/amplify"

echo
echo "Prepared $APP_DIR"
echo "  - bin/config.json  (deploy configuration)"
echo "  - amplify/         (codegen scaffolding)"
echo
echo "Next: apply the build fixes and deploy — see real-world-apps/EVALUATION.md."
echo "  cd $APP_DIR"
echo "  npm install && npm run build"
echo "  npx cdk bootstrap   # once per account/region"
echo "  npx cdk deploy"
