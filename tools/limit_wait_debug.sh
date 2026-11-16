#!/usr/bin/env bash
# limit-wait: debug engine suite once per branch
export CARGO_TARGET_DIR=/root/lw-target-dbg
cargo test --manifest-path engine/Cargo.toml
