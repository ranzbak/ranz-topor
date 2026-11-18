#!/usr/bin/env bash
# tw3_job.sh (branch tw3-table, docs/dev/design/tw3-table.md): TW3 TABLE acceptance on the VM.
#   tw3_job.sh tip      tip rev: bornhack first (4 threads), the targeted release tests + clippy + pytest + mutants
#                       (one 4-thread unit), the fixed-cycle ladder (eprom, wiccon, qmtech, esp seeds 0-4) and the
#                       drawing cost at runs = 1 / 1 thread (lead condition 1), hzq units, then wait
#   tw3_job.sh base     base rev (main): bornhack first, the same ladder, and the F6/F7 copper check runs
#                       (rounds=1 runs=1, the vx4b options: qmtech 4 threads, the rest 1)
#   tw3_job.sh lad NAME BOARD SEED THREADS SETS...   one ladder run (inside a unit)
#   tw3_job.sh tests-in                              (inside a unit)
# The script may run from a base tree that does not have it (piped in): ROOT = the current dir (hz.sh runs in the
# repo root). Ladder: score_runs cpu clock 2400 s (never raised), --no-png, fixed cycles: optimize.rounds per board
# and optimize.loop_stable_window=1000 so neither side stops at stable (lead: compare at fixed cycle counts),
# runs = 5 (the default), TOPO_TIMES=1. Rounds: eprom 300, wiccon 150 (cheap cycles; a restart period of 2400
# re-lays is ~40-90 cycles there), esp 30 (the default cap), qmtech 10 (the loop's share of the 2400 s budget at
# runs = 5), bornhack rounds=1 runs=1 (one run already takes ~1100 s at 4 threads).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(pwd)
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
SELF=$R/tw3_job.sh
[ -f $SELF ] || cp "$0" $SELF
FIX=(--set optimize.loop_stable_window=1000 --set optimize.runs=5)
lad() {  # NAME BOARD SEED THREADS SETS...
  local n=$1 b=$2 s=$3 t=$4; shift 4
  local t0=$(date +%s)
  env RAYON_NUM_THREADS=$t TOPO_TIMES=1 python -m harness.score_runs $R/lad/$n $b --only $s --threads $t --no-png "$@" > $R/lad-$n.log 2>&1 9>&-
  note "lad $n rc $? wall $(( $(date +%s) - t0 )) s sets=[$*]"
  rm -rf $R/lad/$n/*/s*/work 2>/dev/null
}
unit() {  # NAME THREADS lad-args...
  local n=$1 t=$2; shift 2
  ( hzq run --threads $t --name tw3-$n -- bash $SELF lad "$@" > $R/hzq-$n.log 2>&1 9>&-; note "unit $n rc $?" ) & sleep 1
}
ladder() {
  unit bornhack 4 bornhack bornhack2026 0 4 --set optimize.rounds=1 --set optimize.runs=1
  unit qmtech 4 qmtech $Q 0 4 --set optimize.rounds=10 "${FIX[@]}"
  for s in 0 1 2 3 4; do unit esp-s$s 2 esp-s$s $E $s 2 --set optimize.rounds=30 "${FIX[@]}"; done
  unit wiccon 2 wiccon wiccon-2023 0 2 --set optimize.rounds=150 "${FIX[@]}"
  unit eprom 1 eprom eprom-to-kernal 0 1 --set optimize.rounds=300 "${FIX[@]}"
}
sub() { cat <<EOF
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF
}
mut() {  # NAME FILE(rel. to engine/) cargo-test-args... ; the python snippet on stdin edits the file in the copy D
  local n=$1 f=$D/engine/$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  ( cd $D/engine && timeout 2400 cargo test --release "$@" 9>&- ) > $R/mut-$n.log 2>&1; local rc=$?
  mv $f.orig $f; touch $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed (rc $rc)"; else note "mutant $n: SURVIVED"; fi
}
LIB=(--lib -- topo::archive topo::table topo::optimise)
INT=(--test topo_tw3_table --test topo_tw3_drawonly --test topo_m2_draw_once --test topo_stop_reason --test loop_local --test cancel_latency --test time_limit)
tests() {
  cd $ROOT/engine
  export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=2
  { timeout 3000 cargo test --release --no-fail-fast "${LIB[@]}" 2>&1; echo "lib rc $?"
    timeout 3000 cargo test --release --no-fail-fast "${INT[@]}" 2>&1; echo "int rc $?"
    cargo clippy --release --all-targets 2>&1 | grep -E "^(warning|error)" -A3 | grep -E "^ +--> " | sort | uniq -c; echo "clippy rc ${PIPESTATUS[0]}"
  } > $R/tests.log 9>&-
  grep -E "test result|rc |FAILED|panicked" $R/tests.log | while read -r l; do note "tests: $l"; done
  cd $ROOT; CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_report.py > $R/pytest.log 2>&1 9>&-
  note "pytest rc $?: $(tail -1 $R/pytest.log)"
  # mutants, in a copy of the engine (the ladder units share this tree)
  D=/root/tw3-mut-$(basename $ROOT); rm -rf $D; mkdir -p $D; cp -a $ROOT/engine $D/; rm -rf $D/engine/target
  export CARGO_TARGET_DIR=/root/tw3-mut-target-$(basename $ROOT)
  local A=src/topo/archive.rs T=src/topo/table.rs O=src/topo/optimise.rs C=src/topo/core.rs
  sub '&& !four.contains(&Some(i)) ' '' | mut m1_prune_bests $A "${LIB[@]}"
  sub 'pick != "errors_weighted" => Objective::Best { mm_per_via, penalty: false },' 'pick != "errors_weighted" => Objective::Best { mm_per_via: 25.0, penalty: false },' | mut m2_fixed_25 $A "${LIB[@]}"
  sub '            if c >= h {
                break;
            }' '' | mut m3_commit_on_arrival $T "${LIB[@]}"
  sub '!g.gain[w.clone()].iter().any(|&x| x) && g.restarts[w].iter().sum::<usize>() >= 2' '!g.gain[w].iter().any(|&x| x)' | mut m4_stable_no_restarts $T "${LIB[@]}"
  sub 'view.rows[i].2.compat == compat && view.rows[i].2.fp != my_fp' 'view.rows[i].2.compat == compat' | mut m5_restart_own_state $O "${LIB[@]}"
  sub 'if gaining && !duplicate {' 'if false {' | mut m6_cut_gaining $O "${LIB[@]}"
  sub 'bests.truncate(if cancel { 1 } else { g.quota });' 'bests.truncate(1);' | mut m7_one_best $C --test topo_tw3_table
  sub 'let who = if cands.contains(&row.run) { Some(row.run) } else {' 'let who = if false { Some(row.run) } else {' | mut m8_not_origin $C --test topo_tw3_table
  rm -rf $D $CARGO_TARGET_DIR
  note "tests done"
}
case ${1:-} in
  lad) shift; lad "$@" ;;
  tests-in) tests ;;
  tip)
    mkdir -p $R/lad
    ladder
    ( hzq run --threads 4 --name tw3-tests -- bash $SELF tests-in > $R/hzq-tests.log 2>&1 9>&-; note "unit tests rc $?" ) & sleep 1
    # drawing cost (lead condition 1): runs = 1, 1 thread, the four bests drawn one after another
    for nb in "eprom eprom-to-kernal 300" "wiccon wiccon-2023 150" "esp $E 30" "qmtech $Q 10"; do
      set -- $nb; unit dc-$1 1 dc-$1 $2 0 1 --set optimize.rounds=$3 --set optimize.loop_stable_window=1000 --set optimize.runs=1
    done
    wait; note "all done" ;;
  base)
    mkdir -p $R/lad
    ladder
    for nb in "eprom eprom-to-kernal 1" "wiccon wiccon-2023 1" "esp $E 1" "qmtech $Q 4"; do
      set -- $nb; unit f67-$1 $3 f67-$1 $2 0 $3 --set optimize.rounds=1 --set optimize.runs=1
    done
    wait; note "all done" ;;
  *) sed -n 2,16p "$0"; exit 2 ;;
esac
