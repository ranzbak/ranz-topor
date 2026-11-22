#!/usr/bin/env bash
# pad-pour-via VM job: det s0 eprom/wiccon/esp/qmtech, one hzq unit per board (4 threads), two arms each, run one
# after another inside the unit, with the stitch-vias r4 base settings (grid 0.254 = hz-runs/sv4-gonly2 arm g, the
# base: same settings at c513c70c):
#   g  = A + B (the estimate's KiCad 10 spoke rule + dog-bones; defaults)
#   ga = A only ([planes] stitch_dogbone = false): A's effect on pour completion during routing, apart from B
# The eprom unit (the smallest board) also routes the PROPOSED pattern (harness/pad_pour_patterns.py) and renders
# its PNG: 4 units in all.
# Each routed file is measured after KiCad's own refill (tools/stitchvias_measure.py).
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
G="--set drawing.grid_mm=0.254"
arm() { # unit arm board-pcb extra-sets...
  local u=$1 a=$2 pcb=$3; shift 3
  python -m harness.score_runs "$OUT/$a" "$pcb" --only 0 --no-png --threads 4 $S "$@" > "$OUT/$u-$a.log" 2>&1
  local rc=$?
  local stem; stem=$(basename "$pcb" .kicad_pcb)
  local routed=$OUT/$a/$stem/s0/$stem-topor/$stem.kicad_pcb
  python -m tools.stitchvias_measure "$routed" --out "$OUT/$a/$stem-measure.json" --work "$OUT/$a/$stem-measure-work" >> "$OUT/$u-$a.log" 2>&1
  echo "$u $a rc $rc measure $?" >> "$OUT/rc.txt"
}
unit() { # name board-pcb
  arm "$1" g "$2" $G
  arm "$1" ga "$2" $G --set planes.stitch_dogbone=false
}
pat() {
  python -m harness.pad_pour_patterns "$OUT/pat" > "$OUT/pat.log" 2>&1
  arm pat g "$OUT/pat/pad_dogbone/problem/pad_dogbone.kicad_pcb"
  local res; res=$(ls "$OUT"/g/pad_dogbone/s0/reports/*/*/result.json | tail -1)
  python -m tools.stitchvias_png --out "$OUT/pad_dogbone.png" \
    --panel "PROPOSED pad_dogbone: topor (KiCad refill)" "$OUT/g/pad_dogbone-measure-work/pad_dogbone.kicad_pcb" "$res" "$OUT/pat/pad_dogbone/expected.json" \
    --panel "PROPOSED pad_dogbone: expected" "$OUT/pat/pad_dogbone/reference/pad_dogbone.kicad_pcb" - "$OUT/pat/pad_dogbone/expected.json" >> "$OUT/pat.log" 2>&1
  echo "pat png $?" >> "$OUT/rc.txt"
}
export -f arm unit pat; export OUT S G
EP=$(ls scratch/boards/eprom-to-kernal/*/eprom-to-kernal.kicad_pcb | head -1)
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
hzq run --threads 4 --name ppv-qm -- bash -c "unit qm $QM" 9>&- &
hzq run --threads 4 --name ppv-esp -- bash -c "unit esp $ESP" 9>&- &
hzq run --threads 4 --name ppv-wic -- bash -c "unit wic $WIC" 9>&- &
hzq run --threads 4 --name ppv-ep -- bash -c "unit ep $EP; pat" 9>&- &
wait
echo "all done" >> "$OUT/rc.txt"
