#!/usr/bin/env bash
# first-draw-45 r7a (TEMP): is the esp +3V3 staircase at B.Cu (149.5, 87.3) a forced run? One esp unit, det s0,
# default order (bends), TOPO_RUNPROV (prints every drawn run, "ladder" = first draw, "forced" = never drop; no
# copper change). Only the RUNPROV lines near the spot and the metrics/copper come back.
set -u
OUT=$HZ_RUN_DIR; TREE="${TOPOR_TREE:-/root/topor}"; cd "$TREE"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() { # name
  (cd "$TREE" && TOPO_RUNPROV=1 python -m harness.score_runs "$OUT/$1" "$ESP" --only 0 --no-png --threads 4 $S) > "$OUT/$1.log" 2>&1
  echo "$1 rc $?" >> "$OUT/rc.txt"
  L=$(ls "$OUT/$1"/*-s0.log | head -1)
  grep '^RUNPROV' "$L" | python3 -c '
import sys,re
for ln in sys.stdin:
    pts=[tuple(map(float,m)) for m in re.findall(r"\(([\d.-]+), ([\d.-]+)\)",ln)]
    if any(abs(x-149.5)<1.2 and abs(y-87.3)<1.6 for x,y in pts): print(ln.rstrip())
' > "$OUT/$1-runprov-spot.txt"
  grep -c '^RUNPROV' "$L" > "$OUT/$1-runprov-count.txt"
  mkdir -p /root/fd45-work-aside/$HZ_TAG-$1 && mv "$OUT/$1"/*/s0/work "$L" /root/fd45-work-aside/$HZ_TAG-$1/ 2>/dev/null
}
export -f unit; export OUT S ESP TREE
hzq run --threads 4 --name "$HZ_TAG-esp" -- bash -c 'unit esp' 9>&- &
wait
echo "all done" >> "$OUT/rc.txt"
