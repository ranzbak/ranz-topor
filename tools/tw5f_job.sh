#!/usr/bin/env bash
# tw5f_job.sh (branch tw5-fix): one hz.sh job: pure-default copper (4 boards, 1 cycle), targeted tests, mutants.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
( hzq run --pin --threads 4 --name tw5f-boards -- bash -c "bash $ROOT/tools/tw5_job.sh lane Dr1:eprom Dr1:wiccon Dr1:esp Dr1:qmtech:4" > $R/boards.log 2>&1; note "boards unit rc $?" ) & sleep 1
( hzq run --pin --threads 4 --name tw5f-tests -- bash -c "cd $ROOT/engine && cargo test --release --no-fail-fast --test tw5_escape --test loop_local -- --nocapture 9>&- > $R/cargo-int.log 2>&1; echo rc \$? >> $R/cargo-int.log; cargo test --release --lib -- escape:: 9>&- > $R/cargo-lib.log 2>&1; echo rc \$? >> $R/cargo-lib.log; rustup component add clippy >/dev/null 2>&1; cargo clippy --release --all-targets 9>&- > $R/clippy.log 2>&1; echo rc \$? >> $R/clippy.log" > $R/tests.log 2>&1; note "tests unit rc $?" ) & sleep 1
( hzq run --pin --threads 4 --name tw5f-mut -- bash $ROOT/tools/tw5f_mut.sh $R/mut > $R/mut.log 2>&1; cat $R/mut/mut.txt >> $R/rc.txt; note "mut unit rc $?" ) & sleep 1
wait; note "all done"
