#!/usr/bin/env bash
# exist-term r3 at the fix (Hetzner, hzq units): the board unit tests (lead_len, kept_polygons),
# every topo_exist_term test (clippy runs locally: the VM toolchain has none).
set -u
R=${HZ_RUN_DIR:-/tmp/xt3x}; mkdir -p "$R"
cd engine
run() { local n=$1; shift; hzq run --threads 4 --name "xt3x-$n" -- "$@" > "$R/$n.log" 2>&1 9>&-; echo "$n rc $?" >> "$R/rc.txt"; }
run lib cargo test --release --lib -- topo::board::tests topo::tidy::tests unsupported_ends exist_end_maps geom::prim --nocapture
run exist cargo test --release --test topo_exist_term -- --nocapture

