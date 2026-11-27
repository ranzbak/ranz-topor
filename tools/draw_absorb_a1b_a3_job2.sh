#!/usr/bin/env bash
# draw-absorb A1b r2 (octi form only where main widens) + A3 r2 (partner yields after a failed try): det s0 C1,
# arms off / a1b / a3 on qmtech + esp (+ wiccon/eprom hash); mutants of A1b/A3 in 3 units (own tree + target each).
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab7}
C1="--set optimize.runs=1 --set optimize.rounds=1"
A1B="--set drawing.reserved_keep_neck=true"
A3="--set drawing.band_passes=3"
export TOPOR_UNRELEASED_SETTINGS=1
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
mut() { local k=$1; shift; python3 tools/draw_absorb_mut2.py $OUT $k "$@" > $OUT/mut-$k.log 2>&1 9>&-; echo "mut-$k rc $?" >> "$OUT/rc.txt"; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit mut; export OUT C1 A1B A3 EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-m1 --pin -- bash -c 'CARGO_BUILD_JOBS=4 mut 1 a1b_tip_off a1b_no_meter a1b_no_grow a1b_no_fit a1b_octi_off' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-m2 --pin -- bash -c 'CARGO_BUILD_JOBS=4 mut 2 a1b_octi_always_kept a1b_room_zero a3_off a3_accept_all a3_no_len_guard' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-m3 --pin -- bash -c 'CARGO_BUILD_JOBS=4 mut 3 a3_no_through_guard a3_side_flip a3_earlier_yields a3_no_swap a3_no_viol_price' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-qm -- bash -c 'unit off qm $QM $C1; unit a1b qm $QM $C1 $A1B' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-small -- bash -c 'TOPO_A3DBG=1 unit a3 qm $QM $C1 $A3; for a in off a1b a3; do case $a in off) X="";; a1b) X=$A1B;; a3) X=$A3;; esac; unit $a esp $ESP $C1 $X; unit $a wic $WIC $C1 $X; unit $a ep $EP $C1 $X; done' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
