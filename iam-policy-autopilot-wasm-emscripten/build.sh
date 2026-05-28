#!/usr/bin/env bash
set -euo pipefail

# Build IAM Policy Autopilot WASM via Emscripten with Asyncify
#
# Prerequisites:
#   - emsdk installed and activated (source ~/emsdk/emsdk_env.sh)
#   - rustup target add wasm32-unknown-emscripten

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"

mkdir -p "$DIST_DIR"

echo "=== Step 1: Compile Rust to static library ==="
EMCC_CFLAGS="-s ERROR_ON_UNDEFINED_SYMBOLS=0 --no-entry" \
  cargo build \
    --package iam-policy-autopilot-wasm-emscripten \
    --target wasm32-unknown-emscripten \
    --release \
    --manifest-path "$PROJECT_ROOT/Cargo.toml"

STATIC_LIB="$PROJECT_ROOT/target/wasm32-unknown-emscripten/release/libiam_policy_autopilot_wasm_emscripten.a"

if [ ! -f "$STATIC_LIB" ]; then
  echo "ERROR: Static library not found at $STATIC_LIB"
  exit 1
fi

echo ""
echo "=== Step 2: Link with emcc (JSPI for async fetch) ==="
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
echo "=== Build complete ==="
echo "Output:"
ls -lh "$DIST_DIR/iam_policy_autopilot.js" "$DIST_DIR/iam_policy_autopilot.wasm"
echo ""
echo "Serve with: $SCRIPT_DIR/serve.sh"
