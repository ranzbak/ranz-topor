#!/usr/bin/env bash
# bundle-turn r3 step 1 (RCA of the user's qm detours): det s0 C1 on qmtech-bh26 (the user's Board Setup rules).
#   top: tip clean (metrics), tip dbg (TOPO_XRDBG + TOPO_LOOPDBG: crossing-pass groups, loop reroute tries), tip split3 (XRDBG only)
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $OUT/rc.txt; }
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
BH=scratch/boards/qmtech-bh26/qmtech-bh26.kicad_pcb
run() { local a=$1; shift; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a" "$BH" --only 0 --no-png --threads 4 $C1 "$@" > "$OUT/$a.log" 2>&1 9>&-; note "$a rc $?"; gzip -f "$OUT/$a.log"; }
case "${1:-}" in
  clean) run tip-bh ;;
  dbg) TOPO_XRDBG=1 TOPO_LOOPDBG=1 run dbg-bh ;;
  split) TOPO_XRDBG=1 run split3-bh --set optimize.crossing_split=3 ;;
  top)
    for u in clean dbg split; do hzq run --threads 4 --name bt3-$u -- bash $ROOT/tools/bundle_turn_r3_rca.sh $u 9>&- & sleep 3; done
    wait; note "all done" ;;
  *) echo "usage: $0 top|clean|dbg|split" >&2; exit 2 ;;
esac
