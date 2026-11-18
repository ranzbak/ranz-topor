#!/usr/bin/env bash
# limit-wait: all mutants one after another on one copy of the tree (shared incremental target).
# usage (cwd = the synced tree): bash mutants_all.sh name...
set -u
SRC=$PWD; W=/root/lw-mut-all
python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$W"
mkdir -p "$W"
tar --exclude=./target --exclude=./scratch --exclude=./.git -cf - . | tar -xf - -C "$W"
cd "$W" || exit 2
export CARGO_TARGET_DIR=/root/lw-mut-all-target
cat > mut.py <<'PY'
import sys,shutil
n=sys.argv[1]; src=sys.argv[2]
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
f,a,b=M[n]; shutil.copy(src+"/"+f, f); s=open(f).read(); assert a in s,(n,"pattern not found"); open(f,"w").write(s.replace(a,b,1))
PY
for n in "$@"; do
  for f in engine/src/topo/core.rs engine/src/control.rs engine/src/topo/optimise.rs engine/src/model.rs; do cp "$SRC/$f" "$f"; done
  python3 mut.py "$n" "$SRC" || { echo "MUTANT $n: edit failed"; continue; }
  killed=0
  cargo test --release --manifest-path engine/Cargo.toml --test topo_limit_wait > mut-$n.t1.log 2>&1 || killed=1
  cargo test --release --manifest-path engine/Cargo.toml --lib -- control:: optimise:: model::tests::the_limit_grace > mut-$n.t2.log 2>&1 || killed=1
  if [ $killed = 1 ]; then echo "MUTANT $n: KILLED ($(grep -h '^test .* FAILED\|panicked\|error\[' mut-$n.t*.log | head -3 | tr '\n' ' '))"; else echo "MUTANT $n: SURVIVED"; fi
done
