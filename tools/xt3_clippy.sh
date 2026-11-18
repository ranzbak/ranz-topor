#!/usr/bin/env bash
# exist-term r3: clippy as CI (cargo clippy --release --all-targets, deny lints fail; warnings listed). The VM toolchain lacks the component: rustup adds it first.
set -u
R=${HZ_RUN_DIR:-/tmp/xt3c}; mkdir -p "$R"
rustup component add clippy > "$R/rustup.log" 2>&1
cd engine
hzq run --threads 4 --name xt3c -- cargo clippy --release --all-targets --keep-going > "$R/clippy.log" 2>&1 9>&-; echo "clippy rc $?" >> "$R/rc.txt"
