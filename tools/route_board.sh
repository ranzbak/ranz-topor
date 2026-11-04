#!/usr/bin/env bash
# Route one board headless through the plugin's run flow and write metrics + hand-vs-topor PNG:
#   tools/route_board.sh <board|pcb> <out_dir> [--timeout S] [--set run.key=value ...] [--whole-project] [--no-png]
# <board> is a harness.paths.BENCH_BOARDS key or a .kicad_pcb path (the source is only read, never written).
# Environment:
#   TOPOR_VENV          venv with pcbnew + topor_engine (default <repo>/.venv)
#   TOPOR_ENGINE_SITE   directory holding a `topor_engine` package that wins over the venv's (a wheel
#                       installed with `pip install --target DIR`); default: the venv's engine
#   TOPOR_TREE          code tree providing plugin/, harness/, toporlib/ (default <repo>); used to run the
#                       frozen classic reference from a worktree of main @ 64b3979
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV="${TOPOR_VENV:-$ROOT/.venv}"
TREE="${TOPOR_TREE:-$ROOT}"
export PYTHONPATH="${TOPOR_ENGINE_SITE:+$TOPOR_ENGINE_SITE:}$TREE:$TREE/plugin${PYTHONPATH:+:$PYTHONPATH}"
cd "$TREE"
exec "$VENV/bin/python" -P "$ROOT/harness/route_board.py" "$@"
