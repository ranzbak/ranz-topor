#!/usr/bin/env bash
# defaults-flip-a1 real settings (ccx33 timing slot, 4 threads): wall clock, timeout 600, as main's cached real runs
# (acdee25b = b59e745e copper: mrq2-real-acde qm, vabs3m esp; both 30 cycles).
#   defaults_flip_a1_real.sh ARM:BOARD ...   ARM = all (new defaults) | fs (forced_shrink only) | et (exit_taut only)
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $OUT/rc.txt; }
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
for ab in "$@"; do
  a=${ab%%:*}; b=${ab##*:}
  case $b in qm) p=$QM ;; esp) p=$ESP ;; esac
  case $a in all) s="" ;; fs) s="--set search.exit_taut=false" ;; et) s="--set drawing.forced_shrink=false" ;; esac
  TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs $OUT/R-$a-$b $p --only 0 --no-png --clock wall --timeout 600 --threads 4 $s > $OUT/R-$a-$b.log 2>&1 9>&-
  note "R-$a-$b rc $?"
done
note "all done"
