#!/usr/bin/env bash
# first-draw-45 r6a (TEMP job script): P4 work round, U6 pinrow_long_bundle test runs (0.1 s each) per arm:
# order bends|sweep x straighten on|off (TOPO_FD45_OFF=p4s) x chord nodes on|off (p4); dumps for the PNG. 1 unit.
set -u
OUT=$HZ_RUN_DIR
cd "${TOPOR_TREE:-/root/topor}"
u6() {
  cd engine
  cargo test --release --test topo_first_draw_45 --no-run 9>&- > "$OUT/build.log" 2>&1; rustup component add clippy >/dev/null 2>&1; cargo clippy --release --all-targets 9>&- > "$OUT/clippy.log" 2>&1; echo "clippy rc $?" >> "$OUT/rc.txt"
  for o in bends sweep; do for off in "" p4s; do
    n="$o-${off:-on}"; n=${n//,/+}
    TOPO_FUSEDBG=1 TOPO_STRDBG=1 FD45_ORDER_TMP=$o TOPO_FD45_OFF="$off" FD45_DUMP="$OUT/u6-$n.json" cargo test --release --test topo_first_draw_45 -- u6 --nocapture > "$OUT/u6-$n.log" 2>&1
    echo "u6-$n rc $?" >> "$OUT/rc.txt"
  done; done
  cargo test --release --test topo_first_draw_45 -- k5 unknown --nocapture > "$OUT/k5.log" 2>&1; echo "k5 rc $?" >> "$OUT/rc.txt"
}
export -f u6; export OUT
hzq run --threads 4 --name r6a-u6 -- bash -c 'u6' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
