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
TF=(--test tw5_escape -- loop_p4_rounds_fold)
TB=(--test tw5_escape -- loop_p4_rounds_run_beside)
# F2: the rounds share the escapes' throttle again
sub 'if !self.tw5.p4_throttle.due() {' 'if !self.tw5.throttle.due() {' | mut f2a_shared $C "${TB[@]}"
sub 'self.tw5.p4_throttle.ran_work(w);' 'self.tw5.throttle.ran_work(w);' | mut f2b_charge $C --test tw5_escape -- loop_p4
sub '                    self.tw5.p4_throttle.add(units);
' '' | mut f2c_nowork $C --test tw5_escape -- loop_p4
# F1: a round that is the history alone (no rip), no cap, no history step
sub '                if vw.is_empty() {
                    return false;' '                if true {
                    return false;' | mut f1a_norip $C "${TF[@]}"
sub 'p4_keep_heaviest(vic, (alive / 4).max(10).min(self.max_ripped))' 'p4_keep_heaviest(vic, 1)' | mut f1b_cap1 $C "${TF[@]}"
sub '                    self.restore(&before);
                    cs.p4_undone += 1;' '                    cs.p4_undone += 1;' | mut f1c_restore $C --test tw5_escape -- loop_p4
# low: the stop poll; the halved shares
sub '                    if stop() {
                        lost = true;
                        break;' '                    if false {
                        lost = true;
                        break;' | mut l1_stop $C --test tw5_escape -- loop_p4
sub 'if opts.escape == "layering" && opts.loop_p4 { opts.escape_share * 0.5 } else { opts.escape_share }, opts.escape_kappa, st_alive),
                p4_throttle' 'opts.escape_share, opts.escape_kappa, st_alive),
                p4_throttle' | mut l2_half_esc $C "${TB[@]}"
note "mutants done"
