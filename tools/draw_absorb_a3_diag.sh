#!/usr/bin/env bash
# draw-absorb A3 real-settings diagnosis (ccx33 timing slot, as dab9q): qmtech defaults + band_passes = 3, wall 600,
# TOPO_A3DBG (every try + every kept stretch) -> does a band pass move JS2_2 in the drawing the run returns?
set -u
OUT=$HZ_RUN_DIR
export TOPOR_UNRELEASED_SETTINGS=1 TOPO_A3DBG=1
python -m harness.score_runs $OUT/R-qm scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb --only 0 --no-png --clock wall --timeout 600 --threads 4 --set drawing.band_passes=3 > $OUT/R-qm.log 2>&1 9>&-
echo "R-qm rc $?" >> $OUT/rc.txt
