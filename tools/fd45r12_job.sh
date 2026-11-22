#!/usr/bin/env bash
# first-draw-45 r12 (TEMP job script, removed before handback). One timing slot per job.
# mode time: esp then qm det s0 (4 threads, one unit) with wall + cpu; mode full: the same, then wiccon + eprom (copper check).
set -u
OUT=$HZ_RUN_DIR
TREE="${TOPOR_TREE:-$(pwd)}"
cd "$TREE"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
EP=$(ls scratch/boards/eprom*/*/*.kicad_pcb scratch/boards/eprom*/*.kicad_pcb 2>/dev/null | head -1)
note() { echo "$*" >> "$OUT/rc.txt"; }
unit() {
  local n=$1 b=$2; shift 2; local t0=$(date +%s.%N)
  python -m harness.score_runs "$OUT/$n" "$b" --only 0 --no-png --threads 4 $S "$@" > "$OUT/$n.log" 2>&1
  local rc=$?
  note "$n rc $rc wall $(echo "$(date +%s.%N) - $t0" | bc) s"
  for f in "$OUT/$n"/*/s0/*-topor/*.kicad_pcb; do [ -f "$f" ] && gzip -kf "$f"; done
}
run() { unit esp $ESP; unit qm $QM; [ "${1:-}" = full ] && { unit wic $WIC; unit ep $EP --set capacity.compass_cut_check=true; }; note "all done"; }
export -f unit note run; export OUT S QM ESP WIC EP TREE
hzq run --threads 4 --name "$HZ_TAG-t" -- bash -c "run ${1:-time}" 9>&-
