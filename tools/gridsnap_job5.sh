#!/usr/bin/env bash
# grid-snap r5/r6 (on main 2d54a276, first-draw-45) on the VM: det seed 0 (score_runs --only 0, rounds=1, runs=1),
# grid 0.254 mm. 2 hzq units: U1 off on esp/qmtech/eprom/wiccon; U2 prefer on the same 4 + the pattern grid_snap_bends
# (off + prefer, PNG). After each run: tools/gridsnap_measure.py on the routed copy.
#   tools/gridsnap_job5.sh            (the job)      tools/gridsnap_job5.sh unit <off|prefer>   (one unit)
set -u
R=${HZ_RUN_DIR:?}; ROOT=$PWD
note() { echo "$*" >> $R/rc.txt; }
A=(--no-png --set optimize.rounds=1 --set optimize.runs=1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
EP=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
board() {  # arm name pcb
  local arm=$1 name=$2 pcb=$3 g=0.254 S=()
  [ "$arm" != off ] && S=(--set "drawing.grid_snap=\"$arm\"" --set "drawing.grid_mm=$g")
  local t0=$(date +%s)
  python -m harness.score_runs $R/$arm/$name $pcb --only 0 --threads 4 "${A[@]}" "${S[@]}" > $R/$arm-$name.log 2>&1 9>&-
  note "$arm-$name rc $? wall $(( $(date +%s) - t0 )) s"
  local out=$(ls $R/$arm/$name/*/s0/*-topor/*.kicad_pcb 2>/dev/null | head -1)
  [ -n "$out" ] && PYTHONPATH=$ROOT python tools/gridsnap_measure.py "$out" --grid-mm $g --json $R/$arm-$name-grid.json >> $R/measure.log 2>&1 9>&-
}
if [ "${1:-}" = unit ]; then
  for b in "esp $ESP" "qmtech $QM" "eprom $EP" "wiccon $WIC"; do board $2 $b; done
  if [ $2 = prefer ]; then
    B=$R/pboards; P=$R/pruns
    python -m harness.drc_patterns $B grid_snap_bends --drc > $R/pattern.log 2>&1 9>&-
    for a in off prefer; do
      S=(); [ $a = prefer ] && S=(--set 'drawing.grid_snap="prefer"' --set drawing.grid_mm=0.254)
      mkdir -p $P/$a/grid_snap_bends
      TOPOR_HAND_LABEL=problem tools/route_board.sh $B/grid_snap_bends/problem/grid_snap_bends.kicad_pcb $P/$a/grid_snap_bends --keep-copper --no-png "${S[@]}" >> $R/pattern.log 2>&1 9>&-
      note "pattern $a rc $?"
      PYTHONPATH=$ROOT python tools/gridsnap_measure.py $P/$a/grid_snap_bends/grid_snap_bends-topor/grid_snap_bends.kicad_pcb --grid-mm 0.254 --json $R/pattern-$a-grid.json >> $R/measure.log 2>&1 9>&-
    done
    python -m harness.drc_patterns_png $P $R/png grid_snap_bends >> $R/pattern.log 2>&1 9>&-; note "png rc $?"
  fi
  exit 0
fi
hzq run --threads 4 --name gs6-off -- bash tools/gridsnap_job5.sh unit off &
sleep 2
hzq run --threads 4 --name gs6-prefer -- bash tools/gridsnap_job5.sh unit prefer &
wait
note "all done"
