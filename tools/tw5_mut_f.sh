#!/usr/bin/env bash
# tw5_mut_f.sh (branch tw5-escape): the TW5 (f) loop_p4 mutants, each against the tests that guard it.
#   tw5_mut_f.sh <out-dir>   copies engine/ into <out-dir>/tree (its own target dir), one mutant at a time;
#                            notes "mutant <name>: killed|SURVIVED|patch did not apply" in <out-dir>/mut.txt
set -u
O=${1:?out dir}; ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p $O/tree; rsync -a --delete --exclude target $ROOT/engine/ $O/tree/engine/; cp $ROOT/Cargo.toml $ROOT/Cargo.lock $O/tree/ 2>/dev/null
find $O/tree -name '*.rs' -exec touch {} +
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$O/target}
note() { echo "$*" >> $O/mut.txt; }
mut() {  # name file test-args... ; the python snippet on stdin edits the file
  local n=$1 f=$O/tree/engine/$2; shift 2
  if [ -n "${ONLY:-}" ] && ! [[ " $ONLY " == *" ${n%%_*} "* ]]; then cat > /dev/null; return; fi
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  ( cd $O/tree/engine && cargo test --release "$@" 9>&- > $O/mut-$n.log 2>&1 ); local rc=$?
  mv $f.orig $f; touch $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
sub() { cat <<EOF2
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF2
}
C=src/topo/core.rs
T=(--test tw5_escape -- loop_p4)
sub '                if !self.tw5.loop_p4 {
                    return false;' '                if true {
                    return false;' | mut p1_skip $C "${T[@]}"
sub 'p4_keep_heaviest(vic, (alive / 4).max(10).min(self.max_ripped))' 'p4_keep_heaviest(vic, 0)' | mut p2_cap0 $C "${T[@]}"
sub 'let worse = !lost && !self.tw5.take_always && self.eval_now() > e0 + 1e-6;' 'let worse = false;' | mut p3_take $C "${T[@]}"
sub '                let k0 = self.work_mark();
                self.history_step(cs);
                let wt = {' '                let k0 = self.work_mark();
                let wt = {' | mut p4_history $C "${T[@]}"
sub '                if !self.tw5.throttle.due() {
                    cs.p4_throttled += 1;' '                if false {
                    cs.p4_throttled += 1;' | mut p5_throttle $C "${T[@]}"
sub 'self.tw5.throttle.ran_work(w);' 'let _ = w;' | mut p6_charge $C "${T[@]}"
sub 'v.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));' 'v.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));' | mut p7_heaviest $C --lib -- p4_keep_heaviest
sub '                if lost {
                    self.restore(&before);
                    cs.p4_undone += 1;' '                if lost {
                    cs.p4_undone += 1;' | mut p8_restore $C "${T[@]}"
sub 'let g = |d: f64| { let d = if d.is_finite() { d.max(0.0) } else { 0.0 }; (1.0 + d) / (2.0 + d) };' 'let g = |_d: f64| 0.5;' | mut p9_weight $C "${T[@]}"
sub 'hist: (opts.loop_history || opts.loop_p4).then(' 'hist: opts.loop_history.then(' | mut p10_hist_on $C "${T[@]}"
note "mutants done"
