#!/usr/bin/env bash
# fx_units.sh <run dir>: the proposed survivor patterns (harness/fx_survivors.py): write the boards (reference DRC), route each in
# sequential and erbs mode (optimize.rounds=1, deterministic), keep the stderr (engine probes "FXP ..." when the rev has them), render the PNGs.
set -u
R=$1; if [ -z "${DISPLAY:-}" ]; then exec xvfb-run -a bash "$0" "$@"; fi; mkdir -p $R/runs $R/png
python -m harness.fx_survivors $R/boards --drc > $R/gen.log 2>&1
for n in edge_tangent collinear_row diag_exit via_fold qfn_two_ends edge_pads; do
  for m in seq erbs; do
    A="--set optimize.rounds=1"; [ $m = erbs ] && A="$A --set drawing.realise=erbs"
    tools/route_board.sh $R/boards/$n/problem/$n.kicad_pcb $R/runs/$n-$m --no-png --timeout 120 $A > $R/runs/$n-$m.out 2> $R/runs/$n-$m.err
    echo "$n $m rc $?" >> $R/rc.txt
  done
done
python -m harness.fx_survivors_png $R/boards $R/runs $R/png > $R/png.log 2>&1
echo "all done" >> $R/rc.txt
