#!/usr/bin/env bash
# exist-term round 4 (Hetzner): targeted tests (topo_exist_term, topo_g0, the factor and exist_end unit tests, the
# config pytest), and the mutants whose earlier base failed.
set -u
R=${HZ_RUN_DIR:-/tmp/xt}; ROOT=$(pwd); mkdir -p "$R"
hzq run --threads 4 --name xt-tests -- bash -c "cd $ROOT/engine && cargo test --release --no-fail-fast --test topo_exist_term --test topo_g0 > $R/tests.log 2>&1; echo \"rc \$?\" >> $R/tests.log; cargo test --release --lib -- exist_end_maps shrinkage_follows >> $R/tests.log 2>&1; echo \"rc \$?\" >> $R/tests.log; cd $ROOT && CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/test_docs_links.py >> $R/tests.log 2>&1; echo \"pytest rc \$?\" >> $R/tests.log" &
sleep 1
hzq run --threads 4 --name xt-mutA -- bash tools/exist_term_mut.sh A dang_retain_off turn_cost_off kept_end_not_free &
wait
echo "all done" > "$R/rc.txt"
