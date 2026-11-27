#!/usr/bin/env bash
# draw-absorb A3 real settings (ccx33 timing slot): defaults + band_passes = 3, wall clock, timeout 600, as main's
# dvn7tq/dvn7tr (a94cde2b) ran them (harness.score_runs --clock wall --timeout 600 --threads 4). Arg: qm | esp.
set -u
OUT=$HZ_RUN_DIR
export TOPOR_UNRELEASED_SETTINGS=1
case "$1" in
  qm) P=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb ;;
  esp) P=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb ;;
esac
python -m harness.score_runs $OUT/R-$1 $P --only 0 --no-png --clock wall --timeout 600 --threads 4 --set drawing.band_passes=3 > $OUT/R-$1.log 2>&1 9>&-
echo "R-$1 rc $?" >> $OUT/rc.txt
