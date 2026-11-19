#!/usr/bin/env bash
# tw7 mutants on the VM: one hzq unit (the mutants edit the tree in turn).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
hzq run --threads 4 --name tw7-mut -- python3 tools/tw7_mutate.py run $R/mut > $R/mut.log 2>&1 9>&-
echo "mut rc $?" >> $R/rc.txt; cat $R/mut/summary.txt >> $R/rc.txt; echo "all done" >> $R/rc.txt
