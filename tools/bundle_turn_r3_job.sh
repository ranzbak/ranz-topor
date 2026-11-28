#!/usr/bin/env bash
# bundle-turn r1 (5) verification (lead 2026-11-27: one timing hour): det s0 C1, A1 on (defaults), on one ccx33 timing
# slot (4 threads): qm (2 threads) beside esp -> wiccon -> eprom (2 threads, one after the other). BOARDS=qm runs qm alone.
set -u
OUT=$HZ_RUN_DIR
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb; EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
unit() { local n=$1 p=$2; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$n" "$p" --only 0 --no-png --threads 2 $C1 > "$OUT/$n.log" 2>&1; echo "$n rc $?" >> "$OUT/rc.txt"; }
unit qm $QM &
if [ "${BOARDS:-all}" != qm ]; then (unit esp $ESP; unit wic $WIC; unit eprom $EP) & fi
wait
echo "all done" >> "$OUT/rc.txt"
