#!/usr/bin/env bash
# Benchmark one board end-to-end: tools/bench.sh <board> <out_dir>
# <board> is a key of harness.paths.BENCH_BOARDS (e.g. wiccon-2023). TOPOR_VENV overrides the venv.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VENV="${TOPOR_VENV:-$ROOT/.venv}"
exec "$VENV/bin/python" -m harness.bench "$@"
