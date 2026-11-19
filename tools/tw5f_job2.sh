#!/usr/bin/env bash
# tw5f_job2.sh: round 2 of tw5-fix: the tw5_escape tests and the mutants f2b, l2.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
( hzq run --pin --threads 4 --name tw5f2-tests -- bash -c "cd $ROOT/engine && cargo test --release --no-fail-fast --test tw5_escape -- --nocapture 9>&- > $R/cargo-int.log 2>&1; echo rc \$? >> $R/cargo-int.log" > $R/tests.log 2>&1; note "tests unit rc $?" ) & sleep 1
( ONLY="f2b l2" hzq run --pin --threads 4 --name tw5f2-mut -- env ONLY="f2b l2" bash $ROOT/tools/tw5f_mut.sh $R/mut > $R/mut.log 2>&1; cat $R/mut/mut.txt >> $R/rc.txt; note "mut unit rc $?" ) & sleep 1
wait; note "all done"
