#!/usr/bin/env bash
# exist-term: the public ladder at the tip with the options of the main 385e043 bases of radii-m3 round j
# (tags m3j-base2, mp-new2: score_runs --only 0, optimize.rounds=1, runs=1, seed 0), for the copper hash (copper = main).
set -u
R=${HZ_RUN_DIR:-/tmp/xtl}
A="--no-png --set optimize.rounds=1 --set optimize.runs=1"
( hzq run --threads 4 --name xl-bh -- python -m harness.score_runs $R/seq/bornhack2026 bornhack2026 --only 0 $A > $R/seq-bornhack2026.log 2>&1; echo "seq-bornhack2026 rc $?" >> $R/rc.txt ) &
sleep 2
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
( hzq run --threads 4 --name xl-small -- bash -c "for b in eprom-to-kernal wiccon-2023 $Q $E; do n=\$(basename \$b .kicad_pcb); python -m harness.score_runs $R/seq/\$n \$b --only 0 $A > $R/seq-\$n.log 2>&1; echo \"seq-\$n rc \$?\" >> $R/rc.txt; done" ) &
wait; echo "all done" >> $R/rc.txt
