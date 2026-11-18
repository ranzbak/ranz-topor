#!/usr/bin/env bash
# tw3f67_job.sh (branch tw3-f67, TW3 F6 DRAW-INDEX + F7 STUBS: exact speed-ups, copper must equal main 34877dc0).
#   tw3f67_job.sh quality       the public ladder (seed 0, optimize.rounds=1 runs=1, --no-png, same options as the
#                               vx4b-3a91f09 base of main 34877dc0: bornhack + qmtech 4 threads, the rest 1), the targeted
#                               release tests and the mutants (tools/tw3f67_mutants.py), hzq units, then wait
#   tw3f67_job.sh timing        exclusive timing: qmtech + esp, 2 runs each, all at once, 1 thread, TOPO_TIMES=1
#                               (per-stage seconds: plane stubs, drawing + feedback, via reduction)
#   tw3f67_job.sh tests-in | mut-in K...   (inside a unit)
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
A="--no-png --set optimize.rounds=1 --set optimize.runs=1"
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
TESTS_LIB="planes::tests exact::tests octi::tests geom::grid"
TESTS_INT="--test grid_props --test check_index --test topo_d6a --test topo_d6b --test topo_44 --test topo_pour_zero --test topo_s2a --test topo_s2b --test topo_pad_end --test topo_mb1"
one() { local n=$1 b=$2 t=$3
  hzq run --threads $t --name f67-$n -- env RAYON_NUM_THREADS=$t python -m harness.score_runs $R/seq/$n $b --only 0 --threads $t $A > $R/seq-$n.log 2>&1 9>&-
  note "seq-$n rc $?"; }
mutrun() {  # K: apply mutant K to a copy of the engine, run the F6/F7 tests, restore
  local k=$1 d=$2
  cp $ROOT/engine/src/topo/planes.rs $ROOT/engine/src/topo/exact.rs $ROOT/engine/src/topo/draw.rs $ROOT/engine/src/topo/octi.rs $d/engine/src/topo/
  python3 $ROOT/tools/tw3f67_mutants.py apply $d $k || { note "mutant $k: patch did not apply"; return; }
  touch $d/engine/src/topo/*.rs
  ( cd $d/engine && timeout 1800 cargo test --release --lib -- planes::tests::fill_index exact::tests octi::tests::room_and_follow 9>&- ) > $R/mut-$k.log 2>&1
  local rc=$?
  if [ $rc -ne 0 ]; then note "mutant $k: killed (rc $rc)"; else note "mutant $k: SURVIVED"; fi
}
case ${1:-} in
  quality)
    mkdir -p $R/seq
    one bornhack2026 bornhack2026 4 & sleep 2
    one qmtech-minimig $Q 4 & sleep 2
    for b in eprom-to-kernal wiccon-2023 $E; do one $(basename $b .kicad_pcb) $b 1 & sleep 1; done
    ( hzq run --threads 4 --name f67-tests -- bash $ROOT/tools/tw3f67_job.sh tests-in > $R/hzq-tests.log 2>&1; note "hzq tests rc $?" ) & sleep 1
    MS=($(python3 tools/tw3f67_mutants.py list))
    for u in A B C D; do
      case $u in A) o=0;; B) o=6;; C) o=12;; D) o=18;; esac
      ( hzq run --threads 4 --name f67-mut$u -- bash $ROOT/tools/tw3f67_job.sh mut-in $u "${MS[@]:$o:6}" > $R/hzq-mut$u.log 2>&1; note "hzq mut$u rc $?" ) & sleep 1
    done
    wait; note "all done" ;;
  tests-in)
    cd $ROOT/engine
    { CARGO_BUILD_JOBS=4 timeout 3000 cargo test --release --no-fail-fast --lib -- $TESTS_LIB 2>&1; echo "lib rc $?"
      CARGO_BUILD_JOBS=4 timeout 3000 cargo test --release --no-fail-fast $TESTS_INT 2>&1; echo "int rc $?"
      cargo clippy --release --all-targets 2>&1 | grep -E "^(warning|error)" -A3 | grep -E "^ +--> " | sort | uniq -c; echo "clippy rc ${PIPESTATUS[0]}"
    } > $R/tests.log 9>&-
    grep -E "test result|rc |FAILED|panicked" $R/tests.log | while read -r l; do note "tests: $l"; done
    note "tests done" ;;
  mut-in)
    u=$2; shift 2; d=/root/f67-mut-$u-$(basename $ROOT); mkdir -p $d
    cp -a $ROOT/engine $d/ 2>/dev/null; rm -rf $d/engine/target
    export CARGO_TARGET_DIR=/root/f67-mut-$u-target CARGO_BUILD_JOBS=4
    # The unmutated tree must pass first (else every mutant is "killed" for nothing).
    ( cd $d/engine && timeout 1800 cargo test --release --lib -- planes::tests::fill_index exact::tests octi::tests::room_and_follow 9>&- ) > $R/mut-$u-clean.log 2>&1 \
      || { note "mutants $u: the unmutated tests fail (rc $?), mutants not run"; exit 1; }
    for k in "$@"; do mutrun $k $d; done
    note "mutants $u done" ;;
  merge)  # the 4 public boards (same options as quality) + the whole release suite + clippy, at the merge with main
    mkdir -p $R/seq
    one qmtech-minimig $Q 4 & sleep 2
    for b in eprom-to-kernal wiccon-2023 $E; do one $(basename $b .kicad_pcb) $b 1 & sleep 1; done
    ( hzq run --threads 6 --name f67-suite -- bash -c "cd $ROOT/engine && { RAYON_NUM_THREADS=4 CARGO_BUILD_JOBS=6 cargo test --release --no-fail-fast 2>&1; echo \"cargo test rc \$?\"; cargo clippy --release --all-targets 2>&1; echo \"clippy rc \$?\"; } > $R/suite.log 9>&-"; note "suite unit rc $?"
      grep -E "^test result|rc [0-9]|FAILED|panicked|^(warning|error)" $R/suite.log | sort | uniq -c | sort -rn | head -40 | while read -r l; do note "suite: $l"; done ) & sleep 1
    wait; note "all done" ;;
  base4)  # the 4 public boards only (run with --rev main for the copper base)
    mkdir -p $R/seq
    one qmtech-minimig $Q 4 & sleep 2
    for b in eprom-to-kernal wiccon-2023 $E; do one $(basename $b .kicad_pcb) $b 1 & sleep 1; done
    wait; note "all done" ;;
  timing)
    for rep in 1 2; do
      for n in qmtech esp; do
        src=$([ $n = qmtech ] && echo $Q || echo $E); o=$R/t/$n-$rep; mkdir -p $o
        ( env RAYON_NUM_THREADS=1 TOPO_TIMES=1 tools/route_board.sh $src $o --clock cpu --timeout 2400 $A > $R/t-$n-$rep.log 2>&1; note "timing $n $rep rc $?" ) & sleep 1
      done
    done
    wait
    for f in $R/t-*.log; do note "$(basename $f .log): $(grep -E 'plane stubs|drawing \+ feedback|via reduction' $f | sed 's/^\[topo [a-z]* *[0-9]*\] //' | tr '\n' ';')"; done
    note "all done" ;;
  *) sed -n 2,9p "$0"; exit 2 ;;
esac
