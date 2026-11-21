#!/usr/bin/env bash
# grid-snap r2 on the VM (r1: tools/gridsnap_job.sh with clippy/tests; r2 adds none: local clippy + targeted tests) (run from the rev's tree root): 4 hzq units, det seed 0 (score_runs --only 0, rounds=1, runs=1),
# grid 0.254 mm (the user's KiCad User Grid, 10 mil; origin from each board), arms off / prefer / fallback:
#   U1 qmtech prefer; U2 qmtech off, then fallback; U3 esp x 3 arms + eprom off/prefer; U4 wiccon off/prefer,
#   the pattern grid_snap_bends (off + prefer) with its PNG. All arms on one VM: cpu off vs prefer comparable.
# After each board run: tools/gridsnap_measure.py on the routed copy (bends / vias on the grid, survivors by reason).
#   tools/gridsnap_job2.sh            (the job)      tools/gridsnap_job2.sh unit <kind> ...   (one unit)
set -u
R=${HZ_RUN_DIR:?}; ROOT=$PWD
note() { echo "$*" >> $R/rc.txt; }
A=(--no-png --set optimize.rounds=1 --set optimize.runs=1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
EP=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
board() {  # arm name pcb [grid_mm]
  local arm=$1 name=$2 pcb=$3 g=${4:-0.254} S=()
  [ "$arm" != off ] && S=(--set "drawing.grid_snap=\"${arm%%-*}\"" --set "drawing.grid_mm=$g")
  local t0=$(date +%s)
  python -m harness.score_runs $R/$arm/$name $pcb --only 0 --threads ${THR:-4} "${A[@]}" "${S[@]}" > $R/$arm-$name.log 2>&1 9>&-
  note "$arm-$name rc $? wall $(( $(date +%s) - t0 )) s"
  local out=$(ls $R/$arm/$name/*/s0/*-topor/*.kicad_pcb 2>/dev/null | head -1)
  [ -n "$out" ] && PYTHONPATH=$ROOT python tools/gridsnap_measure.py "$out" --grid-mm $g --json $R/$arm-$name-grid.json >> $R/measure.log 2>&1 9>&-
}
if [ "${1:-}" = unit ]; then
  case $2 in
    qm-prefer) board prefer qmtech $QM ;;
    qm-off-fb) board off qmtech $QM; board fallback qmtech $QM ;;
    esp-ep) for a in off prefer fallback; do board $a esp $ESP; done; for a in off prefer; do board $a eprom $EP; done ;;
    wic-rest)
      for a in off prefer; do board $a wiccon $WIC; done
      B=$R/pboards; P=$R/pruns
      python -m harness.drc_patterns $B grid_snap_bends --drc > $R/pattern.log 2>&1 9>&-
      for a in off prefer; do
        S=(); [ $a = prefer ] && S=(--set 'drawing.grid_snap="prefer"' --set drawing.grid_mm=0.254)
        mkdir -p $P/$a/grid_snap_bends
        TOPOR_HAND_LABEL=problem tools/route_board.sh $B/grid_snap_bends/problem/grid_snap_bends.kicad_pcb $P/$a/grid_snap_bends --keep-copper --no-png "${S[@]}" >> $R/pattern.log 2>&1 9>&-
        note "pattern $a rc $?"
      done
      python -m harness.drc_patterns_png $P $R/png grid_snap_bends >> $R/pattern.log 2>&1 9>&-; note "png rc $?" ;;
  esac
  exit 0
fi
hzq run --threads 4 --name gs2-qm-prefer -- bash tools/gridsnap_job2.sh unit qm-prefer &
sleep 2
hzq run --threads 4 --name gs2-qm-off-fb -- bash tools/gridsnap_job2.sh unit qm-off-fb &
sleep 2
hzq run --threads 4 --name gs2-esp-ep -- bash tools/gridsnap_job2.sh unit esp-ep &
sleep 2
THR=2 hzq run --threads 2 --name gs2-wic-rest -- env THR=2 bash tools/gridsnap_job2.sh unit wic-rest &
wait
note "all done"
