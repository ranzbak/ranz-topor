#!/usr/bin/env bash
# full pytest for no-finished-notice (Python touched)
cd "$(pwd)"; CI=1 timeout 3000 python -m pytest -q tests > $HZ_RUN_DIR/pytest.log 2>&1 9>&-; echo "pytest rc $?" > $HZ_RUN_DIR/rc.txt; tail -5 $HZ_RUN_DIR/pytest.log >> $HZ_RUN_DIR/rc.txt; echo "all done" >> $HZ_RUN_DIR/rc.txt
