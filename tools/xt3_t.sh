#!/usr/bin/env bash
# exist-term r3: run named topo_exist_term tests (Hetzner, one hzq unit): xt3_t.sh <filter>...
set -u
R=${HZ_RUN_DIR:-/tmp/xt3t}; mkdir -p "$R"
cd engine
for f in "$@"; do
  hzq run --threads 2 --name "xt3t-$f" -- cargo test --release --test topo_exist_term -- "$f" --nocapture > "$R/t-$f.log" 2>&1 9>&-; echo "$f rc $?" >> "$R/rc.txt"
done
