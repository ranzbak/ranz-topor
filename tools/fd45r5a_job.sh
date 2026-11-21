#!/usr/bin/env bash
# first-draw-45 r5a (TEMP job script): Step A' blame at the rebased tip. qmtech det s0 tip + P2-off with the
# copper-neutral probes (OCTINODES 478,479, VIADBG incl. FORCEVIASITES, FORCEDBG); esp det s0 tip, lane-off, p5-off,
# p2-off. 6 units x 4 threads, at most 4 running/waiting at once (the 2 last start when one ends).
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() { # name board off dbg
  if [ -n "$4" ]; then export TOPO_OCTINODES=478,479 TOPO_VIADBG=1 TOPO_FORCEDBG=1; fi
  TOPO_FD45_OFF="$3" python -m harness.score_runs "$OUT/$1" "$2" --only 0 --no-png --threads 4 $S > "$OUT/$1.log" 2>&1
  echo "$1 rc $?" >> "$OUT/rc.txt"
  mkdir -p /root/fd45-work-aside/r5a-$1 && mv "$OUT/$1"/*/s0/work /root/fd45-work-aside/r5a-$1/ 2>/dev/null
}
export -f unit; export OUT S QM ESP
hzq run --threads 4 --name r5a-qtip -- bash -c 'unit qtip $QM "" 1' 9>&- &
hzq run --threads 4 --name r5a-qp2 -- bash -c 'unit qp2 $QM p2 1' 9>&- &
hzq run --threads 4 --name r5a-etip -- bash -c 'unit etip $ESP "" ""' 9>&- &
hzq run --threads 4 --name r5a-elane -- bash -c 'unit elane $ESP lane ""' 9>&- &
wait -n
hzq run --threads 4 --name r5a-ep5 -- bash -c 'unit ep5 $ESP p5 ""' 9>&- &
wait -n
hzq run --threads 4 --name r5a-ep2 -- bash -c 'unit ep2 $ESP p2 ""' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
