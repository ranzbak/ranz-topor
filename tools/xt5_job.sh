#!/usr/bin/env bash
# exist-term r5 (Hetzner): parallel hzq units: the targeted tests, the private badge (lead dump: KEPTLEAD / KEPTVIA),
# esp-rust-board seed 0 (XT5_ESP=0: skip it) (score_runs --only 0, rounds=1, runs=1). No bornhack (lead 2026-11-18).
set -u
R=${HZ_RUN_DIR:-/tmp/xt5}; mkdir -p "$R"
P=$(ls scratch/boards/xt-badge/*.kicad_pcb | head -1)
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
( hzq run --threads 4 --name xt5-badge -- bash -c "TOPO_KEPTDBG=1 tools/route_board.sh $P $R/badge --keep-copper --no-png --set optimize.rounds=1 > $R/badge.txt 2>&1; echo rc \$? >> $R/badge.txt" 9>&-; echo "badge rc $?" >> $R/rc.txt ) &
sleep 2
[ "${XT5_ESP:-1}" = 1 ] && ( hzq run --threads 2 --name xt5-esp -- python -m harness.score_runs $R/seq/esp $E --only 0 --no-png --set optimize.rounds=1 --set optimize.runs=1 > $R/esp.log 2>&1 9>&-; echo "esp rc $?" >> $R/rc.txt ) &
sleep 2
( cd engine && hzq run --threads 4 --name xt5-tests -- bash -c "cargo test --release --test topo_exist_term --test decompose_nets --test topo_d0 --test topo_s3b --test topo_r0 9>&- && cargo test --release --lib 9>&-" > $R/tests.log 2>&1 9>&-; echo "tests rc $?" >> $R/rc.txt ) &
wait; echo "all done" >> $R/rc.txt
