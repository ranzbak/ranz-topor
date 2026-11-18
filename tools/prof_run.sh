#!/usr/bin/env bash
# TW0: one command = per-stage time table (+ cycles / re-lays per 600 s, + optional flame graphs) for one board run.
#   tools/prof_run.sh <board|pcb> [out_dir] [--flame] [route_board.sh options ...]
# <board> is a harness.paths.BENCH_BOARDS key or a .kicad_pcb path (read only). Default out_dir: scratch/prof/<board>.
# Runs tools/route_board.sh with TOPO_PROF=1 (engine counters on), optimize.rounds=1 optimize.runs=1 (the stage windows
# need one run), --no-png; then prints tools/prof_table.py and writes <out>/prof.json (sidecar "metrics" object) and
# <out>/prof.md. --flame also wraps the run in `perf record -k mono -F 499 -g` and writes flame-run.svg, flame-loop.svg and
# flame-stages.svg (root = stage) next to it; needs perf, rustfilt and inferno (cargo install inferno) on PATH, and an
# engine built with debug info + frame pointers (docs/dev/profiling.md). Extra options go to route_board.sh, e.g.
# `--timeout 600 --set optimize.seed=1`. Environment as route_board.sh (TOPOR_VENV, TOPOR_ENGINE_SITE).
# Timing numbers for decisions come from an exclusive ccx43 (hz.sh job --exclusive); locally it is a smoke test.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ $# -ge 1 ] || { sed -n 2,14p "$0"; exit 2; }
BOARD=$1; shift
OUT=""; FLAME=0; ARGS=()
if [ $# -ge 1 ] && [[ $1 != --* ]]; then OUT=$1; shift; fi
for a in "$@"; do if [ "$a" = --flame ]; then FLAME=1; else ARGS+=("$a"); fi; done
NAME=$(basename "${BOARD%.kicad_pcb}")
OUT=${OUT:-$ROOT/scratch/prof/$NAME}
mkdir -p "$OUT"
export TOPO_PROF=1
CMD=("$ROOT/tools/route_board.sh" "$BOARD" "$OUT/run" --no-png --set optimize.rounds=1 --set optimize.runs=1 "${ARGS[@]}")
if [ $FLAME = 1 ]; then
  for t in perf rustfilt inferno-flamegraph inferno-collapse-perf; do command -v $t >/dev/null || { echo "prof_run: --flame needs $t on PATH" >&2; exit 3; }; done
  perf record -k mono -F 499 -g -o "$OUT/perf.data" -- "${CMD[@]}" > "$OUT/run.log" 2>&1 || { tail -20 "$OUT/run.log"; exit 1; }
else
  "${CMD[@]}" > "$OUT/run.log" 2>&1 || { tail -20 "$OUT/run.log"; exit 1; }
fi
M=$(ls "$OUT"/run/*--metrics.json | head -1)
python3 "$ROOT/tools/prof_table.py" "$M" --name "$NAME" --json "$OUT/prof.json" | tee "$OUT/prof.md"
if [ $FLAME = 1 ]; then
  perf script -i "$OUT/perf.data" -F comm,tid,time,period,ip,sym 2>/dev/null | rustfilt | python3 "$ROOT/tools/prof_fold.py" "$M" > "$OUT/stacks-timed.txt"
  inferno-flamegraph --title "$NAME: whole run by stage (root = stage window)" < "$OUT/stacks-timed.txt" > "$OUT/flame-stages.svg"
  perf script -i "$OUT/perf.data" 2>/dev/null | rustfilt | inferno-collapse-perf > "$OUT/stacks.txt"
  inferno-flamegraph --title "$NAME: whole run" < "$OUT/stacks.txt" > "$OUT/flame-run.svg"
  grep 'topo::optimise::optimise' "$OUT/stacks.txt" | inferno-flamegraph --title "$NAME: TopoR loop only" > "$OUT/flame-loop.svg"
  gzip -f "$OUT/stacks.txt" "$OUT/stacks-timed.txt"; rm -f "$OUT/perf.data"
  echo "flame graphs: $OUT/flame-{stages,run,loop}.svg"
fi
echo "table: $OUT/prof.md  sidecar metrics: $OUT/prof.json  run: $OUT/run"
