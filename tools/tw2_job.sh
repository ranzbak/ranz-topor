#!/usr/bin/env bash
# tw2_job.sh (branch tw2-metric, docs/dev/design/tw2-metric.md): board ladder, tests and mutants on the VM.
#   tw2_job.sh ladder CFG...   per CFG one hzq unit for qmtech (4 threads, started first) and one for esp + eprom +
#                              wiccon (1 thread; <= 4 waiting units per job: 2 CFGs per job), then wait
#   tw2_job.sh tests           one hzq unit (4 threads): cargo test --release, clippy, targeted pytest, TW2 mutants
#   tw2_job.sh both CFG...     ladder CFG... and tests in one job
#   tw2_job.sh retest          one hzq unit (4 threads): clippy (log kept), the TW2 tests, mutants j3 and d1
#   tw2_job.sh task CFG BOARD  one run (inside a unit)
# CFG: D (the tree's defaults; main = sequential, tw2-metric = erbs joint), J (realise erbs), S (realise sequential),
#      JF / SF (J / S with optimize.allow_violations = false: no never-drop forcing), JD (J + TOPO_ERBS_DEBUG=1: per-run trace).
# Runs: seed 0, optimize.rounds=1 (one fixed TopoR loop cycle), runs=1, --clock cpu --timeout 2400, TOPO_TIMES=1.
# Out: $HZ_RUN_DIR/<CFG>/<board>/ (route_board.sh layout; tools ladder_diff.py <tag>/S <tag>/J).
# bornhack is not in the ladder here (lead 2026-11-18: once, at the end).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; bornhack) echo bornhack2026;; esac; }
cfg() {
  SETS=(); ENVS=()
  case $1 in
    D) ;;
    JD) SETS=(--set 'drawing.realise="erbs"'); ENVS=(TOPO_ERBS_DEBUG=1 TOPO_FUSEDBG=1) ;;
    JN) SETS=(--set 'drawing.realise="erbs"'); ENVS=(TOPO_NOREDUCE=1 TOPO_FUSEDBG=1) ;;
    J) SETS=(--set 'drawing.realise="erbs"') ;;
    S) SETS=(--set 'drawing.realise="sequential"') ;;
    JF) SETS=(--set 'drawing.realise="erbs"' --set optimize.allow_violations=false) ;;
    SF) SETS=(--set 'drawing.realise="sequential"' --set optimize.allow_violations=false) ;;
    *) echo "tw2_job: unknown config $1" >&2; exit 2 ;;
  esac
}
task() {
  local c=$1 n=$2 th=${3:-1}; cfg $c
  local o=$R/$c/$n; mkdir -p $o
  local t0=$(date +%s)
  env RAYON_NUM_THREADS=$th TOPO_TIMES=1 "${ENVS[@]}" tools/route_board.sh "$(board_path $n)" $o --clock cpu --timeout 2400 --no-png \
    --set optimize.rounds=1 --set optimize.runs=1 "${SETS[@]}" > $R/$c-$n.log 2>&1
  note "task $c $n rc $? wall $(( $(date +%s) - t0 )) s"
  rm -rf $o/work $o/*/work 2>/dev/null
}
ladder() {
  for c in "$@"; do
    ( hzq run --threads 4 --name "$c-qmtech" -- bash $ROOT/tools/tw2_job.sh task $c qmtech 4; ) & sleep 1
  done
  for c in "$@"; do
    ( hzq run --threads 1 --name "$c-rest" -- bash $ROOT/tools/tw2_job.sh small $c ) & sleep 1
  done
}
mut() {  # name file test-args... ; the python snippet on stdin edits the file
  local n=$1 f=$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  touch $f
  ( cd $ROOT/engine && cargo test --release "$@" > $R/mut-$n.log 2>&1 9>&- ); local rc=$?
  mv $f.orig $f; touch $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
sub() { cat <<EOF
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF
}
tests() {
  cd $ROOT/engine
  { timeout 5400 cargo test --release --no-fail-fast 2>&1 9>&-; echo "cargo test rc $?"
    rustup component add clippy > $R/clippy-install.log 2>&1 9>&-
    cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; echo "clippy rc $?"; grep -E "^ +--> |^error" $R/clippy.log | sort | uniq
  } > $R/tests.log
  grep -E "test result|rc |FAILED|panicked" $R/tests.log | while read -r l; do note "tests: $l"; done
  cd $ROOT; CI=1 python -m pytest -q tests/plugin/test_postrun.py tests/plugin/test_config.py tests/plugin/test_config_docs.py \
    tests/plugin/test_messages.py > $R/pytest.log 2>&1
  note "pytest rc $?: $(tail -1 $R/pytest.log)"
  local J=$ROOT/engine/src/topo/joint.rs D=$ROOT/engine/src/topo/draw.rs M=$ROOT/engine/src/model.rs
  sub 'if self.cur != Some(o) {' 'if self.cur == Some(o) {' | mut j1_lost_other $J --lib topo::joint
  sub 'if (bands[a].gap || bands[c].gap) && d <= gap_max {' 'if false && d <= gap_max {' | mut j2_gap_keep $J --test tw2_metric
  sub 'if !stat[bi] && !b.octi {' 'if false {' | mut j3_octi_rescue $J --test tw2_metric
  sub 'r.forced_erbs += 1;' '' | mut j4_tally_forced $J --lib topo::joint
  sub '            self.release_bands(w);' '' | mut d1_release $D --test tw2_metric
  sub 'out.viol.extend(gv);' 'drop(gv);' | mut d2_gap_viol $D --test tw2_metric
  sub 'let adopted = if attempt < 0 { res.take() } else { None };' 'let adopted = if false { res.take() } else { None };' | mut d3_adopt $D --test tw2_metric
  sub 'realise: "erbs".into(),' 'realise: "sequential".into(),' | mut m1_default $M --test tw2_metric
  note "tests done"
}
retest() {  # targeted: the tests and mutants a fix round touched, clippy with its log
  cd $ROOT/engine
  rustup component add clippy > $R/clippy-install.log 2>&1 9>&-  # the VM image has no clippy (tw2l-retest 2026-11-18)
  cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; note "clippy rc $?: $(grep -cE '^(warning|error)' $R/clippy.log) warning/error lines"
  { cargo test --release --lib topo::joint 9>&-; cargo test --release --test tw2_metric --test topo_m3_patterns --test topo_m3_class_keep 9>&-; } > $R/retest.log 2>&1
  note "retest rc $?"; grep -E "test result|FAILED|panicked" $R/retest.log | while read -r l; do note "retest: $l"; done
  local J=$ROOT/engine/src/topo/joint.rs D=$ROOT/engine/src/topo/draw.rs
  sub 'if !stat[bi] && !b.octi {' 'if false {' | mut j3_octi_rescue $J --test tw2_metric
  sub '            self.release_bands(w);' '' | mut d1_release $D --test tw2_metric
  note "retest done"
}
case ${1:-} in
  task) shift; task "$@" ;;
  small) for n in esp eprom wiccon; do task $2 $n; done ;;
  ladder) shift; ladder "$@"; wait; note "all done" ;;
  one) shift; hzq run --threads ${3:-1} --name "$1-$2" -- bash $ROOT/tools/tw2_job.sh task "$@"; note "all done" ;;
  tests) hzq run --threads 4 --name tw2-tests -- bash $ROOT/tools/tw2_job.sh tests-in; note "all done" ;;
  tests-in) tests ;;
  retest) hzq run --threads 4 --name tw2-retest -- bash $ROOT/tools/tw2_job.sh retest-in; note "all done" ;;
  retest-in) retest ;;
  both) shift; ladder "$@"; ( hzq run --threads 4 --name tw2-tests -- bash $ROOT/tools/tw2_job.sh tests-in ) & sleep 1; wait; note "all done" ;;
  ladder-retest) shift; ladder "$@"; ( hzq run --threads 4 --name tw2-retest -- bash $ROOT/tools/tw2_job.sh retest-in ) & sleep 1; wait; note "all done" ;;
  *) sed -n 2,12p "$0"; exit 2 ;;
esac
