#!/usr/bin/env bash
# exist-term r2: the public ladder (score_runs --only 0, optimize.rounds=1, runs=1, seed 0, no PNG) for the copper hash
# (tip copper = main bc42ab0). Same script for base (main bc42ab0) and tip. bornhack first (long unit).
set -u
R=${HZ_RUN_DIR:-/tmp/xtl}
A="--no-png --set optimize.rounds=1 --set optimize.runs=1"
( hzq run --threads 1 --name xl2-bh -- python -m harness.score_runs $R/seq/bornhack2026 bornhack2026 --only 0 $A > $R/seq-bornhack2026.log 2>&1; echo "seq-bornhack2026 rc $?" >> $R/rc.txt ) &
sleep 2
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
for b in eprom-to-kernal wiccon-2023 $Q $E; do
  n=$(basename $b .kicad_pcb)
  ( hzq run --threads 1 --name xl2-$n -- python -m harness.score_runs $R/seq/$n $b --only 0 $A > $R/seq-$n.log 2>&1; echo "seq-$n rc $?" >> $R/rc.txt ) &
done
wait; echo "all done" >> $R/rc.txt
