#!/usr/bin/env bash
# Route every drc-patterns problem board with topor (local slot), one run per config (harness/drc_patterns.py CONFIGS):
#   [LABEL=main] [CFGS="default narrow"] [TOPOR_ROOT=tree] tools/drc_patterns_run.sh BOARDS RUNS ENGINE_SITE [name ...]
# RUNS/<LABEL->cfg/<name>/ = harness.route_board output (metrics, DRC json, routed copy); RUNS must be a new directory.
# TOPOR_ROOT = the source tree whose tools/route_board.sh and plugin run (default: this tree), ENGINE_SITE = its engine.
set -u
B="$1"; R="$2"; SITE="$3"; shift 3
ROOT="${TOPOR_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
VENV="${TOPOR_VENV:-$(cd "$(dirname "$0")/.." && pwd)/.venv}"
names=("$@"); [ ${#names[@]} -eq 0 ] && names=($(ls "$B"))
for n in "${names[@]}"; do
  for cfg in ${CFGS:-default narrow}; do
    sets=()
    [ "$cfg" = narrow ] && sets=(--set drawing.candidates=1 --set 'drawing.any_angle="forbidden"' --set 'drawing.acute="forbidden"')
    out="$R/${LABEL:+$LABEL-}$cfg/$n"; mkdir -p "$out"
    TOPOR_VENV="$VENV" TOPOR_ENGINE_SITE="$SITE" TOPOR_HAND_LABEL=problem \
      "$ROOT/tools/route_board.sh" "$B/$n/problem/$n.kicad_pcb" "$out" --keep-copper --no-png "${sets[@]}" > "$out/run.log" 2>&1
    echo "${LABEL:+$LABEL-}$cfg $n rc=$? $(grep -m1 'routing_new' "$out/run.log" | xargs)"
  done
done
echo "all done"
