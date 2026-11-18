#!/usr/bin/env bash
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
A="--set optimize.rounds=1 --set optimize.runs=1"
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
for b in eprom-to-kernal wiccon-2023; do u main-$b 4 python -m harness.score_runs "$R/main/$b" $b --only 0 $A; done
u main-esp 4 python -m harness.score_runs "$R/main/esp" $E --only 0 $A
wait
echo "all done" >> "$R/rc.txt"
