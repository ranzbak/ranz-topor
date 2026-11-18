#!/usr/bin/env bash
# ndperf_job.sh (branch nd-perf, measurement): never-drop per-forced-wire profile (TOPO_NDPROF) and the
# drawing sub-phase times (TOPO_TIMES); bornhack at 4 threads, qmtech at 1 thread; rounds=1 runs=1.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
A="--no-png --set optimize.rounds=1 --set optimize.runs=1"
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
B=scratch/boards/bornhack2026/21c5f478f1af/bornhack2026-hardware.kicad_pcb
one() { local n=$1 th=$2 b=$3; shift 3
  mkdir -p $R/o/$n
  hzq run --threads $th --name nd-$n -- env TOPO_NDBB=${TOPO_NDBB:-0} RAYON_NUM_THREADS=$th TOPO_TIMES=1 TOPO_NDPROF=1 TOPO_FORCEDBG=1 tools/route_board.sh $b $R/o/$n --clock cpu --timeout 2400 $A "$@" > $R/$n.log 2>&1 9>&-
  note "$n rc $?"; }
if [ "${1:-}" = bb ]; then
  # prototype: TOPO_NDBB=1 lazy pricing + bound (timing, copper vs ndp-cd7315b), =2 every pick checked against eager
  TOPO_NDBB=1 one bornhack-bb 4 $B & sleep 2
  TOPO_NDBB=2 one bornhack-chk 4 $B & sleep 2
  TOPO_NDBB=1 one qmtech-bb 1 $Q & sleep 1
  TOPO_NDBB=2 one qmtech-chk 1 $Q & sleep 1
else
  one bornhack 4 $B & sleep 2
  one qmtech 1 $Q & sleep 1
fi
wait; note "all done"
