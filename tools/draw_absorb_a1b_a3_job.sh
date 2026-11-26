#!/usr/bin/env bash
# draw-absorb A1b (reserved_keep_neck) + A3 (band_passes): det s0 one cycle (optimize.runs=1 rounds=1), 4 boards, arms:
#   off = defaults (copper hash must equal main a94cde2b; A1b meter unnecked_list; qmtech also TOPO_ERBS_DUMP for the Goal 3 RCA),
#   a1b = --set drawing.reserved_keep_neck=true, a3 = --set drawing.band_passes=3.
# 4 hzq units (4 threads each).
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab6}
C1="--set optimize.runs=1 --set optimize.rounds=1"
A1B="--set drawing.reserved_keep_neck=true"
A3="--set drawing.band_passes=3"
export TOPOR_UNRELEASED_SETTINGS=1
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit; export OUT C1 A1B A3 EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
mkdir -p $OUT/dump
hzq run --threads 4 --name $TAG-qoff -- bash -c 'TOPO_A1BDBG=1 TOPO_ERBS_DUMP=$OUT/dump unit off qm $QM $C1' 9>&- &
sleep 5
hzq run --threads 4 --name $TAG-qa1b -- bash -c 'unit a1b qm $QM $C1 $A1B' 9>&- &
sleep 5
hzq run --threads 4 --name $TAG-qa3 -- bash -c 'TOPO_A3DBG=1 unit a3 qm $QM $C1 $A3' 9>&- &
sleep 5
hzq run --threads 4 --name $TAG-small -- bash -c 'for a in off a1b a3; do case $a in off) X="";; a1b) X=$A1B;; a3) X=$A3;; esac; TOPO_A1BDBG=1 unit $a esp $ESP $C1 $X; unit $a wic $WIC $C1 $X; unit $a ep $EP $C1 $X; done' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
