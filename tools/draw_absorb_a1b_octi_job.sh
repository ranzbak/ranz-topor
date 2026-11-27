#!/usr/bin/env bash
# draw-absorb A1b octi-only (lead 2026-11-27: the re-anchor half dropped): det s0 C1 off / a1b on qmtech + esp with
# TOPO_A1BDBG (octi_wider sites for a crop fixture), + the A1b mutants.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab10}
C1="--set optimize.runs=1 --set optimize.rounds=1"
A1B="--set drawing.reserved_keep_neck=true"
export TOPOR_UNRELEASED_SETTINGS=1
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
mut() { local k=$1; shift; python3 tools/draw_absorb_mut2.py $OUT $k "$@" > $OUT/mut-$k.log 2>&1 9>&-; echo "mut-$k rc $?" >> "$OUT/rc.txt"; }
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
export -f unit mut; export OUT C1 A1B QM ESP
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-m -- bash -c 'CARGO_BUILD_JOBS=4 mut 1 a1b_no_meter a1b_no_grow a1b_no_fit a1b_octi_off a1b_octi_always_kept a1b_room_zero' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-qm -- bash -c 'TOPO_A1BDBG=1 unit off qm $QM $C1; TOPO_A1BDBG=1 unit a1b qm $QM $C1 $A1B' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-esp -- bash -c 'TOPO_A1BDBG=1 unit off esp $ESP $C1; TOPO_A1BDBG=1 unit a1b esp $ESP $C1 $A1B' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
