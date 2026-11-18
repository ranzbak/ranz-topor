#!/usr/bin/env bash
# exist-term merge check (Hetzner): exist_term_suite.sh cargo|pytest, one hzq unit, results under $HZ_RUN_DIR.
#   cargo   the CI engine suite (cargo test --release, every test)
#   pytest  the CI python suite (pytest -q against the job's engine build)
set -u
R=${HZ_RUN_DIR:-/tmp/xts}; ROOT=$(pwd); mkdir -p "$R"
case ${1:-} in
  cargo) hzq run --threads 4 --name xs-cargo -- bash -c "cd $ROOT/engine && RUST_TEST_THREADS=4 cargo test --release --no-fail-fast > $R/cargo.log 2>&1; echo \"cargo rc \$?\" >> $R/rc.txt" ;;
  pytest) hzq run --threads 4 --name xs-py -- bash -c "cd $ROOT && CI=1 RAYON_NUM_THREADS=4 python -m pytest -q -rs > $R/pytest.log 2>&1; echo \"pytest rc \$?\" >> $R/rc.txt" ;;
esac
echo "all done" >> "$R/rc.txt"
