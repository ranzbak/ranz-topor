#!/usr/bin/env bash
# stitch-vias round 5 (deliberate placement): det s0 eprom/wiccon/esp/qmtech + the PROPOSED pattern, one hzq unit
# each (4 threads), arms run one after another inside the unit:
#   g   = defaults + [drawing] grid_mm = 0.254 (as r4's g arm, sv4-gonly2 = the before at c513c70c)
#   ng  = defaults, no grid (wiccon only; before = sv4/ng at 6c0ea5b0)
#   off = stitching as before stitch-vias (one via per piece, no region classes), no grid: compared by copper hash
#         to main a3bb159a (hz-runs/vgs7/off) and c513c70c (hz-runs/vfysv/off1) afterwards
#   gn  = g + [planes] stitch_nudge_mm = 0.5 (the lead's arm: what dropping blocked lattice points costs)
# Each routed file is measured after KiCad's own refill (tools/stitchvias_measure.py). After the units: the PNGs
# (r5 before/after, rendered here; the before inputs are in r5-before/ of this temporary rev only).
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
OFF="--set planes.stitch_dropped=false --set planes.stitch_thin_mm=0 --set planes.stitch_min_vias=1 --set planes.stitch_per_area=false"
G="--set drawing.grid_mm=0.254"
N="--set planes.stitch_nudge_mm=0.5"
arm() { # unit arm board-pcb extra-sets...
  local u=$1 a=$2 pcb=$3; shift 3
  python -m harness.score_runs "$OUT/$a" "$pcb" --only 0 --no-png --threads 4 $S "$@" > "$OUT/$u-$a.log" 2>&1
  local rc=$?
  local stem; stem=$(basename "$pcb" .kicad_pcb)
  local routed=$OUT/$a/$stem/s0/$stem-topor/$stem.kicad_pcb
  python -m tools.stitchvias_measure "$routed" --out "$OUT/$a/$stem-measure.json" --work "$OUT/$a/$stem-measure-work" >> "$OUT/$u-$a.log" 2>&1
  echo "$u $a rc $rc measure $?" >> "$OUT/rc.txt"
}
unit() { # name board-pcb arms...
  local u=$1 pcb=$2; shift 2
  for a in "$@"; do
    case $a in
      g) arm "$u" g "$pcb" $G ;;
      ng) arm "$u" ng "$pcb" ;;
      off) arm "$u" off "$pcb" $OFF ;;
      gn) arm "$u" gn "$pcb" $G $N ;;
    esac
  done
}
export -f arm unit; export OUT S OFF G N
EP=$(ls scratch/boards/eprom-to-kernal/*/eprom-to-kernal.kicad_pcb | head -1)
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
PAT=r5-before/pattern-boards/stitch_pieces/problem/stitch_pieces.kicad_pcb
hzq run --threads 4 --name sv5-qm -- bash -c "unit qm $QM g off gn" 9>&- &
hzq run --threads 4 --name sv5-wic -- bash -c "unit wic $WIC g ng off gn" 9>&- &
hzq run --threads 4 --name sv5-esp -- bash -c "unit esp $ESP g off gn" 9>&- &
hzq run --threads 4 --name sv5-small -- bash -c "unit ep $EP g off gn; unit pat $PAT g off" 9>&- &
wait
# PNGs: lattice overlay (5.08 x 5.08 staggered on the 10 mil grid, 5 x 5 without a grid), stitch vias ringed by kind
L5=5.08,5.08,2.54,0,0
L0=5,5,2.5,0,0
res() { ls "$1"/s0/reports/*/*/result.json 2>/dev/null | sort | tail -1; }  # res <arm dir>/<stem>
P=$OUT/png; mkdir -p "$P"
B=r5-before
python -m tools.stitchvias_png --out "$P/pattern-compare.png" \
  --panel "stitching off (min 1 via, no region classes), r5 tip" "$OUT/off/stitch_pieces-measure-work/stitch_pieces.kicad_pcb" "$(res $OUT/off/stitch_pieces)" "$B/pattern-boards/stitch_pieces/expected.json" "$L0" \
  --panel "PROPOSED r5 (awaiting approval): board lattice + piece corners, 10 mil grid" "$OUT/g/stitch_pieces-measure-work/stitch_pieces.kicad_pcb" "$(res $OUT/g/stitch_pieces)" "$B/pattern-boards/stitch_pieces/expected.json" "$L5" \
  > "$P/png.log" 2>&1
python -m tools.stitchvias_png --out "$P/wiccon-g-before-after.png" \
  --panel "before (r4 c513c70c): 10 mil grid, lattice 5.08 staggered" "$B/wic-g/2023.kicad_pcb" "$B/wic-g/result.json" - "$L5" \
  --panel "after (r5): board lattice + piece corners" "$OUT/g/2023-measure-work/2023.kicad_pcb" "$(res $OUT/g/2023)" - "$L5" >> "$P/png.log" 2>&1
python -m tools.stitchvias_png --out "$P/wiccon-ng-before-after.png" \
  --panel "before (r4 6c0ea5b0): no grid, lattice 5 staggered" "$B/wic-ng/2023.kicad_pcb" "$B/wic-ng/result.json" - "$L0" \
  --panel "after (r5): no grid, board lattice + piece corners" "$OUT/ng/2023-measure-work/2023.kicad_pcb" "$(res $OUT/ng/2023)" - "$L0" >> "$P/png.log" 2>&1
for b in 2023 esp-rust-board qmtech-minimig; do
  python -m tools.stitchvias_png --out "$P/$b-drop-vs-nudge.png" \
    --panel "r5 default: blocked lattice points dropped (stitch_nudge_mm 0)" "$OUT/g/$b-measure-work/$b.kicad_pcb" "$(res $OUT/g/$b)" - "$L5" \
    --panel "stitch_nudge_mm 0.5: blocked points moved along the grid" "$OUT/gn/$b-measure-work/$b.kicad_pcb" "$(res $OUT/gn/$b)" - "$L5" >> "$P/png.log" 2>&1
done
echo "png rc $?" >> "$OUT/rc.txt"
echo "all done" >> "$OUT/rc.txt"
