#!/usr/bin/env bash
# draw-absorb A1 real settings (ccx33 timing slot): defaults + forced_shrink on, wall clock, timeout 600, as main's
# dvn7tq/dvn7tr (a94cde2b) ran them. Arg: qm | esp.
set -u
OUT=$HZ_RUN_DIR
export TOPOR_UNRELEASED_SETTINGS=1
case "$1" in
  qm) P=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; D=R-qm/qmtech-minimig/s0 ;;
  esp) P=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; D=R-esp/esp-rust-board/s0 ;;
esac
tools/route_board.sh $P $OUT/$D --clock wall --timeout 600 --set capacity.compass_cut_check=true --set drawing.forced_shrink=true --no-png > $OUT/R-$1.log 2>&1 9>&-
echo "R-$1 rc $?" >> $OUT/rc.txt
