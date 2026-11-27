#!/usr/bin/env bash
# bundle_turn_job.sh (branch bundle-turn, RCA only): VM units.
#   bundle_turn_job.sh dbg          qmtech det s0 C1, arms off and a1 (drawing.forced_shrink), with the drawing's debug lines
#                                   (RUNPROV, OCTINODES of the JS1 bus wires, FORCEDBG, OCTIDBG, LANEDBG, ABSORBDBG, NECKDBG),
#                                   TOPO_DUMP and TOPO_PLANDUMP; stderr gzipped per arm. Copper must equal dab3's (hash).
#   bundle_turn_job.sh crop         the qmtech crop round the hole H2 and RN2 (kept copper), det s0, same debug lines
#   bundle_turn_job.sh top          dbg + crop as two hzq units, then "all done"
#   bundle_turn_job.sh pats [ARM [sets]]  the proposed bundle-turn patterns (harness.bundle_turn_patterns), det s0, PNGs
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
note() { echo "$*" >> $OUT/rc.txt; }
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
JS="774,1275,734,851,604,1027,1028,584"
dbgenv() { echo "TOPO_RUNPROV=1 TOPO_OCTINODES=$JS TOPO_FORCEDBG=1 TOPO_OCTIDBG=1 TOPO_LANEDBG=1 TOPO_ABSORBDBG=1 TOPO_NECKDBG=1 TOPO_OCTIWHYF=1"; }
arm() {  # arm NAME [sets]: qmtech det s0 C1 with the debug lines
  local a=$1; shift
  mkdir -p $OUT/$a
  env $(dbgenv) TOPO_DUMP=$OUT/$a/dump.json TOPO_PLANDUMP=$OUT/$a/plan.json TOPOR_UNRELEASED_SETTINGS=1 RAYON_NUM_THREADS=4 \
    tools/route_board.sh $QM $OUT/$a/qmtech-minimig/s0 --clock cpu --timeout 2400 $C1 "$@" --no-png \
    2> >(gzip -c > $OUT/$a/stderr.gz) > $OUT/$a/route.log 9>&-
  note "arm $a rc $?"
}
case "${1:-}" in
  dbg)
    arm off & sleep 2; arm a1 --set drawing.forced_shrink=true & wait ;;
  crop)
    N=bt_qm_js; C=$OUT/crops
    python -m harness.abstract_crop $QM --name $N --window 149.5 117.5 166.5 129.5 --out $C --no-png > $OUT/crop-gen.log 2>&1; note "crop gen rc $?"
    for a in off a1; do
      x=""; [ $a = a1 ] && x="--set drawing.forced_shrink=true"
      mkdir -p $OUT/crop/$a
      env $(dbgenv) TOPO_OCTINODES= TOPO_PLANDUMP=$OUT/crop/$a/plan.json TOPOR_UNRELEASED_SETTINGS=1 tools/route_board.sh $C/$N/problem/$N.kicad_pcb $OUT/crop/$a/$N/s0 \
        --keep-copper --set optimize.runs=1 --set optimize.rounds=1 $x > $OUT/crop/$a/route.log 2> >(gzip -c > $OUT/crop/$a/stderr.gz) 9>&-
      note "crop $a rc $?"
    done ;;
  pats)
    shift; A=${1:-main}; shift || true
    P=$OUT/pats; mkdir -p $P
    [ -d $P/boards ] || { python -m harness.bundle_turn_patterns $P/boards --drc > $P-gen.log 2>&1; note "pats gen rc $?"; }
    for d in $P/boards/*/; do
      n=$(basename $d)
      tools/route_board.sh $d/problem/$n.kicad_pcb $P/runs/$A/$n/$n/s0 --keep-copper --no-png --set optimize.runs=1 --set optimize.rounds=1 "$@" \
        > $P/$A-$n.txt 2> >(gzip -c > $P/$A-$n.stderr.gz) 9>&-
      note "pat $A $n rc $?"
    done
    python -m harness.bundle_turn_patterns_png $P/runs $P/png > $P/png.log 2>&1; note "pats png rc $?"
    note "all done" ;;
  probe)  # qmtech off (and a1) again with TOPO_OCTIPROBE on the JS1 bus forms (copper must stay = dab3)
    PR="774:152.699,120.352;155.556,123.209;161.377,123.209|774:152.699,120.352;154.964,122.617;155.469,122.617;155.779,122.927;156.068,122.927;156.350,123.209;161.377,123.209|774:153.980,122.240;154.949,123.209;156.858,123.209|774:154.587,122.240;155.046,122.698;155.695,122.698;156.044,123.046;156.696,123.046;156.858,123.209|1275:161.403,122.843;163.988,125.428;163.988,125.861;164.327,126.200|1275:161.497,122.843;163.687,125.033;163.687,125.560;164.327,126.200"
    for a in off a1; do
      x=""; [ $a = a1 ] && x="--set drawing.forced_shrink=true"
      mkdir -p $OUT/p-$a
      TOPO_OCTIPROBE="$PR" TOPO_OCTINODES=774 TOPOR_UNRELEASED_SETTINGS=1 RAYON_NUM_THREADS=4 tools/route_board.sh $QM $OUT/p-$a/qmtech-minimig/s0 --clock cpu --timeout 2400 $C1 $x --no-png \
        2> >(grep -E "^(OCTIPROBE|OCTINODE|OCTILEG|OCTIWHYF|RUNPROV)" | gzip -c > $OUT/p-$a/stderr.gz) > $OUT/p-$a/route.log 9>&- &
    done; wait; note "probe rc $?" ;;
  top)  # the hzq units of the dbg job
    T=${HZ_TAG:-bt}
    hzq run --threads 8 --name $T-dbg -- bash $ROOT/tools/bundle_turn_job.sh dbg 9>&- & sleep 3
    hzq run --threads 2 --name $T-crop -- bash $ROOT/tools/bundle_turn_job.sh crop 9>&- &
    wait; note "all done" ;;
  *) echo "usage: $0 dbg|crop|pats" >&2; exit 2 ;;
esac
