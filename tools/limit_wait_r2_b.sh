#!/usr/bin/env bash
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
u dbg 4 bash tools/limit_wait_cargo.sh "$R" dbg
u m2 4 bash -c 'CARGO_TARGET_DIR=/root/lw2-rel cargo test --release --manifest-path engine/Cargo.toml --test topo_m2_draw_once 2>&1 | grep -E "^test |test result"'
u mutA 4 bash tools/limit_wait_mutants.sh "$R" scope chk1 chk2 chk3
u mutB 4 bash tools/limit_wait_mutants.sh "$R" due loopmsg latch
wait
echo "all done" >> "$R/rc.txt"
