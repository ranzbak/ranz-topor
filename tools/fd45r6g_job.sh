#!/usr/bin/env bash
# first-draw-45 r6g (TEMP job script), rebased on main fa2d41bd: the octi_order re-judge, ladder det s0 on
# qmtech/esp/wiccon/eprom with octi_order = bends and = sweep (8 units), plus one unit with the U6/K5 tests
# under both orders and the P4 mutants (sweep order). At most 4 units waiting at once.
# usage: bash tools/fd45r6g_job.sh [boards|base]   (base = the fuse-guard base rev, no octi_order key)
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
  mkdir -p /root/fd45-work-aside/$HZ_TAG-$n && mv "$OUT/$n"/*/s0/work /root/fd45-work-aside/$HZ_TAG-$n/ 2>/dev/null
}
tests() {
  (
    cd "$TREE/engine"
    for o in sweep bends; do
      FD45_ORDER_TMP=$o FD45_DUMP="$OUT/u6-$o.json" timeout 600 cargo test --release --test topo_first_draw_45 -- --nocapture > "$OUT/fd45-$o.log" 2>&1
      echo "fd45-$o rc $?" >> "$OUT/rc.txt"
    done
    timeout 600 cargo test --release --lib -- octilinear_keeps_the_lane octilinear_taut_piece sweep_relates pull_keeps own_taut_crossings cycle_break forced_octilinear octilinear_shape_cache style_price connectors_reach > "$OUT/t-lib.log" 2>&1
    echo "t-lib rc $?" >> "$OUT/rc.txt"
  )
  FD45_ORDER_TMP=sweep python3 "$TREE/tools/fd45r6_mutants.py" "$TREE" "$OUT/mut" /root/fd45-mut-tree /root/fd45-mut-target > "$OUT/mut.log" 2>&1
  echo "mut rc $?" >> "$OUT/rc.txt"
}
export -f unit tests; export OUT S QM ESP WIC EP EPX TREE
q() { while [ "$(jobs -rp | wc -l)" -ge 4 ]; do wait -n; done; hzq run --threads 4 --name "$HZ_TAG-$1" -- bash -c "$2" 9>&- & }
if [ "${1:-boards}" = base ]; then
  q qm 'unit qm $QM'; q esp 'unit esp $ESP'; q wic 'unit wic $WIC'; q ep 'unit ep $EP $EPX'
else
  q t 'tests'
  q qmb 'unit qmb $QM --set drawing.octi_order=bends'; q qms 'unit qms $QM --set drawing.octi_order=sweep'
  q espb 'unit espb $ESP --set drawing.octi_order=bends'; q esps 'unit esps $ESP --set drawing.octi_order=sweep'
  q wicb 'unit wicb $WIC --set drawing.octi_order=bends'; q wics 'unit wics $WIC --set drawing.octi_order=sweep'
  q epb 'unit epb $EP $EPX --set drawing.octi_order=bends'; q eps 'unit eps $EP $EPX --set drawing.octi_order=sweep'
fi
wait
echo "all done" >> "$OUT/rc.txt"
