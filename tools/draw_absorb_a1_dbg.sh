#!/usr/bin/env bash
# draw-absorb A1 round 2: forced_shrink on, det s0 one cycle, qmtech with TOPO_ABSORBDBG (gap-kept neck trace) + esp. 2 hzq units.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab2}
C1="--set optimize.runs=1 --set optimize.rounds=1"
ON="--set drawing.forced_shrink=true"
export TOPOR_UNRELEASED_SETTINGS=1
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
export -f unit; export OUT C1 ON QM ESP
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-qm -- bash -c 'TOPO_ABSORBDBG=1 unit on qm $QM $C1 $ON' 9>&- &
sleep 5
hzq run --threads 4 --name $TAG-esp -- bash -c 'unit on esp $ESP $C1 $ON' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
