#!/usr/bin/env bash
# defaults-flip-a1 (bulk, ccx43): every test binary once (flip rule 14c) + det s0 arms on 4 boards + the user patterns.
#   arms (det s0, optimize.runs=1 rounds=1, compass_cut_check): off = both flips off (copper must equal main),
#   fs = forced_shrink on only (changes 1+2), et = exit_taut on only (change 3), all = the new defaults (with TOPO_ABSORBDBG,
#   copper-neutral prints: the A1 neck list of the report).
#   defaults_flip_a1_job.sh            the job: 4 hzq units (suite, qm, small, pats), then "all done"
#   defaults_flip_a1_job.sh <unit>     one unit
set -u
OUT=${HZ_RUN_DIR:?}; TAG=${HZ_TAG:-dfa1}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
W=/root/$TAG-work; mkdir -p $W
note() { echo "$*" >> $OUT/rc.txt; }
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
OFF="--set drawing.forced_shrink=false --set search.exit_taut=false"
FS="--set search.exit_taut=false"
ET="--set drawing.forced_shrink=false"
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
run1() {  # run1 ARM NAME PCB [sets]: one det s0 board run at 1 thread
  local a=$1 n=$2 p=$3; shift 3
  TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 1 $C1 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-
  note "$a-$n rc $?"
}
arms() {  # arms NAME PCB: the 4 arms in parallel (4 threads)
  local n=$1 p=$2
  run1 off $n $p $OFF & run1 fs $n $p $FS & run1 et $n $p $ET & TOPO_ABSORBDBG=1 run1 all $n $p & wait
}
suite() {
  rustup component add clippy > /dev/null 2>&1
  (cd engine; CARGO_TARGET_DIR=$W/tgt CARGO_BUILD_JOBS=4 cargo clippy --release --all-targets > $OUT/clippy.log 2>&1 9>&-; note "clippy rc $?")
  (cd engine; CARGO_TARGET_DIR=$W/tgt RUST_TEST_THREADS=4 cargo test --release --no-fail-fast > $OUT/rel.log 2>&1 9>&-; note "rel rc $?")
  # the new stub step's debug arm (does any test kill it?)
  (cd engine; TOPO_FD45_OFF=mono_stub CARGO_TARGET_DIR=$W/tgt RUST_TEST_THREADS=4 cargo test --release --no-fail-fast --test topo_draw_absorb --test topo_neck_regrow --lib > $OUT/arm-mono_stub.log 2>&1 9>&-; note "arm mono_stub rc $? (want != 0)")
  CI=1 python -m pytest -q -p no:cacheprovider > $OUT/pytest.log 2>&1 9>&-; note "pytest rc $?"
  (cd plugin && python -m topor.docs_gen --check > $OUT/docs_gen.log 2>&1; note "docs_gen rc $?")
}
pats() {  # the user patterns (APPROVED 2026-11-26) at the new defaults (all) and with exit_taut off (et0), PNGs
  local P=$OUT/pats NAMES="neck_both_ends neck_reentry"
  mkdir -p $P/runs
  python -m harness.neckrg_patterns $P/boards --drc > $P/gen.log 2>&1; note "pats gen rc $?"
  python - $P/runs/labels.json <<'PY'
import json, sys
json.dump({"all": "defaults-flip-a1 (exit_taut, forced_shrink on)", "et0": "exit_taut false"}, open(sys.argv[1], "w"))
PY
  for a in all et0; do for n in $NAMES; do
    if [ $a = et0 ]; then set -- --set search.exit_taut=false; else set --; fi
    tools/route_board.sh $P/boards/$n/problem/$n.kicad_pcb $P/runs/$a/$n/$n/s0 --keep-copper --no-png --set optimize.runs=1 --set optimize.rounds=1 "$@" > $P/$a-$n.txt 2>&1 9>&-
    note "pat $a $n rc $?"
  done; done
  python -m harness.neckrg_patterns_png $P/runs $P/png > $P/png.log 2>&1; note "pats png rc $?"
}
case ${1:-job} in
  suite) suite ;;
  qm) arms qm $QM ;;
  small) arms esp $ESP; arms wic $WIC; arms ep $EP ;;
  pats) pats ;;
  job)
    git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
    hzq run --threads 4 --name $TAG-qm -- bash $ROOT/tools/defaults_flip_a1_job.sh qm 9>&- & sleep 3
    hzq run --threads 4 --name $TAG-suite --pin -- bash $ROOT/tools/defaults_flip_a1_job.sh suite 9>&- & sleep 3
    hzq run --threads 4 --name $TAG-small -- bash $ROOT/tools/defaults_flip_a1_job.sh small 9>&- & sleep 3
    hzq run --threads 2 --name $TAG-pats --pin -- bash $ROOT/tools/defaults_flip_a1_job.sh pats 9>&- &
    wait
    rm -rf $W/tgt
    find $OUT -name '*.log' -size +20M -exec gzip -9 {} + 2>/dev/null
    note "all done" ;;
esac
