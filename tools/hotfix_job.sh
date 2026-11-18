#!/usr/bin/env bash
# hotfix_job.sh (branch hotfix-main): acceptance of the two main fixes, tests only.
#   hotfix_job.sh all      build, then 4 hzq units: rep 1 / rep 2 / rep 4 + suite; wait
#   hotfix_job.sh rep R    20 x (lib test binary and topo_m6, each --test-threads 1 and default) at RAYON_NUM_THREADS=R
#   hotfix_job.sh suite    full release suite (--no-fail-fast) + clippy --release --all-targets
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT/engine
note() { echo "$*" >> $R/rc.txt; }
case ${1:-all} in
  all)
    cargo test --release --no-run --message-format=json 9>&- 2> $R/build.log > $R/build.json; note "build rc $?"
    # the test binaries, run directly by the rep units (a cargo test per run would wait for the suite's cargo lock)
    python3 -c 'import json,sys
for l in open(sys.argv[1]):
    try: m = json.loads(l)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] in ("topor_engine", "topo_m6") and m["profile"]["test"]:
        print(m["target"]["name"], m["executable"])' $R/build.json > $R/bins.txt; note "bins: $(cat $R/bins.txt | tr '\n' ' ')"
    for r in 1 2 4; do ( hzq run --threads 4 --name "rep$r" -- bash $ROOT/tools/hotfix_job.sh rep $r ) & sleep 1; done
    ( hzq run --threads 4 --name suite -- bash $ROOT/tools/hotfix_job.sh suite ) & sleep 1
    wait; note "all done" ;;
  rep)
    t=$2; fail=0
    for i in $(seq 1 20); do
      for tt in 1 default; do
        a=(); [ $tt = 1 ] && a=(--test-threads 1)
        for what in topor_engine topo_m6; do
          bin=$(awk -v n=$what '$1 == n {print $2}' $R/bins.txt | head -1)
          env -u RUST_TEST_THREADS RAYON_NUM_THREADS=$t CARGO_MANIFEST_DIR=$ROOT/engine $bin "${a[@]}" > $R/rep$t-cur.log 2>&1
          rc=$?
          if [ $rc -ne 0 ]; then fail=$((fail + 1)); cp $R/rep$t-cur.log $R/rep$t-fail-$i-$tt-$what.log; note "rep$t $i tt=$tt $what rc $rc"; fi
        done
      done
    done
    note "rep$t done: $fail failures in 80 runs" ;;
  suite)
    { RAYON_NUM_THREADS=4 cargo test --release --no-fail-fast 9>&- 2>&1; echo "cargo test rc $?"
      cargo clippy --release --all-targets 9>&- 2>&1; echo "clippy rc $?"
    } > $R/suite.log
    grep -E "^test result|rc [0-9]|FAILED|panicked|^(warning|error)(\[|:)" $R/suite.log | sort | uniq -c | while read -r l; do note "suite: $l"; done ;;
esac
