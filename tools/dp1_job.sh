#!/usr/bin/env bash
# dp1_job.sh (branch dp-onepoint): esp det seed 0 at real settings, with TOPO_DPDBG (logs every kept one-point run).
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
export TOPO_DPDBG=1
hzq run --threads 4 --name dp1-esp -- python -m harness.score_runs "$OUT/T-esp" "$ESP" --only 0 --no-png --threads 4 > "$OUT/T-esp.log" 2>&1 9>&-
echo "T-esp rc $?" >> $OUT/rc.txt
echo "all done" >> $OUT/rc.txt
