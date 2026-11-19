#!/usr/bin/env bash
# since-loud round 2: one full CI=1 pytest on the merged tip.
set -u
R=${HZ_RUN_DIR:?}; cd "$(dirname "$0")/.."
CI=1 PYTHONDONTWRITEBYTECODE=1 python3 -m pytest -q -p no:cacheprovider 9>&- > $R/pytest.log 2>&1; echo "pytest rc $?" >> $R/rc.txt
tail -3 $R/pytest.log >> $R/rc.txt; echo "all done" >> $R/rc.txt
