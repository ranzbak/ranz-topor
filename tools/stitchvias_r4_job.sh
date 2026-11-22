#!/usr/bin/env bash
# stitch-vias round 4 (stitching vias on the user's KiCad grid): det s0 eprom/wiccon/esp/qmtech, one hzq unit per
# board (4 threads), three arms each, run one after another inside the unit:
#   g   = defaults + [drawing] grid_mm = 0.254 (10 mil, KiCad 10's current grid; grid_snap stays off)
#   ng  = defaults (headless: no KiCad settings dir, so no grid: "stitching vias: no grid found")
#   off = stitching as before stitch-vias (one via per cut piece, no region classes), no grid: compared to main
#         a3bb159a (hz-runs/vgs7/off) by the author afterwards
# Each routed file is measured after KiCad's own refill (tools/stitchvias_measure.py, --work keeps the refilled
# board for the grid share and the PNG).
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
OFF="--set planes.stitch_dropped=false --set planes.stitch_thin_mm=0 --set planes.stitch_min_vias=1 --set planes.stitch_per_area=false"
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
  arm "$1" ng "$2"
  arm "$1" off "$2" $OFF
}
export -f arm unit; export OUT S OFF G
EP=$(ls scratch/boards/eprom-to-kernal/*/eprom-to-kernal.kicad_pcb | head -1)
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
hzq run --threads 4 --name sv4-qm -- bash -c "unit qm $QM" 9>&- &
hzq run --threads 4 --name sv4-esp -- bash -c "unit esp $ESP" 9>&- &
hzq run --threads 4 --name sv4-wic -- bash -c "unit wic $WIC" 9>&- &
hzq run --threads 4 --name sv4-ep -- bash -c "unit ep $EP" 9>&- &
wait
echo "all done" >> "$OUT/rc.txt"
