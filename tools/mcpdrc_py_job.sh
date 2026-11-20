#!/usr/bin/env bash
# full pytest for mcp-drc (Python touched; gui tests deselected by pytest.ini)
cd "$(pwd)"; CI=1 timeout 3000 python -m pytest -q -rfEX tests > $HZ_RUN_DIR/pytest.log 2>&1 9>&-; echo "pytest rc $?" > $HZ_RUN_DIR/rc.txt; tail -8 $HZ_RUN_DIR/pytest.log >> $HZ_RUN_DIR/rc.txt; echo "all done" >> $HZ_RUN_DIR/rc.txt
