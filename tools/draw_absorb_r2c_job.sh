#!/usr/bin/env bash
# draw-absorb r2c (bulk, shared ccx43): det s0 C1 A1 arms with main's full-width form first for A1 bands.
# U1 qmtech a1 with TOPO_ABSORBDBG (debug lines only; copper = a1, checked in dab12); U2 esp/wic/ep a1.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab15}
C1="--set optimize.runs=1 --set optimize.rounds=1"
A1="--set drawing.forced_shrink=true"
unit() { local a=$1 n=$2 p=$3 th=$4; shift 4; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads $th "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit; export OUT C1 A1 EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-qm -- bash -c 'TOPO_ABSORBDBG=1 unit a1 qm $QM 4 $C1 $A1' 9>&- &
sleep 3
hzq run --threads 2 --name $TAG-small -- bash -c 'for b in esp:$ESP wic:$WIC ep:$EP; do unit a1 ${b%%:*} ${b#*:} 2 $C1 $A1; done' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
