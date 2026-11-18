#!/usr/bin/env bash
# ndperf_r2.sh (branch nd-perf, docs/dev/design/nd-perf.md): phase 2 checks on the VM.
#   ndperf_r2.sh tip    hzq units: bornhack 4 t (timing + copper), bornhack 4 t with TOPO_FORCED_CHECK (every forced pick
#                       against the exhaustive one), eprom/wiccon/esp/qmtech 1 t with TOPO_FORCED_CHECK (copper + picks),
#                       the release suite + clippy + FULL pytest (CI=1), the F1/F2 mutants in their own tree
#   ndperf_r2.sh base   (run on main through base64) eprom/wiccon/esp/qmtech 1 t: the copper base, same options
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); [ -d $ROOT/engine ] || ROOT=$(pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
A="--no-png --set optimize.rounds=1 --set optimize.runs=1"
P=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
W=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
B=scratch/boards/bornhack2026/21c5f478f1af/bornhack2026-hardware.kicad_pcb
board() { local n=$1 th=$2 b=$3; shift 3
  mkdir -p $R/o/$n
  hzq run --threads $th --name nd2-$n -- env RAYON_NUM_THREADS=$th TOPO_TIMES=1 "$@" tools/route_board.sh $b $R/o/$n --clock cpu --timeout 2400 $A > $R/$n.log 2>&1 9>&-
  note "$n rc $?"; }
MT=""
mut() {  # name file(relative) test-args... ; the python snippet on stdin edits the file in the mutant tree
  local n=$1 f=$MT/$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  touch $f
  ( cd $MT/engine && cargo test --release "$@" > $R/mut-$n.log 2>&1 9>&- ); local rc=$?
  mv $f.orig $f; touch $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
sub() { cat <<EOF
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF
}
mutants() {
  MT=/root/mt/$(basename $R); mkdir -p $MT
  rsync -a --exclude target --exclude .git --exclude scratch $ROOT/ $MT/; find $MT/engine -name '*.rs' -exec touch {} +
  export CARGO_TARGET_DIR=$MT/target
  local D=engine/src/topo/draw.rs PB="--lib topo::draw::tests::pick_by_bound" LB="--lib topo::exact::tests::unprobed" RV="--lib via_reach"
  ( cd $MT/engine && cargo test --release --lib -- pick_by_bound unprobed via_reach > $R/mut-base.log 2>&1 9>&- ); note "mutant base (unmutated) rc $?: $(grep 'test result' $R/mut-base.log | tail -1)"
  sub 'Ordering::Equal if j > bi => continue,' 'Ordering::Equal => continue,' | mut b1_tie_skip $D $PB
  sub 'Ordering::Greater => break,' 'Ordering::Greater => {}' | mut b2_no_prune $D $PB
  sub '    order.sort_by(|&a, &b| forced_key_cmp(&lbs[a], &lbs[b]).then(a.cmp(&b)));' '' | mut b3_no_sort $D $PB
  sub 'Ordering::Equal => j < bi,' 'Ordering::Equal => false,' | mut b4_tie_best $D $PB
  sub 'len_nm: tw.min(len.max(tw)), against: name(&v.against), via: false });' 'len_nm: len.max(tw), against: name(&v.against), via: false });' | mut l1_len_not_floor $D $LB
  sub 'depth_nm: depth, len_nm: tw.min(len.max(tw))' 'depth_nm: depth + 1.0, len_nm: tw.min(len.max(tw))' | mut l2_depth $D $LB
  sub '            let mut vs = self.viols_of(&c.2, &rep);
            vs.append(&mut c.3);' '            let mut vs = std::mem::take(&mut c.3);
            vs.extend(self.viols_of(&c.2, &rep));' | mut p1_graze_order $D $LB
  sub '    opts.via_reach_price || std::env::var_os("TOPO_VIADBG").is_some()' '    std::env::var_os("TOPO_VIADBG").is_some()' | mut r1_priced_off $D $RV
  sub '            if reach_priced(self.opts) {' '            if false {' | mut r2_site_skip $D $RV
  sub '            if reach_priced(self.opts) {' '            if true {' | mut r3_site_always $D $RV
}
suite() {
  cd $ROOT/engine
  { timeout 5400 cargo test --release --no-fail-fast 2>&1 9>&-; echo "cargo test rc $?"
    rustup component add clippy > $R/clippy-install.log 2>&1 9>&-
    cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; echo "clippy rc $?"; grep -E "^ +--> |^error" $R/clippy.log | sort | uniq
  } > $R/tests.log
  grep -E "test result|rc |FAILED|panicked" $R/tests.log | awk '{a[$0]++} END {for (k in a) print a[k]"x "k}' | while read -r l; do note "tests: $l"; done
  cd $ROOT; CI=1 timeout 5400 python -m pytest -q > $R/pytest.log 2>&1
  note "pytest rc $?: $(tail -1 $R/pytest.log)"
}
case "${1:-}" in
  tip)
    board bornhack 4 $B & sleep 2
    board bornhack-chk 4 $B TOPO_FORCED_CHECK=1 & sleep 2
    board qmtech-chk 1 $Q TOPO_FORCED_CHECK=1 & sleep 1
    board esp-chk 1 $E TOPO_FORCED_CHECK=1 & sleep 1
    board wiccon-chk 1 $W TOPO_FORCED_CHECK=1 & sleep 1
    board eprom-chk 1 $P TOPO_FORCED_CHECK=1 & sleep 1
    ( hzq run --threads 4 --name nd2-suite -- bash $ROOT/tools/ndperf_r2.sh suite; note "suite unit rc $?" ) & sleep 1
    ( hzq run --threads 2 --name nd2-mut -- bash $ROOT/tools/ndperf_r2.sh mutants; note "mutants unit rc $?" ) & sleep 1
    wait; note "all done" ;;
  base)
    board qmtech 1 $Q & sleep 1
    board esp 1 $E & sleep 1
    board wiccon 1 $W & sleep 1
    board eprom 1 $P & sleep 1
    wait; note "all done" ;;
  suite) suite ;;
  mutants) mutants ;;
esac
