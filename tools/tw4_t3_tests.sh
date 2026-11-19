#!/usr/bin/env bash
# tw4 T3 merge trial (Hetzner; rev = the merge of tw4-eval with main): the full release suite, clippy (CI's
# `cargo clippy --release --all-targets`) and the FULL pytest (CI=1), two hzq units.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
note() { echo "$*" >> "$R/rc.txt"; }
( hzq run --threads 6 --name t3m-cargo -- bash -c "cd $ROOT/engine && RUST_TEST_THREADS=6 cargo test --release --no-fail-fast > $R/cargo.log 2>&1 9>&-; echo \"cargo rc \$?\" >> $R/rc.txt; cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; echo \"clippy rc \$?\" >> $R/rc.txt" ) & sleep 1
( hzq run --threads 4 --name t3m-py -- bash -c "cd $ROOT && CI=1 RAYON_NUM_THREADS=4 python -m pytest -q -rs -p no:cacheprovider > $R/pytest.log 2>&1 9>&-; echo \"pytest rc \$?\" >> $R/rc.txt" ) & sleep 1
wait; note "all done"
