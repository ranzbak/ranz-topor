#!/usr/bin/env bash
# limit_flake_job.sh (branch limit-flake): 40 whole-binary runs of topo_m2_draw_once (RAYON 4 and 1),
# the release suite + clippy, then the mutant (old clock) which must fail the test.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT/engine
note() { echo "$*" >> $R/rc.txt; }
case ${1:-all} in
  all)
    cargo test --release --test topo_m2_draw_once --no-run --message-format=json 9>&- 2> $R/build.log > $R/build.json; note "build rc $?"
    python3 -c 'import json,sys
for l in open(sys.argv[1]):
    try: m = json.loads(l)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "topo_m2_draw_once" and m["profile"]["test"]:
        print(m["executable"])' $R/build.json > $R/bin.txt; note "bin: $(cat $R/bin.txt)"
    for r in 4 1; do ( hzq run --threads 4 --name "rep$r" -- bash $ROOT/tools/limit_flake_job.sh rep $r ) & sleep 1; done
    ( hzq run --threads 4 --name suite -- bash $ROOT/tools/limit_flake_job.sh suite ) & sleep 1
    wait
    # mutant: the old clock (r.runtime_s only)
    sed -i 's/ctl.limit.elapsed_s().max(r.runtime_s)/r.runtime_s/' src/topo/core.rs; touch src/topo/core.rs
    git diff --stat > $R/mutant.diff
    RAYON_NUM_THREADS=4 cargo test --release --test topo_m2_draw_once a_hard_limit_reached_in_the_via_reduction 9>&- > $R/mutant.log 2>&1; note "mutant rc $? (must be non-zero)"
    git checkout src/topo/core.rs
    note "all done" ;;
  rep)
    t=$2; fail=0; bin=$(head -1 $R/bin.txt)
    for i in $(seq 1 $((t == 4 ? 40 : 20))); do
      env -u RUST_TEST_THREADS RAYON_NUM_THREADS=$t CARGO_MANIFEST_DIR=$ROOT/engine $bin > $R/rep$t-cur.log 2>&1
      rc=$?
      if [ $rc -ne 0 ]; then fail=$((fail + 1)); cp $R/rep$t-cur.log $R/rep$t-fail-$i.log; note "rep$t $i rc $rc"; fi
    done
    note "rep$t done: $fail failures" ;;
  suite)
    { RAYON_NUM_THREADS=4 cargo test --release --no-fail-fast 9>&- 2>&1; echo "cargo test rc $?"
      cargo clippy --release --all-targets 9>&- 2>&1; echo "clippy rc $?"
    } > $R/suite.log
    grep -E "^test result|rc [0-9]|FAILED|panicked|^(warning|error)(\[|:)" $R/suite.log | sort | uniq -c | while read -r l; do note "suite: $l"; done ;;
esac
