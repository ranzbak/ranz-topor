#!/usr/bin/env bash
# last-preview fix round 2: preview cost on the ccx33 timing slot (run: hz.sh job --timing -- bash tools/lastprev_r2_job.sh <interval> [reps])
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
I=${1:-2.0}; N=${2:-3}
hzq run --threads 4 --name lpr2-cost-$I -- env RAYON_NUM_THREADS=4 python -m harness.preview_cost $E $R/cost-$I --reps $N --interval $I > $R/cost-$I.log 2>&1 9>&-
echo "unit cost $I rc $?" >> $R/rc.txt
echo "all done" >> $R/rc.txt
