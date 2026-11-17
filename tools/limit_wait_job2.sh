#!/usr/bin/env bash
# limit-wait VM job 2: engine tests (with timings) and the mutants, on the eprom-fixture integration test.
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
u cargo 4 bash -c 'export CARGO_TARGET_DIR=/root/lw-target2; cargo test --release --manifest-path engine/Cargo.toml --lib -- control:: optimise:: model:: && cargo test --release --manifest-path engine/Cargo.toml --test topo_limit_wait --test time_limit -- --report-time -Z unstable-options; cargo test --release --manifest-path engine/Cargo.toml --test topo_limit_wait --test time_limit'
u mutants 4 bash tools/limit_wait_mutants_all.sh inner_no_grace scope_ignored grace_ignored no_set_grace partial_offered no_loop_end no_hit default_off no_extra_stop
wait
echo "all done" >> "$R/rc.txt"
