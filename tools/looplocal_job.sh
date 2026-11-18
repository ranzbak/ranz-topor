#!/usr/bin/env bash
# looplocal_job.sh (branch loop-local, docs/dev/design/loop-local.md): copper and loop timing runs.
#   looplocal_job.sh main CFG...         queue one hzq unit (1 thread) per board and pattern for each CFG, then wait
#   looplocal_job.sh timing CFG...       exclusive timing: qmtech + esp per CFG, all at once (1 thread each), no hzq
#   looplocal_job.sh full CFG...         one hzq unit per board (eprom wiccon esp qmtech bornhack) per CFG, the loop's
#                                        default cycles (quality ladder at 100 % drawn), then wait
#   looplocal_job.sh tests               engine tests + clippy + plugin config tests + Step B mutants (one hzq unit, 4 threads)
#   looplocal_job.sh both                tip: main G SH + full G L + tests in one session
#   looplocal_job.sh task CFG KIND NAME  one run (inside a unit); KIND b = board, p = pattern, f = board, default cycles
# CFG: D (defaults), NOSC (TOPO_NOSCACHE=1: Step A cache off), G (optimize.loop_judge="global"), L (="local"),
#      SH (local + shadow check every 8th re-lay).
# Runs: seed 0, optimize.rounds=1 (one TopoR loop cycle) runs=1, 1 thread, --clock cpu --timeout 2400, TOPO_TIMES=1.
# Boards: the public set eprom, wiccon, esp, qmtech, bornhack; patterns fit (all) + via (approved 6) + radii (all).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
VIA="via_pair_redundant via_pair_slide via_force_relocate branch_point_relocate branch_point_into_pad via_size_rule"
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; bornhack) echo bornhack2026;; esac; }
cfg() {
  SETS=(); ENVS=()
  case $1 in
    D) ;;
    NOSC) ENVS=(TOPO_NOSCACHE=1) ;;
    G) SETS=(--set 'optimize.loop_judge="global"') ;;
    L) SETS=(--set 'optimize.loop_judge="local"') ;;
    SH) SETS=(--set 'optimize.loop_judge="local"' --set optimize.loop_shadow=8) ;;
    LC) SETS=(--set 'optimize.loop_judge="local"'); ENVS=(TOPO_LOCALCHECK=1) ;;
    *) echo "looplocal_job: unknown config $1" >&2; exit 2 ;;
  esac
}
task() {
  local c=$1 k=$2 n=$3; cfg $c
  local o=$R/$c/$k/$n; mkdir -p $o
  local src; if [ $k = p ]; then src=$R/pat/$n/problem/$n.kicad_pcb; else src=$(board_path $n); fi
  local ROUNDS=(--set optimize.rounds=1); [ $k = f ] && ROUNDS=()
  local t0=$(date +%s)
  env RAYON_NUM_THREADS=1 TOPO_TIMES=1 "${ENVS[@]}" tools/route_board.sh "$src" $o --clock cpu --timeout 2400 --no-png \
    "${ROUNDS[@]}" --set optimize.runs=1 "${SETS[@]}" > $R/$c/$k-$n.log 2>&1
  note "task $c $k $n rc $? wall $(( $(date +%s) - t0 )) s"
  rm -rf $o/work $o/*/work 2>/dev/null
}
gen() {
  [ -d $R/pat ] && return
  mkdir -p $R/pat
  PYTHONPATH=$ROOT:$ROOT/plugin python - "$R/pat" $VIA > $R/gen.log 2>&1 <<'EOF'
import sys
from pathlib import Path
from harness import fit_patterns, via_patterns, radii_patterns
out = Path(sys.argv[1])
for n in fit_patterns.PATTERNS:
    fit_patterns.generate(n, out, png=False)
for n in sys.argv[2:]:
    via_patterns.generate(n, out, png=False, drc=False)
for n in radii_patterns.PATTERNS:
    radii_patterns.generate(n, out, png=False, drc=False)
EOF
  note "gen rc $? patterns: $(ls $R/pat | tr '\n' ' ')"
}
mut() {  # name file test-args... ; the python snippet on stdin edits the file
  local n=$1 f=$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  ( cd $ROOT/engine && cargo test --release "$@" > $R/mut-$n.log 2>&1 ); local rc=$?
  mv $f.orig $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
sub() { cat <<EOF
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF
}
tests() {
  cd $ROOT/engine
  { timeout 5400 cargo test --release --no-fail-fast 2>&1; echo "cargo test rc $?"
    cargo clippy --release --all-targets 2>&1 | grep -E "^ +--> " | sort | uniq; echo "clippy rc ${PIPESTATUS[0]}"
  } > $R/tests.log
  grep -E "test result|rc |FAILED|panicked" $R/tests.log | while read -r l; do note "tests: $l"; done
  cd $ROOT; CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_messages.py > $R/pytest.log 2>&1
  note "pytest rc $?: $(tail -1 $R/pytest.log)"
  local L=$ROOT/engine/src/topo/layers.rs
  sub 'pi: (1 - lo.clamp(0, 1)) as u8, k, s }, i_o.map(|i| (o, i))));' 'pi: lo.clamp(0, 1) as u8, k, s }, i_o.map(|i| (o, i))));' | mut b1_two_orient $L --test loop_local
  sub 'pi: (1 - lo.clamp(0, 1)) as u8, k, s }, i_o.map(|i| (o, i))));' 'pi: (1 - lo.clamp(0, 1)) as u8, k, s }, None));' | mut b2_two_link $L --test loop_local
  sub 'let m = tm & !(1u64 << lo.clamp(0, 63));' 'let m = tm;' | mut b3_n_mask $L --test loop_local
  sub 'unl.push(((a, b), h.prev.unlayerable[i]));' '' | mut b4_held_unl $L --test loop_local
  sub 'out[st.passes[q].wire] = true;' 'let _ = q;' | mut b5_neigh $L --test loop_local
  note "tests done"
}
case ${1:-} in
  task) shift; task "$@" ;;
  small) for n in eprom wiccon; do task $2 b $n; done; for n in $(ls $R/pat); do task $2 p $n; done ;;
  main)
    shift; gen
    for c in "$@"; do  # 4 units per config: bornhack, qmtech, esp, and the small ones in one
      for n in bornhack qmtech esp; do ( hzq run --threads 1 --name "$c-$n" -- bash $ROOT/tools/looplocal_job.sh task $c b $n; ) & sleep 1; done
      ( hzq run --threads 1 --name "$c-small" -- bash $ROOT/tools/looplocal_job.sh small $c ) & sleep 1
    done
    wait; note "all done" ;;
  full)
    shift
    for c in "$@"; do for n in bornhack qmtech esp wiccon eprom; do ( hzq run --threads 1 --name "$c-f-$n" -- bash $ROOT/tools/looplocal_job.sh task $c f $n; ) & sleep 1; done; done
    wait; note "all done" ;;
  tests) hzq run --threads 4 --name tests -- bash $ROOT/tools/looplocal_job.sh tests-in ;;
  tests-in) tests ;;
  shadow)  # SH again on eprom esp qmtech (shadow compares changes after - before)
    for n in eprom esp qmtech; do ( hzq run --threads 1 --name "SH-$n" -- bash $ROOT/tools/looplocal_job.sh task SH b $n; ) & sleep 1; done
    wait; note "all done" ;;
  check)  # LC (local + TOPO_LOCALCHECK) rounds=1 on bornhack, qmtech, esp; the L ladder again (fixed rev)
    for n in bornhack qmtech esp; do ( hzq run --threads 1 --name "LC-$n" -- bash $ROOT/tools/looplocal_job.sh task LC b $n; ) & sleep 1; done
    for n in bornhack qmtech esp wiccon eprom; do ( hzq run --threads 1 --name "L-f-$n" -- bash $ROOT/tools/looplocal_job.sh task L f $n; ) & sleep 1; done
    wait; note "all done" ;;
  both)  # SH = L's copper (the shadow only observes), so L needs no own rounds=1 units
    gen
    for c in G SH; do
      for n in bornhack qmtech esp; do ( hzq run --threads 1 --name "$c-$n" -- bash $ROOT/tools/looplocal_job.sh task $c b $n; ) & sleep 1; done
      ( hzq run --threads 1 --name "$c-small" -- bash $ROOT/tools/looplocal_job.sh small $c ) & sleep 1
    done
    for c in G L; do for n in bornhack qmtech esp wiccon eprom; do ( hzq run --threads 1 --name "$c-f-$n" -- bash $ROOT/tools/looplocal_job.sh task $c f $n; ) & sleep 1; done; done
    ( hzq run --threads 4 --name tests -- bash $ROOT/tools/looplocal_job.sh tests-in ) & sleep 1
    wait; note "all done" ;;
  timing)
    shift
    for c in "$@"; do for n in qmtech esp; do bash $ROOT/tools/looplocal_job.sh task $c b $n & sleep 1; done; done
    wait; note "all done" ;;
  *) sed -n 2,9p "$0"; exit 2 ;;
esac
