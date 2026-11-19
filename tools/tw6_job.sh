#!/usr/bin/env bash
# tw6_job.sh (branch tw6-via-sites, docs/dev/design/tw6-via-sites.md): TW6 runs on the VM.
#   tw6_job.sh base        baseline of the via patterns on main's engine (this branch = main + this script + boards):
#                          per pattern rounds=1 (K) and the product defaults (D), runs=1, seed 0, harness.score_runs;
#                          PNGs only for the two TW6 acceptance patterns (planned_crossing_window, interleaved_boundary).
#   tw6_job.sh one <cfg> <pattern> [png]
# Pattern boards: tw6-job/pat/<n>/problem (copied from loopdiv-dbg 61c65793 = scratch/untangle-patterns, harness/via_patterns.py).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
PATS="planned_crossing_window interleaved_boundary via_force_relocate via_pair_slide via_pair_redundant via_size_rule branch_point_relocate branch_point_into_pad"
one() {
  local c=$1 n=$2 png=${3:-}
  local SETS=(--set optimize.runs=1) PNG=(--no-png)
  case $c in K) SETS+=(--set optimize.rounds=1) ;; D) ;; *) note "unknown cfg $c"; return ;; esac
  [ -n "$png" ] && PNG=()
  local t0=$(date +%s)
  python -m harness.score_runs $R/$c/$n $ROOT/tw6-job/pat/$n/problem/$n.kicad_pcb --only 0 "${PNG[@]}" --threads 1 "${SETS[@]}" > $R/$c-$n.log 2>&1 9>&-
  note "run $c $n rc $? wall $(( $(date +%s) - t0 )) s"
}
case ${1:-} in
  one) one "$2" "$3" "${4:-}" ;;
  base)
    for c in K D; do
      ( hzq run --threads 2 --name tw6-$c -- bash -c "for n in $PATS; do p=; case \$n in planned_crossing_window|interleaved_boundary) p=png;; esac; bash $ROOT/tools/tw6_job.sh one $c \$n \$p; done" > $R/u-$c.log 2>&1; note "unit $c rc $?" ) &
    done
    wait; note "all done" ;;
  *) echo "usage: $0 base | one <cfg> <pattern> [png]"; exit 2 ;;
esac
