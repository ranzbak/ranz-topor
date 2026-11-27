#!/usr/bin/env bash
# draw-absorb A0 (provenance meter): det s0 one cycle (optimize.runs=1 rounds=1, defaults) on 4 boards;
# copper hash vs main a94cde2b (hz-runs/dvn7s/C1-*), compass absorb block per board. 2 hzq units.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab0}
C1="--set optimize.runs=1 --set optimize.rounds=1"
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit; export OUT C1 EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-qm -- bash -c 'unit A0 qm $QM $C1' 9>&- &
sleep 5
hzq run --threads 4 --name $TAG-small -- bash -c 'unit A0 esp $ESP $C1; unit A0 wic $WIC $C1; unit A0 ep $EP $C1' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
