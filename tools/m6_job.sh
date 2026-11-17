#!/usr/bin/env bash
# m6_job.sh: 6b/M6 phase 1 (branch m6-fire): what the method draws without the drawing-side stand-ins.
#   m6_job.sh main                 generate the patterns, then one hzq unit per config pair + bornhack (seqOFF)
#   m6_job.sh unit T CFG...        (inside a unit) every board + pattern task of CFG..., T at a time (1 thread each)
#   m6_job.sh task CFG KIND NAME   (inside a unit) one run; KIND = b (public board, harness.score_runs) | p (pattern)
# Configs (seed 0, optimize.rounds=1 runs=1, 1 thread; TOPO_M6_OFF = engine/src/topo/mod.rs m6_off, debug only):
#   seqD default | seqOFF everything off | seqForce force_via+force_wire+force_join | seqRoom planned_room weight 0 |
#   seqPull drawing.pull_taut=false | seqFollow follow | seqLadder ladder | erbsD realise=erbs | erbsOFF erbs + everything off
# Patterns: fit (7) / via (6 approved) / radii (4 approved) from their generators (no PNG, no DRC of the reference);
# untangle (5) + fanout (5) from m6-job/boards (copies of the approved scratch/ boards; their generators are not in git).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
BOARDS="qmtech esp wiccon eprom"
VIA="via_pair_redundant via_pair_slide via_force_relocate branch_point_relocate branch_point_into_pad via_size_rule"
cfg() {  # sets OFFV and SETS for config $1
  OFFV=""; SETS=()
  local all=(--set drawing.pull_taut=false --set 'factors.weight={"planned_room": 0}')
  case $1 in
    seqD) ;;
    seqOFF) OFFV=all; SETS=("${all[@]}") ;;
    seqForce) OFFV=force_via,force_wire,force_join ;;
    seqRoom) SETS=(--set 'factors.weight={"planned_room": 0}') ;;
    seqPull) SETS=(--set drawing.pull_taut=false) ;;
    seqFollow) OFFV=follow ;;
    seqLadder) OFFV=ladder ;;
    erbsD) SETS=(--set drawing.realise=erbs) ;;
    erbsOFF) OFFV=all; SETS=(--set drawing.realise=erbs "${all[@]}") ;;
    *) echo "m6_job: unknown config $1" >&2; exit 2 ;;
  esac
}
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; bornhack) echo bornhack2026;; esac; }
patterns() { ls -d $R/pat/*/ | xargs -n1 basename; }
task() {
  local c=$1 k=$2 n=$3; cfg $c
  export TOPO_M6_OFF="$OFFV" RAYON_NUM_THREADS=1
  mkdir -p $R/$c/$k
  if [ $k = b ]; then
    python -m harness.score_runs $R/$c/b/$n "$(board_path $n)" --only 0 --no-png --threads 1 \
      --set optimize.rounds=1 --set optimize.runs=1 "${SETS[@]}" > $R/$c/b-$n.log 2>&1
  else
    PYTHONPATH=$ROOT:$ROOT/plugin python -P harness/route_board.py $R/pat/$n/problem/$n.kicad_pcb $R/$c/p/$n --no-png \
      --set optimize.rounds=1 --set optimize.runs=1 "${SETS[@]}" > $R/$c/p-$n.log 2>&1
  fi
  note "task $c $k $n rc $? off=[$OFFV] sets=[${SETS[*]}]"
}
case ${1:-} in
  task) shift; task "$@" ;;
  unit) shift; T=$1; shift
    { for n in $BOARDS; do for c in "$@"; do echo "$c b $n"; done; done
      for n in $(patterns); do for c in "$@"; do echo "$c p $n"; done; done; } | xargs -P $T -L1 bash $ROOT/tools/m6_job.sh task ;;
  main)
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
    note "gen rc $?"
    cp -r m6-job/boards/* $R/pat/; note "patterns: $(patterns | tr '\n' ' ')"
    u() { local n=$1 t=$2; shift 2; ( hzq run --threads $t --name "$n" -- bash $ROOT/tools/m6_job.sh "$@" > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & sleep 2; }
    u m6-bh 1 task seqOFF b bornhack
    u m6-u1 8 unit 8 seqD seqOFF
    u m6-u2 8 unit 8 erbsD erbsOFF
    u m6-u3 8 unit 8 seqForce seqRoom
    u m6-u4 8 unit 8 seqPull seqFollow
    u m6-u5 4 unit 4 seqLadder
    wait; note "all done" ;;
  *) sed -n 2,12p "$0"; exit 2 ;;
esac
