#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PORT="${1:-8081}"

echo "Serving at http://localhost:$PORT"
echo "Open http://localhost:$PORT/index.html"
echo ""
python3 -m http.server "$PORT" -d "$SCRIPT_DIR"
