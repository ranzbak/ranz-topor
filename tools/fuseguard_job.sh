#!/usr/bin/env bash
# fuse-guard r1 ladder: det s0 esp + qmtech, base arm (TOPOR_FUSE_NO_FEET=1 = fuse-octi 2dccb353's cut-part check) and tip arm,
# 4 hzq units x 4 threads, no PNG.
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
unit() { # name arm board
  if [ "$2" = base ]; then export TOPOR_FUSE_NO_FEET=1; fi
  TOPO_FUSEDBG=1 python -m harness.score_runs "$OUT/$2" "$3" --only 0 --no-png --threads 4 $S > "$OUT/$1.log" 2>&1
  echo "$1 rc $?" >> "$OUT/rc.txt"
}
export -f unit; export OUT S
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
hzq run --threads 4 --name fg-qm-base -- bash -c "unit qm-base base $QM" 9>&- &
hzq run --threads 4 --name fg-qm-tip -- bash -c "unit qm-tip tip $QM" 9>&- &
hzq run --threads 4 --name fg-esp-base -- bash -c "unit esp-base base $ESP" 9>&- &
hzq run --threads 4 --name fg-esp-tip -- bash -c "unit esp-tip tip $ESP" 9>&- &
wait
echo "all done" >> $OUT/rc.txt
