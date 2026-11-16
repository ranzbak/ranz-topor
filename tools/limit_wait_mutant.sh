#!/usr/bin/env bash
# limit-wait: one mutant: tools/limit_wait_mutant.sh <name>; copies the tree, applies the edit, runs the killing tests
set -u
n=$1; W=/root/lw-mut-$n
python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$W"
mkdir -p "$W"
tar --exclude=./target --exclude=./scratch --exclude=./.git -cf - . | tar -xf - -C "$W"
cd "$W" || exit 2
python3 - "$n" <<'PY'
import sys
n=sys.argv[1]
M={
 "inner_no_grace":("engine/src/topo/core.rs","ctl.limit.grace_over() || (!soft()","(!soft()"),
 "scope_ignored":("engine/src/control.rs","(LOOP_SCOPE.with(|s| s.get()) && l.grace_over())","l.grace_over()"),
 "grace_ignored":("engine/src/control.rs","(Some(l), Some(g)) => self.elapsed_s() >= l + g,","(Some(l), Some(_)) => self.elapsed_s() >= l,"),
 "no_set_grace":("engine/src/topo/core.rs","    ctl.limit.set_grace(opts.limit_grace_s);\n","    let _ = opts.limit_grace_s;\n"),
 "partial_offered":("engine/src/topo/optimise.rs","        if inner() {\n            // cut short inside (Cancel)","        if false && inner() {\n            // cut short inside (Cancel)"),
 "no_loop_end":("engine/src/topo/core.rs","            ctl.limit.note_loop_end(); // the wait","            // the wait"),
 "no_hit":("engine/src/control.rs","            self.grace_hit.store(true, Ordering::Relaxed);\n","            let _ = 0;\n"),
 "default_off":("engine/src/model.rs","            limit_grace_s: 30.0,","            limit_grace_s: -1.0,"),
 "no_extra_stop":("engine/src/topo/core.rs","                if stop() {\n                    cs.busy_s = t0.elapsed().as_secs_f64();\n                    return cs;\n                }\n                // Floating up","                // Floating up"),
}
f,a,b=M[n]; s=open(f).read(); assert a in s,(n,"pattern not found"); open(f,"w").write(s.replace(a,b,1))
PY
[ $? -eq 0 ] || { echo "MUTANT $n: edit failed"; exit 2; }
export CARGO_TARGET_DIR=/root/lw-mut-target-$n
case $n in
 default_off) T="--lib model::";;
 inner_no_grace|no_set_grace|no_loop_end|no_hit|partial_offered|no_extra_stop) T="--test topo_limit_wait --lib optimise::";;
 *) T="--lib control::";;
esac
if cargo test --release --manifest-path engine/Cargo.toml $T; then echo "MUTANT $n: SURVIVED"; else echo "MUTANT $n: KILLED"; fi
