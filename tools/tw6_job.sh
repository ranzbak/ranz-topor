#!/usr/bin/env bash
# tw6_job.sh (branch tw6-via-sites, docs/dev/design/tw6-via-sites.md): TW6 runs on the VM.
#   tw6_job.sh base        baseline of the via patterns on main's engine (this branch = main + this script + boards):
#                          per pattern rounds=1 (K) and the product defaults (D), runs=1, seed 0, harness.score_runs;
#                          PNGs only for the two TW6 acceptance patterns (planned_crossing_window, interleaved_boundary).
#   tw6_job.sh one <cfg> <pattern> [png]
#   tw6_job.sh round1      round 1 (a) + (b): the PROPOSED patterns (harness/tw6_patterns.py: boards, KiCad DRC of both
#                          boards, routed on main's engine under K and D through the plugin flow, PNGs + summary.json) and
#                          the dump of planned_crossing_window (K and D, TOPO_VIADBG + TOPO_VIAREL: VIAMISS / VIAREL lines)
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
pat() {
  # one PROPOSED pattern routed through the plugin flow (route_board.sh) under cfg K | D
  local c=$1 n=$2 P=$R/tw6p
  local A=(--set optimize.runs=1)
  case $c in K) A+=(--set optimize.rounds=1) ;; D) ;; esac
  case $n in lead_via_end) A+=(--keep-copper --set search.dangling_lead_mm=1.5) ;; esac
  TOPOR_UNRELEASED_SETTINGS=1 TOPO_VIADBG=1 TOPO_KEPTDBG=1 tools/route_board.sh $P/boards/$n/problem/$n.kicad_pcb \
    $P/runs/$c/$n/$n/s0 "${A[@]}" > $P/$c-$n.txt 2>&1 9>&-
  note "pat $c $n rc $?"
}
case ${1:-} in
  one) one "$2" "$3" "${4:-}" ;;
  pat) pat "$2" "$3" ;;
  repng) python -m harness.tw6_patterns_png /root/runs/$2/tw6p/runs $R/tw6p/png > $R/tw6p-png.log 2>&1; note "png rc $?"; note "all done" ;;
  round1)
    ( hzq run --threads 2 --name tw6-dump -- bash -c "for c in K D; do TOPO_VIADBG=1 TOPO_VIAREL=1 bash $ROOT/tools/tw6_job.sh one \$c planned_crossing_window png; done" > $R/u-dump.log 2>&1; note "unit dump rc $?" ) &
    ( hzq run --threads 2 --name tw6-pat -- bash -c "python -m harness.tw6_patterns $R/tw6p/boards --drc > $R/tw6p-gen.log 2>&1; echo gen rc \$? >> $R/rc.txt; for n in site_or_stay lead_via_end; do for c in K D; do bash $ROOT/tools/tw6_job.sh pat \$c \$n; done; done; python -m harness.tw6_patterns_png $R/tw6p/runs $R/tw6p/png > $R/tw6p-png.log 2>&1; echo png rc \$? >> $R/rc.txt" > $R/u-pat.log 2>&1; note "unit pat rc $?" ) &
    wait; note "all done" ;;
  base)
    for c in K D; do
      ( hzq run --threads 2 --name tw6-$c -- bash -c "for n in $PATS; do p=; case \$n in planned_crossing_window|interleaved_boundary) p=png;; esac; bash $ROOT/tools/tw6_job.sh one $c \$n \$p; done" > $R/u-$c.log 2>&1; note "unit $c rc $?" ) &
    done
    wait; note "all done" ;;
  *) echo "usage: $0 base | round1 | one <cfg> <pattern> [png] | pat <cfg> <pattern> | repng <tag>"; exit 2 ;;
esac
