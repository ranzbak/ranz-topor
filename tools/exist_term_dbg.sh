#!/usr/bin/env bash
# exist-term debug (Hetzner, one hzq unit): the root-rule fixture with and without the src retain (mutant dang_retain_off)
# and without the dangling_turn price, TOPO_TREEDBG on: the cost of each source end.
set -u
R=${HZ_RUN_DIR:-/tmp/xtd}; ROOT=$(pwd); mkdir -p "$R"
W=/root/xtm/dbg; rm -rf "$W"; mkdir -p "$W"
rsync -a --exclude target --exclude .git --exclude scratch --exclude .venv ./ "$W"/
export CARGO_TARGET_DIR=/root/xtm-target-dbg CARGO_BUILD_JOBS=4
for m in base dang_retain_off turn_cost_off; do
  [ "$m" = base ] || python3 "$W"/tools/exist_term_mut.py "$m" apply "$W"
  ( cd "$W"/engine && TOPO_TREEDBG=1 cargo test --release --test topo_exist_term dangling_end_is_the_root -- --nocapture > "$R/dbg-$m.log" 2>&1 )
  [ "$m" = base ] || python3 "$W"/tools/exist_term_mut.py "$m" revert "$W"
done
echo "all done" > "$R/rc.txt"
