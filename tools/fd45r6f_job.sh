#!/usr/bin/env bash
# first-draw-45 r6f (TEMP job script): P4 (chord nodes + fixpoint straightening) U6/K5 tests, then the octi_order
# re-judge: ladder det s0 on qmtech/esp/wiccon/eprom with octi_order = bends and = sweep (8 units, at most 4 at once).
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
EP=$(ls scratch/boards/eprom*/*/*.kicad_pcb scratch/boards/eprom*/*.kicad_pcb 2>/dev/null | head -1)
unit() { # name board order extra
  python -m harness.score_runs "$OUT/$1" "$2" --only 0 --no-png --threads 4 $S --set drawing.octi_order=$3 ${4:-} > "$OUT/$1.log" 2>&1
  echo "$1 rc $?" >> "$OUT/rc.txt"
  mkdir -p /root/fd45-work-aside/r6f-$1 && mv "$OUT/$1"/*/s0/work /root/fd45-work-aside/r6f-$1/ 2>/dev/null
}
tests() {
  cd engine
  for o in bends sweep; do
    FD45_ORDER_TMP=$o FD45_DUMP="$OUT/u6-$o.json" cargo test --release --test topo_first_draw_45 -- u6 --nocapture 9>&- > "$OUT/u6-$o.log" 2>&1
    echo "u6-$o rc $?" >> "$OUT/rc.txt"
  done
  cargo test --release --test topo_first_draw_45 -- k5 unknown --nocapture > "$OUT/k5.log" 2>&1; echo "k5 rc $?" >> "$OUT/rc.txt"
  cargo test --release --lib -- octilinear_keeps_the_lane octilinear_taut_piece sweep_relates pull_keeps own_taut_crossings cycle_break forced_octilinear octilinear_shape_cache style_price connectors_reach --nocapture > "$OUT/t-lib.log" 2>&1; echo "t-lib rc $?" >> "$OUT/rc.txt"
}
export -f unit tests; export OUT S QM ESP WIC EP
hzq run --threads 4 --name r6f-qmb -- bash -c 'unit qmb $QM bends' 9>&- &
hzq run --threads 4 --name r6f-qms -- bash -c 'unit qms $QM sweep' 9>&- &
hzq run --threads 4 --name r6f-espb -- bash -c 'unit espb $ESP bends' 9>&- &
hzq run --threads 4 --name r6f-esps -- bash -c 'unit esps $ESP sweep' 9>&- &
wait -n
hzq run --threads 4 --name r6f-small -- bash -c 'tests; unit wicb $WIC bends; unit wics $WIC sweep; unit epb $EP bends "--set capacity.compass_cut_check=true"; unit eps $EP sweep "--set capacity.compass_cut_check=true"' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
