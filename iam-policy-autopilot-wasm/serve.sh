#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PORT="${1:-8080}"

# --- prerequisites -----------------------------------------------------------
if ! command -v wasm-pack &>/dev/null; then
  echo "Installing wasm-pack…"
  cargo install wasm-pack
fi

# --- build for wasm32-unknown-unknown (browser target) -----------------------
echo "Building WASM with wasm-pack…"
wasm-pack build "$SCRIPT_DIR" \
  --target web \
  --out-dir "$SCRIPT_DIR/pkg" \
  --release \
  --no-typescript

echo "Build complete — pkg/ ready."
ls -lh "$SCRIPT_DIR/pkg/"

# --- serve -------------------------------------------------------------------
echo ""
echo "Serving at http://localhost:$PORT"
echo "Open http://localhost:$PORT/index.html"
echo ""
python3 -m http.server "$PORT" -d "$SCRIPT_DIR"
