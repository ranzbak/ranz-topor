#!/usr/bin/env bash
# first-draw-45 r6h (TEMP): blame arms for the esp DRC rise of r6g (base 47 -> sweep 65 / bends 66), det s0, sweep order.
set -u
OUT=$HZ_RUN_DIR; TREE="${TOPOR_TREE:-/root/topor}"; cd "$TREE"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1 --set drawing.octi_order=sweep"
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() { # name off
  (cd "$TREE" && TOPO_FD45_OFF=$2 python -m harness.score_runs "$OUT/$1" "$ESP" --only 0 --no-png --threads 4 $S) > "$OUT/$1.log" 2>&1
  echo "$1 rc $?" >> "$OUT/rc.txt"
  mkdir -p /root/fd45-work-aside/$HZ_TAG-$1 && mv "$OUT/$1"/*/s0/work /root/fd45-work-aside/$HZ_TAG-$1/ 2>/dev/null
}
export -f unit; export OUT S ESP TREE
hzq run --threads 4 --name "$HZ_TAG-p4s" -- bash -c 'unit p4s p4s' 9>&- &
hzq run --threads 4 --name "$HZ_TAG-p4" -- bash -c 'unit p4 p4' 9>&- &
hzq run --threads 4 --name "$HZ_TAG-p4both" -- bash -c 'unit p4both p4,p4s' 9>&- &
hzq run --threads 4 --name "$HZ_TAG-allp" -- bash -c 'unit allp p1,p2,p5,lane,p4,p4s' 9>&- &
wait
echo "all done" >> "$OUT/rc.txt"
