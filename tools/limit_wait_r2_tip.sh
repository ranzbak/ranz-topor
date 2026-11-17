#!/usr/bin/env bash
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
A="--set optimize.rounds=1 --set optimize.runs=1"
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
u rel 4 bash tools/limit_wait_cargo.sh "$R" rel
u dbg 4 bash tools/limit_wait_cargo.sh "$R" dbg
u pytest 4 bash tools/limit_wait_pytest.sh "$R"
u mutA 4 bash tools/limit_wait_mutants.sh "$R" scope chk1 chk2
u mutB 4 bash tools/limit_wait_mutants.sh "$R" chk3 due loopmsg latch
for b in eprom-to-kernal wiccon-2023; do u tip-$b 4 python -m harness.score_runs "$R/tip/$b" $b --only 0 $A; done
u tip-esp 4 python -m harness.score_runs "$R/tip/esp" $E --only 0 $A
wait
echo "all done" >> "$R/rc.txt"
