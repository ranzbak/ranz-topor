#!/usr/bin/env bash
# erbsm_merge_job.sh: erbs-margin merge-prep (2026-11-17), hzq units, results under $HZ_RUN_DIR.
#   eprom + wiccon seq, wiccon erbs; seed 0, optimize.rounds=1 runs=1, no PNG. Run on tip and on base 385e043 (+ this file).
set -u
R=$HZ_RUN_DIR; ROOT=$(pwd); mkdir -p $R
A="--only 0 --no-png --set optimize.rounds=1 --set optimize.runs=1"
u() { local n=$1; shift; ( hzq run --threads 4 --name mp-$n -- bash -c "cd $ROOT && python -m harness.score_runs $R/$n $1 $A $2 > $R/$n.log 2>&1; echo \"run $n rc \$?\" >> $R/rc.txt" ) & }
u eprom-seq eprom-to-kernal ""
u wiccon-seq wiccon-2023 ""
u wiccon-erbs wiccon-2023 "--set drawing.realise=erbs"
wait; echo "all done" >> $R/rc.txt
