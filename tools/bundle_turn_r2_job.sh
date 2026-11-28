#!/usr/bin/env bash
# bundle-turn r1 attribution arms (lead RESOURCE REVIEW 2026-11-27: 2 units): A = items 1-3 only (capacity.oblique_price=0),
# B = item 4 only (TOPO_FD45_OFF=fork,relaxloc,refit); det s0 C1, A1 on, qm + esp + wic.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-bt1r2}
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
unit() { local a=$1 n=$2 p=$3; shift 3; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads ${TH:-2} "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit; export OUT C1 QM ESP WIC
hzq run --threads 4 --name $TAG-qm -- bash -c 'unit A qm $QM $C1 --set capacity.oblique_price=0 & TOPO_FD45_OFF=fork,relaxloc,refit unit B qm $QM $C1 & wait' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-small -- bash -c 'TH=1; export TH; unit A esp $ESP $C1 --set capacity.oblique_price=0 & TOPO_FD45_OFF=fork,relaxloc,refit unit B esp $ESP $C1 & unit A wic $WIC $C1 --set capacity.oblique_price=0 & TOPO_FD45_OFF=fork,relaxloc,refit unit B wic $WIC $C1 & wait' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
