#!/usr/bin/env bash
# draw-absorb A1 (forced_shrink): det s0 one cycle (optimize.runs=1 rounds=1) on 4 boards, two arms:
#   off = defaults (copper hash must equal main a94cde2b, hz-runs/dvn7s/C1-*),
#   on  = --set drawing.forced_shrink=true (meter per-class counts vs off).
# 2 hzq units (4 threads each).
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab1}
C1="--set optimize.runs=1 --set optimize.rounds=1"
ON="--set drawing.forced_shrink=true"
export TOPOR_UNRELEASED_SETTINGS=1
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit; export OUT C1 ON EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-qm -- bash -c 'unit on qm $QM $C1 $ON; unit off qm $QM $C1' 9>&- &
sleep 5
hzq run --threads 4 --name $TAG-small -- bash -c 'unit on esp $ESP $C1 $ON; unit on wic $WIC $C1 $ON; unit on ep $EP $C1 $ON; unit off esp $ESP $C1; unit off wic $WIC $C1; unit off ep $EP $C1' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
