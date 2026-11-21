#!/usr/bin/env bash
# grid-snap r3 on the VM: ONE hzq unit (4 threads), det seed 0, rounds=1, runs=1, grid 0.254 mm. Isolation of r2 item 1 on
# qmtech (grid snap off): full tip, without the forced-DP drill price (TOPOR_DIAG_NO_DRILL_PRICE), without the
# shorts/drill-first forced via key (TOPOR_DIAG_NO_SHORT_FIRST); then esp prefer and esp off (acceptance: 0 hole_clearance).
#   tools/gridsnap_job3.sh   (the job)      tools/gridsnap_job3.sh unit   (the unit)
set -u
R=${HZ_RUN_DIR:?}
note() { echo "$*" >> $R/rc.txt; }
A=(--no-png --set optimize.rounds=1 --set optimize.runs=1)
G=(--set 'drawing.grid_snap="prefer"' --set drawing.grid_mm=0.254)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
run() {  # arm pcb name [env...] -- [sets...]
  local arm=$1 pcb=$2 name=$3; shift 3; local t0=$(date +%s)
  env "$@" python -m harness.score_runs $R/$arm/$name $pcb --only 0 --threads 4 "${A[@]}" ${SETS:+"${SETS[@]}"} > $R/$arm-$name.log 2>&1 9>&-
  note "$arm-$name rc $? wall $(( $(date +%s) - t0 )) s"
}
if [ "${1:-}" = unit ]; then
  SETS=() run off $QM qmtech X=1
  SETS=() run nodrill $QM qmtech TOPOR_DIAG_NO_DRILL_PRICE=1
  SETS=() run noshort $QM qmtech TOPOR_DIAG_NO_SHORT_FIRST=1
  SETS=("${G[@]}") run prefer $ESP esp X=1
  SETS=() run off $ESP esp X=1
  exit 0
fi
hzq run --threads 4 --name gs3-iso -- bash tools/gridsnap_job3.sh unit
note "all done"
