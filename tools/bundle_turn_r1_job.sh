#!/usr/bin/env bash
# bundle-turn r1 (bulk, ccx43): det s0 C1 on eprom/wiccon/esp/qm at the tip, A1 on (defaults) and off (forced_shrink=false),
# the same options as main's cached base (vfy-defaults-flip-a1 r1, rev 6d688bf = main 316e86ef engine): 2 hzq units.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-bt1}
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
OFF="--set drawing.forced_shrink=false"
unit() { local a=$1 n=$2 p=$3; shift 3; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads ${TH:-2} "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit; export OUT C1 OFF EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-qm -- bash -c 'unit tip qm $QM $C1 & unit off qm $QM $C1 $OFF & wait' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-small -- bash -c 'TH=1; export TH; unit tip esp $ESP $C1 & unit off esp $ESP $C1 $OFF & unit tip wic $WIC $C1 & unit off wic $WIC $C1 $OFF & wait; unit tip ep $EP $C1 & unit off ep $EP $C1 $OFF & wait' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
