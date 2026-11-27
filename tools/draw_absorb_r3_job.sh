#!/usr/bin/env bash
# draw-absorb fix round r3 (bulk, ccx43, 24 thread tokens over 4 units): the fixed merge (main acdee25b + A1 mid slots).
# real: real settings (wall clock, timeout 600, compass_cut_check) off + a1 on qm, then esp (4 thr each; VM at <= target);
# qm: det s0 C1(+ccc) off / a1 / a1b / a3 (2 thr each); small: esp arms (1 thr each) then wic / ep a1 (hash vs main);
# mut: the r2 mutants on the fixed merge (tools/draw_absorb_r3_mut.py, r2 list; the r3 lines ran in the local slot).
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab3}; W=/root/dab3-work; mkdir -p $W
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
A1="--set drawing.forced_shrink=true"; A1B="--set drawing.reserved_keep_neck=true"; A3="--set drawing.band_passes=3"
unit() { local a=$1 n=$2 p=$3; shift 3; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads ${TH:-2} "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
arms() { local n=$1 p=$2; unit off $n $p $C1 & unit a1 $n $p $C1 $A1 & unit a1b $n $p $C1 $A1B & unit a3 $n $p $C1 $A3 & wait; }
real() { local arm=$1 b=$2 p=$3 x=""; [ "$arm" = a1 ] && x="$A1"
  TOPOR_UNRELEASED_SETTINGS=1 RAYON_NUM_THREADS=4 tools/route_board.sh $p $OUT/R-$arm-$b/s0 --clock wall --timeout 600 --set capacity.compass_cut_check=true $x --no-png > $OUT/R-$arm-$b.log 2>&1 9>&-
  echo "R-$arm-$b rc $?" >> $OUT/rc.txt; }
mut() { CARGO_BUILD_JOBS=4 python3 tools/draw_absorb_r3_mut.py $W/mut mono_off stub_off merge_off mono_always_fwd mono_always_bwd mono_no_early_none ofb_off supply_all_nets supply_not_a1_only supply_flat_sev a3_no_swap > $OUT/mut.log 2>&1 9>&-; cp $W/mut/mut-r3.json $OUT/ 2>/dev/null; echo "mut rc $?" >> $OUT/rc.txt; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit arms real mut; export OUT W C1 A1 A1B A3 EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 8 --name $TAG-real -- bash -c 'real off qm $QM & real a1 qm $QM & wait; real off esp $ESP & real a1 esp $ESP & wait' 9>&- &
sleep 3
hzq run --threads 8 --name $TAG-qm -- bash -c 'arms qm $QM' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-small -- bash -c 'TH=1 arms esp $ESP; TH=2 unit a1 wic $WIC $C1 $A1 & TH=2 unit a1 ep $EP $C1 $A1 & wait' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-mut --pin -- bash -c 'mut' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
