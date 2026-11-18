#!/usr/bin/env bash
# fp-clear VM job: bornhack seq run (KiCad DRC) + per-board check of the exported pad clearance + touched pytest files.
set -u
R=${HZ_RUN_DIR:-/root/runs/fpclear}; mkdir -p $R/b $R/exp
export RAYON_NUM_THREADS=4
( hzq run --threads 4 --name fpc-bornhack -- python -m harness.score_runs $R/b bornhack2026 --only 0 --no-png --threads 4 \
    --set optimize.rounds=1 --set optimize.runs=1 > $R/b.log 2>&1 ; echo "bornhack rc $?" >> $R/rc.txt ) &
( hzq run --threads 1 --name fpc-pytest -- env CI=1 python -m pytest -q tests/test_model_export.py tests/plugin/test_model_ipc.py \
    tests/plugin/test_ipc_parity.py tests/test_fixtures.py tests/test_custom_rules.py > $R/pytest.log 2>&1 ; echo "pytest rc $?" >> $R/rc.txt ) &
wait; echo "all done" >> $R/rc.txt
