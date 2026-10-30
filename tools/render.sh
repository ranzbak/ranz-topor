#!/usr/bin/env bash
# Render a KiCad board to PNG: tools/render.sh <pcb> <png> [--height N]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VENV="${TOPOR_VENV:-$ROOT/.venv}"
exec "$VENV/bin/python" -m harness.render "$@"
