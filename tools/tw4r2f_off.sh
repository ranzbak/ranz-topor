#!/usr/bin/env bash
# tw4r2-fix: default copper at 1 cycle (K) on the 4 boards, for the hash compare tip vs main.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
hzq run --threads 4 --name t4r2f-off-$(git rev-parse --short HEAD) -- bash -c "for b in eprom wiccon esp qmtech; do bash $ROOT/tools/tw4r2_job.sh one K:\$b & done; wait" > $R/u-off.log 2>&1 9>&-
echo "unit off rc $?" >> $R/rc.txt
echo "all done" >> $R/rc.txt
