#!/usr/bin/env bash
# phase sweep (eprom, esp crop; 10 variants) and the no-limit board runs of the tip
R=$1
cp tools/limit_wait_r3_sweep.rs engine/tests/lw3_sweep.rs
export CARGO_TARGET_DIR=/root/lw3-sw
cargo test --release --manifest-path engine/Cargo.toml --test lw3_sweep --no-run > "$R/sw-build.txt" 2>&1; echo "sw build rc $?"
VFY_FIX=eprom-to-kernal-ripped VFY_ROUNDS=2 VFY_N=30 VFY_OUT=$R/sweep-eprom.jsonl cargo test --release --manifest-path engine/Cargo.toml --test lw3_sweep -- --ignored --test-threads 1 2>&1 | grep -E "^test |test result|panicked"
VFY_FIX=esp-rust-board-usb-crop VFY_ROUNDS=2 VFY_N=20 VFY_OUT=$R/sweep-esp.jsonl cargo test --release --manifest-path engine/Cargo.toml --test lw3_sweep -- --ignored --test-threads 1 2>&1 | grep -E "^test |test result|panicked"
python3 tools/limit_wait_r3_check.py "$R"/sweep-*.jsonl --show 5 --json "$R/sweep-check.json"
rm -f engine/tests/lw3_sweep.rs
A="--set optimize.rounds=1 --set optimize.runs=1"
for b in eprom-to-kernal wiccon-2023; do python -m harness.score_runs "$R/tip/$b" $b --only 0 $A > "$R/tip-$b.log" 2>&1; echo "board $b rc $?"; done
python -m harness.score_runs "$R/tip/esp" scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb --only 0 $A > "$R/tip-esp.log" 2>&1; echo "board esp rc $?"
echo "sweep done"
