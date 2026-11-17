#!/usr/bin/env bash
# cm1_job.sh <stage...>: cm1-room VM job (hzq units, results under $HZ_RUN_DIR). Stages:
#   gui     wiccon GUI engine input (name-sorted net codes, reports/wiccon-gui27-20261117-2215.md) through the
#           headless plugin flow with the defaults (+ the harness numbering), and route_board wiccon defaults;
#           inputs in tools/cm1_models (throw-away VM commit only)
#   boards  ladder seq: 5 public boards, seed 0, optimize.rounds=1 runs=1, --no-png (3 units)
#   tests   targeted release tests (rule 14c) + the config pytest files
#   mut     mutants of the lines this branch changed (tools/cm1_mutate.py)
set -u
R=$HZ_RUN_DIR; ROOT=$(pwd); mkdir -p $R
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
M=$ROOT/tools/cm1_models
A="--only 0 --no-png --set optimize.rounds=1 --set optimize.runs=1"
note() { echo "$*" >> $R/rc.txt; }
br() { local n=$1 b=$2; shift 2; mkdir -p $(dirname $R/$n); python -m harness.score_runs $R/$n $b $A "$@" > $R/$n.log 2>&1; note "run $n rc $?"; }
app() {  # app <out> <model.json>: headless plugin flow on a dumped engine input
  local out=$R/$1 model=$2; mkdir -p $out/board
  cp $M/input/2023.kicad_pro $M/input/2023.kicad_pcb $out/board/
  TOPOR_DUMP_ENGINE_INPUT=$out/dump TOPOR_REPORT_ROOT=$out/reports PYTHONPATH="$ROOT:$ROOT/plugin" \
    python -m topor.app --headless $out/board/2023.kicad_pcb --model $model --accept > $out/run.log 2>&1; note "app $1 rc $?"; }
rb() { local out=$R/$1; shift; mkdir -p $out; TOPOR_REPORT_ROOT=$out/reports tools/route_board.sh wiccon-2023 $out --no-png "$@" > $out/run.log 2>&1; note "rb $(basename $out) rc $?"; }
tests() {
  cd $ROOT/engine
  CARGO_TARGET_DIR=$ROOT/target RUST_TEST_THREADS=4 timeout 3000 cargo test --release --no-fail-fast --lib -- viacut route::tests refine:: pack:: cutmodel:: > $R/cargo-lib.log 2>&1; note "cargo lib rc $?"
  CARGO_TARGET_DIR=$ROOT/target RUST_TEST_THREADS=4 timeout 3000 cargo test --release --no-fail-fast --test topo_cm1_room --test topo_reroute52 --test topo_compass --test topo_q14 --test topo_d3 --test topo_s3c --test topo_p5_price > $R/cargo-int.log 2>&1; note "cargo int rc $?"
  cd $ROOT
  timeout 1200 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_toml_defaults.py > $R/pytest.log 2>&1; note "pytest rc $?"
}
u() { local n=$1; shift; ( hzq run --threads 4 --name "$n" -- bash -c "$(declare -f note br app rb tests); export R=$R A='$A' ROOT=$ROOT M=$M E=$E Q=$Q; $*" > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & }
for st in "$@"; do
  case $st in
    gui) u gui "app gui $M/gui.json; app gui-hcodes $M/gui-hcodes.json; rb rbdef" ;;
    boards)
      u bh "br seq/bornhack bornhack2026"
      u qm "br seq/qmtech $Q"
      u small "br seq/eprom eprom-to-kernal; br seq/wiccon wiccon-2023; br seq/esp $E" ;;
    tests) u tests "tests" ;;
    mut) u mut "python3 tools/cm1_mutate.py run $R/mut > $R/mut.log 2>&1; note 'mutants rc '\$?" ;;
  esac
done
wait; note "all done"
