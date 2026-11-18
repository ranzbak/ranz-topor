#!/usr/bin/env bash
# stable-label r5 (merge of main bc42ab0): mutants of the core.rs conflict resolution; each must fail a test of its branch.
# usage: tools/sl5_merge_mut.sh <outdir> <backup-of-core.rs>   Modifies only engine/src/topo/core.rs (restored from the backup).
set -u
R=$1; B=$2; mkdir -p "$R"
mut() { name=$1; tests=$2; old=$3; new=$4
  python3 - "$old" "$new" <<'P'
import sys
p='engine/src/topo/core.rs'; s=open(p).read(); o,n=sys.argv[1],sys.argv[2]
assert s.count(o)==1, o
open(p,'w').write(s.replace(o,n))
P
  cargo test --release --manifest-path engine/Cargo.toml $tests > "$R/mut-$name.txt" 2>&1; rc=$?
  cp "$B" engine/src/topo/core.rs
  if [ $rc -ne 0 ]; then echo "$name killed ($(grep -E '^test .* FAILED' "$R/mut-$name.txt" | head -2 | tr '\n' ' '))"; else echo "$name SURVIVED"; fi
}
mut r_no_grace "--test topo_limit_wait" ' || ctl.limit.grace_over() || (!soft() && loop_over());' ' || (!soft() && loop_over());'
mut r_no_dropped_note "--test topo_limit_wait" 'ctl.limit.note_cycle_dropped(); // a cycle the limit cut' '// a cycle the limit cut'
mut r_no_loop_scope "--test topo_limit_wait --test topo_limit_erbs_end" 'crate::control::in_loop_scope(|| optimise(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock, &mut on_pick))' 'optimise(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock, &mut on_pick)'
mut r_no_onpick "--test topo_mb0 --test topo_stop_reason" '            live(&LiveLoop { pick: *pick, planned, total, stable });
        };' '            let _ = (pick, planned, total, &live);
        };'
mut r_no_yield "--test topo_mb0 --test topo_stop_reason" '                if gate.1.slots.yield_to_first() {' '                if false && gate.1.slots.yield_to_first() {'
echo "all done"
