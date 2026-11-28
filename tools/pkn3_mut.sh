#!/usr/bin/env bash
# pick-necks r3: each surviving mutant must now fail a targeted test. usage: pkn3_mut.sh <wt>/engine
set -u
E=$1; D=/home/paul/work/kicad/plugins/topor/scratch/agent-wt
export CARGO_TARGET_DIR=$D/wt-pkn-runs/target RUSTC_WRAPPER=/home/paul/.cargo/bin/sccache
T() { (cd $E && cargo test --release --lib -- necking round_delta 2>&1 | tail -3; cargo test --release --test topo_pick_necks 2>&1 | grep "^test result"); }
for m in ${MUTS:-m2_dec_tie m4_via_plan_model m5_fullw_no_pair m6_crit_necking_zero m8_round_delta m9_bests_json}; do
  python3 $D/wt-pkn/tools/pkn3_mut.py apply $m $E || continue
  out=$(T); echo "== $m"; echo "$out" | grep -E "FAILED|failed|test result" | head -3
  echo "$out" | grep -q "FAILED" && echo "$m KILLED" || echo "$m SURVIVED"
  python3 $D/wt-pkn/tools/pkn3_mut.py restore $E
done
