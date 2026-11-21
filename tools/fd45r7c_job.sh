#!/usr/bin/env bash
# first-draw-45 r7c (TEMP job script): [drawing] short_leg_mm (default 2.0) + chord nodes a jog leg apart.
# octi_order ladder det s0, bends and sweep x qmtech/esp/wiccon/eprom (8 units), the weight arm
# short_leg_mm = 1.0 (bends) on esp and qmtech (2 units), one unit of targeted tests. At most 4 units at once.
set -u
OUT=$HZ_RUN_DIR
TREE="${TOPOR_TREE:-/root/topor}"
cd "$TREE"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
EP=$(ls scratch/boards/eprom*/*/*.kicad_pcb scratch/boards/eprom*/*.kicad_pcb 2>/dev/null | head -1)
EPX="--set capacity.compass_cut_check=true"
unit() { # name board extra...
  local n=$1 b=$2; shift 2
  (cd "$TREE" && python -m harness.score_runs "$OUT/$n" "$b" --only 0 --no-png --threads 4 $S "$@") > "$OUT/$n.log" 2>&1
  echo "$n rc $?" >> "$OUT/rc.txt"
  mkdir -p /root/fd45-work-aside/$HZ_TAG-$n && mv "$OUT/$n"/*/s0/work "$OUT/$n"/*-s0.log /root/fd45-work-aside/$HZ_TAG-$n/ 2>/dev/null
}
tests() {
  cd "$TREE/engine"
  timeout 900 cargo test --release --test topo_first_draw_45 > "$OUT/t-fd45.log" 2>&1
  echo "t-fd45 rc $?" >> "$OUT/rc.txt"
  timeout 900 cargo test --release --lib -- short_leg_price straightening_keeps octilinear_keeps_the_lane octilinear_taut_piece sweep_relates pull_keeps own_taut_crossings cycle_break forced_octilinear octilinear_shape_cache style_price connectors_reach > "$OUT/t-lib.log" 2>&1
  echo "t-lib rc $?" >> "$OUT/rc.txt"
}
export -f unit tests; export OUT S QM ESP WIC EP EPX TREE
if [ "${1:-}" = tests ]; then
  hzq run --threads 4 --name "$HZ_TAG-t" -- bash -c 'tests; cd "$TREE/engine" && cargo test --release --lib short_leg_price -- --nocapture > "$OUT/t-short.log" 2>&1' 9>&-
  echo "all done" >> "$OUT/rc.txt"; exit 0
fi
q() { while [ "$(jobs -rp | wc -l)" -ge 4 ]; do wait -n; done; hzq run --threads 4 --name "$HZ_TAG-$1" -- bash -c "$2" 9>&- & }
q qmb 'unit qmb $QM --set drawing.octi_order=bends'; q espb 'unit espb $ESP --set drawing.octi_order=bends'
q qms 'unit qms $QM --set drawing.octi_order=sweep'; q esps 'unit esps $ESP --set drawing.octi_order=sweep'
q esp1 'unit esp1 $ESP --set drawing.octi_order=bends --set drawing.short_leg_mm=1.0'
q qm1 'unit qm1 $QM --set drawing.octi_order=bends --set drawing.short_leg_mm=1.0'
q wicb 'unit wicb $WIC --set drawing.octi_order=bends'; q wics 'unit wics $WIC --set drawing.octi_order=sweep'
q epb 'unit epb $EP $EPX --set drawing.octi_order=bends'; q eps 'unit eps $EP $EPX --set drawing.octi_order=sweep'
wait
echo "all done" >> "$OUT/rc.txt"
