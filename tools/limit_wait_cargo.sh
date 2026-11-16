#!/usr/bin/env bash
# limit-wait: engine tests (release lib filters + the integration tests), one hzq unit
set -u
export CARGO_TARGET_DIR=/root/lw-target
cargo test --release --manifest-path engine/Cargo.toml --lib -- control:: optimise:: model::
cargo test --release --manifest-path engine/Cargo.toml --test topo_limit_wait --test time_limit --test topo_m2_draw_once
