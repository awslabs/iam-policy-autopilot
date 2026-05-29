#!/usr/bin/env bash
set -euo pipefail

# Build IAM Policy Autopilot WASM via Emscripten with JSPI
#
# Prerequisites:
#   - emsdk installed and activated (source ~/emsdk/emsdk_env.sh)
#   - rustup target add wasm32-unknown-emscripten

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"
BUILD_START=$SECONDS

mkdir -p "$DIST_DIR"

echo ""
echo "=== Step 1: Compile Rust to static library ==="
STEP_START=$SECONDS
# ERROR_ON_UNDEFINED_SYMBOLS=0 is needed because Rust's compiled objects reference
# Emscripten-provided symbols (libc, etc.) that aren't resolved until the link step.
EMCC_CFLAGS="-s ERROR_ON_UNDEFINED_SYMBOLS=0 --no-entry" \
  cargo build \
    --package iam-policy-autopilot-wasm \
    --target wasm32-unknown-emscripten \
    --release \
    --manifest-path "$PROJECT_ROOT/Cargo.toml"

STATIC_LIB="$PROJECT_ROOT/target/wasm32-unknown-emscripten/release/libiam_policy_autopilot_wasm.a"

if [ ! -f "$STATIC_LIB" ]; then
  echo "ERROR: Static library not found at $STATIC_LIB"
  exit 1
fi
echo "  Step 1 completed in $((SECONDS - STEP_START))s"

echo ""
echo "=== Step 2: Link with emcc (JSPI for async fetch) ==="
STEP_START=$SECONDS
# ERROR_ON_UNDEFINED_SYMBOLS=0 is also needed at link time because the em_fetch FFI
# functions are provided via --js-library and aren't visible to emcc's symbol checker.
emcc "$STATIC_LIB" \
  -o "$DIST_DIR/iam_policy_autopilot.js" \
  -s EXPORTED_FUNCTIONS='["_generate_policies_wasm","_free_string","_malloc","_free"]' \
  -s EXPORTED_RUNTIME_METHODS='["ccall","cwrap","UTF8ToString","stringToUTF8","lengthBytesUTF8"]' \
  -s MODULARIZE=1 \
  -s EXPORT_NAME="createModule" \
  -s ENVIRONMENT=web \
  -s ALLOW_MEMORY_GROWTH=1 \
  -s NO_EXIT_RUNTIME=1 \
  -s JSPI \
  -s 'JSPI_IMPORTS=["em_fetch_get_sync"]' \
  -s ERROR_ON_UNDEFINED_SYMBOLS=0 \
  --js-library "$SCRIPT_DIR/em_fetch.js" \
  -O3 \
  --no-entry

echo ""
echo "=== Step 2 completed in $((SECONDS - STEP_START))s ==="
echo "Output:"
ls -lh "$DIST_DIR/iam_policy_autopilot.js" "$DIST_DIR/iam_policy_autopilot.wasm"
echo ""

echo "=== Step 3: Build npm package ==="
NPM_DIR="$SCRIPT_DIR/npm"
NPM_DIST="$NPM_DIR/dist"
mkdir -p "$NPM_DIST"

# Copy WASM artifacts into npm dist
cp "$DIST_DIR/iam_policy_autopilot.js" "$NPM_DIST/"
cp "$DIST_DIR/iam_policy_autopilot.wasm" "$NPM_DIST/"

# Compile TypeScript wrapper
if [ ! -d "$NPM_DIR/node_modules" ]; then
  echo "  Installing npm dependencies..."
  npm install --prefix "$NPM_DIR" --silent
fi
npx --prefix "$NPM_DIR" tsc --project "$NPM_DIR/tsconfig.json"

echo ""
echo "=== npm package ready at $NPM_DIST ==="
ls -lh "$NPM_DIST"
echo ""
echo "=== Total build time: $((SECONDS - BUILD_START))s ==="
