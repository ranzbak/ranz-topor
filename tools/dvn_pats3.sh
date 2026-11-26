#!/usr/bin/env bash
# dvn_pats3.sh OUT (branch draw-via-neck r3): the two PROPOSED patterns for the A4/B2 survivors, each routed through the plugin
# flow (defaults, runs=1, seed 0) on this build and with its mechanism off (debug arm TOPO_FD45_OFF), + PNG/summary.
set -u
R=${1:?out dir}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
PY=${TOPOR_VENV:-$ROOT/.venv}/bin/python
note() { echo "$*" >> $R/rc.txt; }
mkdir -p $R
$PY - "$R" <<'PY' > $R/gen.log 2>&1
import sys
from harness.dvn_patterns import generate
for n in ("neck_between_signals", "via_outside_full_interval"):
    print(n, generate(n, sys.argv[1] + "/boards", drc=True))
PY
note "gen rc $?"
declare -A ARM=([neck_between_signals]=dvn_growth [via_outside_full_interval]=none)
for n in neck_between_signals via_outside_full_interval; do
  TOPOR_UNRELEASED_SETTINGS=1 TOPO_VIADBG=1 TOPO_NECKDBG=1 tools/route_board.sh $R/boards/$n/problem/$n.kicad_pcb $R/runs/D/$n/$n/s0 \
    --set optimize.runs=1 > $R/D-$n.txt 2>&1 9>&-; note "D $n rc $?"
  TOPOR_UNRELEASED_SETTINGS=1 TOPO_VIADBG=1 TOPO_NECKDBG=1 TOPO_FD45_OFF=${ARM[$n]} tools/route_board.sh $R/boards/$n/problem/$n.kicad_pcb \
    $R/runs/X/$n/$n/s0 --set optimize.runs=1 > $R/X-$n.txt 2>&1 9>&-; note "X $n rc $?"
done
echo '{"D": "draw-via-neck r3 (defaults)", "X": "mechanism off (debug arm: B2 dvn_growth; via_outside_full_interval: none since r4, = D)"}' > $R/runs/labels.json
$PY -m harness.dvn_patterns_png $R/runs $R/png neck_between_signals via_outside_full_interval > $R/png.log 2>&1; note "png rc $?"
note "all done"
