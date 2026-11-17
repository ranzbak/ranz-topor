#!/usr/bin/env bash
# edge_job.sh <stage...>: edge-clear VM job (hzq units, results under $HZ_RUN_DIR). Stages:
#   boards  5 public boards x seq/erbs, seed 0, optimize.rounds=1 runs=1, --no-png (3 units: small / qmtech / bornhack)
#   tests   cargo test --release (all), pytest (default + mechfix test_fx_edge_pads), fx edge_pads PNG, mutants
#           (tools/edge_mutate.py), cargo test debug (own tests) -- one unit, serial
#   wq      rule C boards: wiccon + qmtech x seq/erbs (copper-to-edge 0 on both; the others are non-zero, C off)
#   ctests  pytest (default) + rule C's Python mutants (tools/edge_mutate.py run-py)
set -u
R=$HZ_RUN_DIR; ROOT=$(pwd); mkdir -p $R
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
A="--only 0 --no-png --set optimize.rounds=1 --set optimize.runs=1"
note() { echo "$*" >> $R/rc.txt; }
br() { local n=$1 b=$2; shift 2; python -m harness.score_runs $R/$n $b $A "$@" > $R/$n.log 2>&1; note "run $n rc $?"; }
pair() { local k=$1 b=$2; br seq/$k $b; br erbs/$k $b --set drawing.realise=erbs; }
u() { local n=$1; shift; ( hzq run --threads 4 --name "$n" -- bash -c "$(declare -f note br pair tests); R=$R A='$A' ROOT=$ROOT $*" > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & }
tests() {
  cd $ROOT/engine
  CARGO_TARGET_DIR=$ROOT/target RUST_TEST_THREADS=4 timeout 5400 cargo test --release --no-fail-fast > $R/cargo-release.log 2>&1; note "cargo release rc $?"
  cd $ROOT
  timeout 5400 python -m pytest -q tests > $R/pytest.log 2>&1; note "pytest rc $?"
  timeout 1200 python -m pytest -q tests/test_fx_edge_pads.py -m mechfix > $R/pytest-fx.log 2>&1; note "pytest fx rc $?"
  if [ -z "${DISPLAY:-}" ]; then P="xvfb-run -a"; else P=""; fi
  python -m harness.fx_survivors $R/fx/boards --drc > $R/fx-gen.log 2>&1
  for m in seq erbs; do X="--set optimize.rounds=1"; [ $m = erbs ] && X="$X --set drawing.realise=erbs"
    tools/route_board.sh $R/fx/boards/edge_pads/problem/edge_pads.kicad_pcb $R/fx/runs/edge_pads-$m --no-png --timeout 120 $X > $R/fx-$m.log 2>&1; note "fx $m rc $?"; done
  $P python -m harness.fx_survivors_png $R/fx/boards $R/fx/runs > $R/fx-png.log 2>&1; note "fx png rc $?"
  python3 tools/edge_mutate.py run $R/mut > $R/mut.log 2>&1; note "mutants rc $?"
  cd $ROOT/engine
  CARGO_TARGET_DIR=$ROOT/target-dbg RUST_TEST_THREADS=4 timeout 7200 cargo test --no-fail-fast > $R/cargo-debug.log 2>&1; note "cargo debug rc $?"
}
for st in "$@"; do
  case $st in
    boards)
      u small "pair eprom eprom-to-kernal; pair wiccon wiccon-2023; pair esp $E"
      u qm "pair qmtech $Q"
      u bh "pair bornhack bornhack2026" ;;
    tests) u tests "tests" ;;
    wq)
      u wic "pair wiccon wiccon-2023"
      u qm "pair qmtech $Q" ;;
    ctests) u ctests "cd $ROOT; timeout 5400 python -m pytest -q tests > $R/pytest.log 2>&1; note 'pytest rc '\$?; python3 tools/edge_mutate.py run-py $R/mut > $R/mut.log 2>&1; note 'mutants rc '\$?" ;;
  esac
done
wait; note "all done"
