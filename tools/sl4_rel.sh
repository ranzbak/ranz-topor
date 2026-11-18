#!/usr/bin/env bash
R=$1
export CARGO_TARGET_DIR=/root/sl4-rel RUST_TEST_THREADS=4
cargo test --release --no-fail-fast --manifest-path engine/Cargo.toml --lib --test topo_stop_reason --test topo_mb0 \
  --test topo_m2_draw_once --test time_limit --test cancel_latency --test topo_d4 > "$R/cargo-rel.txt" 2>&1; echo "cargo rel rc $?"
grep -E "^test result" "$R/cargo-rel.txt"; grep -E "^test .*FAILED|^test result: FAILED" "$R/cargo-rel.txt"
