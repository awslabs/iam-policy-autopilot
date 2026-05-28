#!/usr/bin/env bash
# Build script for the iam-policy-autopilot-wasm npm package.
#
# Stages:
#   1. Compile Rust policy engine to WASM (cargo build --target wasm32-unknown-unknown)
#   2. Generate browser-compatible JS glue via wasm-bindgen
#   3. Bundle TypeScript extraction layer with esbuild
#   4. Emit type declarations with tsc
#
# Output: npm/dist/ ready for npm publish.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NPM_DIR="$SCRIPT_DIR/npm"
DIST_DIR="$NPM_DIR/dist"
WASM_OUT_DIR="$DIST_DIR/wasm"

# --- Stage 1: Compile Rust to WASM -------------------------------------------
echo "=== Stage 1: Compiling Rust to WASM (release) ==="
cargo build \
  --package iam-policy-autopilot-wasm \
  --target wasm32-unknown-unknown \
  --release

WASM_FILE="$SCRIPT_DIR/../target/wasm32-unknown-unknown/release/iam_policy_autopilot_wasm.wasm"
if [ ! -f "$WASM_FILE" ]; then
  echo "ERROR: WASM binary not found at $WASM_FILE"
  exit 1
fi

# --- Stage 2: wasm-bindgen (JS glue + typed .wasm) ----------------------------
echo "=== Stage 2: Generating JS glue via wasm-bindgen ==="
mkdir -p "$WASM_OUT_DIR"

# Ensure wasm-bindgen-cli is available
if ! command -v wasm-bindgen &>/dev/null; then
  echo "Installing wasm-bindgen-cli..."
  cargo install wasm-bindgen-cli
fi

wasm-bindgen "$WASM_FILE" \
  --target web \
  --out-dir "$WASM_OUT_DIR" \
  --out-name iam_policy_autopilot_wasm

echo "  -> $WASM_OUT_DIR/"

# --- Stage 3: Bundle TypeScript extractors with esbuild -----------------------
echo "=== Stage 3: Bundling TypeScript with esbuild ==="

# Ensure npm deps are installed
if [ ! -d "$NPM_DIR/node_modules" ]; then
  echo "  Installing npm dependencies..."
  (cd "$NPM_DIR" && npm install)
fi

npx --prefix "$NPM_DIR" esbuild \
  "$NPM_DIR/src/index.ts" \
  "$NPM_DIR/src/extractor.ts" \
  --bundle \
  --format=esm \
  --outdir="$DIST_DIR" \
  --external:@ast-grep/wasm \
  --external:./wasm/* \
  --sourcemap

echo "  -> $DIST_DIR/index.js, extractor.js"

# --- Stage 4: Emit type declarations with tsc ---------------------------------
echo "=== Stage 4: Emitting type declarations ==="
npx --prefix "$NPM_DIR" tsc \
  --project "$NPM_DIR/tsconfig.json" \
  --emitDeclarationOnly

echo "  -> $DIST_DIR/*.d.ts"

# --- Done ---------------------------------------------------------------------
echo ""
echo "Build complete. Package ready at: $DIST_DIR/"
ls -lh "$WASM_OUT_DIR/iam_policy_autopilot_wasm_bg.wasm"
