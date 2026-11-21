#!/usr/bin/env bash
# stitch-vias r1 ladder, one arm per job (ARM = base | tip, from the job tag): det s0 eprom/wiccon/esp/qmtech, 4 hzq units
# x 4 threads, no PNG; each board's routed file measured after KiCad's own refill (tools/stitchvias_measure.py); the eprom
# unit also routes the PROPOSED stitch pattern (harness/stitch_patterns.py) with its PNG.
set -u
OUT=$HZ_RUN_DIR
ARM=${ARM:-tip}
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
unit() { # name board-pcb
  python -m harness.score_runs "$OUT/$ARM" "$2" --only 0 --no-png --threads 4 $S > "$OUT/$1.log" 2>&1
  local rc=$?
  local stem; stem=$(basename "$2" .kicad_pcb)
  local pcb; pcb=$(ls "$OUT/$ARM/$stem/s0/$stem-topor/$stem.kicad_pcb" 2>/dev/null | head -1)
  [ -n "$pcb" ] && python -m tools.stitchvias_measure "$pcb" --out "$OUT/$ARM/$stem-measure.json" >> "$OUT/$1.log" 2>&1
  echo "$1 rc $rc measure $?" >> "$OUT/rc.txt"
}
pattern() {
  python -m harness.stitch_patterns "$OUT/pattern-boards" > "$OUT/pattern.log" 2>&1
  local P=$OUT/pattern-boards/stitch_pieces/problem/stitch_pieces.kicad_pcb
  TOPOR_HAND_LABEL=problem tools/route_board.sh "$P" "$OUT/$ARM-pattern" --keep-copper >> "$OUT/pattern.log" 2>&1
  local rc=$?
  local pcb; pcb=$(find "$OUT/$ARM-pattern" -name 'stitch_pieces.kicad_pcb' -path '*topor*' | head -1)
  [ -n "$pcb" ] && python -m tools.stitchvias_measure "$pcb" --out "$OUT/$ARM-pattern-measure.json" >> "$OUT/pattern.log" 2>&1
  echo "pattern rc $rc measure $?" >> "$OUT/rc.txt"
}
export -f unit pattern; export OUT S ARM
EP=$(ls scratch/boards/eprom-to-kernal/*/eprom-to-kernal.kicad_pcb | head -1)
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
hzq run --threads 4 --name sv-$ARM-qm -- bash -c "unit qm $QM" 9>&- &
hzq run --threads 4 --name sv-$ARM-esp -- bash -c "unit esp $ESP" 9>&- &
hzq run --threads 4 --name sv-$ARM-wic -- bash -c "unit wic $WIC" 9>&- &
hzq run --threads 4 --name sv-$ARM-ep -- bash -c "unit ep $EP; pattern" 9>&- &
wait
echo "all done" >> $OUT/rc.txt
