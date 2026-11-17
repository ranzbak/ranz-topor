#!/usr/bin/env bash
R=$1; k=$2
if [ "$k" = rel ]; then export CARGO_TARGET_DIR=/root/lw2-rel; cargo test --release --manifest-path engine/Cargo.toml 2>&1 | tee "$R/cargo-rel.txt" | grep -E "^test result|FAILED|panicked" ;
else export CARGO_TARGET_DIR=/root/lw2-dbg; cargo test --no-fail-fast --manifest-path engine/Cargo.toml 2>&1 | tee "$R/cargo-dbg.txt" | grep -E "^test result|FAILED|panicked"; fi
echo "cargo $k rc ${PIPESTATUS[0]}"
