#!/usr/bin/env bash
# looplocal_job.sh (branch loop-local, docs/dev/design/loop-local.md): copper and loop timing runs.
#   looplocal_job.sh main CFG...         queue one hzq unit (1 thread) per board and pattern for each CFG, then wait
#   looplocal_job.sh timing CFG...       exclusive timing: qmtech + esp per CFG, all at once (1 thread each), no hzq
#   looplocal_job.sh task CFG KIND NAME  one run (inside a unit); KIND b = board, p = pattern
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
    *) echo "looplocal_job: unknown config $1" >&2; exit 2 ;;
  esac
}
task() {
  local c=$1 k=$2 n=$3; cfg $c
  local o=$R/$c/$k/$n; mkdir -p $o
  local src; if [ $k = b ]; then src=$(board_path $n); else src=$R/pat/$n/problem/$n.kicad_pcb; fi
  local t0=$(date +%s)
  env RAYON_NUM_THREADS=1 TOPO_TIMES=1 "${ENVS[@]}" tools/route_board.sh "$src" $o --clock cpu --timeout 2400 --no-png \
    --set optimize.rounds=1 --set optimize.runs=1 "${SETS[@]}" > $R/$c/$k-$n.log 2>&1
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
  timing)
    shift
    for c in "$@"; do for n in qmtech esp; do bash $ROOT/tools/looplocal_job.sh task $c b $n & sleep 1; done; done
    wait; note "all done" ;;
  *) sed -n 2,9p "$0"; exit 2 ;;
esac
