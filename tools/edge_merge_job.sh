#!/usr/bin/env bash
# edge_merge_job.sh tests|boards: edge-clear merge-prep (2026-11-17), hzq units, results under $HZ_RUN_DIR.
#   tests   cargo --release targeted (topo_pad_end topo_d2 topo_d4 topo_vias54), pytest tests/plugin + tests/test_docs_links.py
#   boards  eprom + wiccon, seq, seed 0, rounds=1 runs=1, no PNG
set -u
R=$HZ_RUN_DIR; ROOT=$(pwd); mkdir -p $R
note() { echo "$*" >> $R/rc.txt; }
A="--only 0 --no-png --set optimize.rounds=1 --set optimize.runs=1"
case ${1:-} in
  tests)
    ( hzq run --threads 4 --name mp-cargo -- bash -c "cd $ROOT/engine && CARGO_TARGET_DIR=$ROOT/target RUST_TEST_THREADS=4 cargo test --release --no-fail-fast --test topo_pad_end --test topo_d2 --test topo_d4 --test topo_vias54 > $R/cargo.log 2>&1; echo \"cargo rc \$?\" >> $R/rc.txt" ) &
    ( hzq run --threads 4 --name mp-py -- bash -c "cd $ROOT && CI=1 python -m pytest -q tests/plugin tests/test_docs_links.py > $R/pytest.log 2>&1; echo \"pytest rc \$?\" >> $R/rc.txt" ) & ;;
  boards)
    for b in eprom:eprom-to-kernal wiccon:wiccon-2023; do
      ( hzq run --threads 4 --name mp-${b%%:*} -- bash -c "cd $ROOT && python -m harness.score_runs $R/${b%%:*} ${b##*:} $A > $R/${b%%:*}.log 2>&1; echo \"run ${b%%:*} rc \$?\" >> $R/rc.txt" ) &
    done ;;
esac
wait; note "all done"
