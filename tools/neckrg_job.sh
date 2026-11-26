#!/usr/bin/env bash
# neckrg_job.sh (branch neck-regrow): VM units.
#   neckrg_job.sh crop ARM [sets...]   the qmtech ADV7511 crop (window of via-plan's via_crop_qm_adv17), routed with kept
#                                      copper (runs 1 + sets), TOPO_NECKDBG + TOPO_PLANDUMP, under $OUT/crop/ARM
#   neckrg_job.sh diag TAG             arms D (rounds 1) and V (default rounds) as two hzq units, then "all done"
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
note() { echo "$*" >> $OUT/rc.txt; }
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
C=$OUT/crops
N=neckrg_qm_adv17
gen() { [ -d $C/$N ] || { python -m harness.abstract_crop $QM --name $N --window 135.5 69.5 141.5 77.0 --out $C --no-png > $OUT/crop-gen.log 2>&1; note "crop gen rc $?"; }; }
crop() {
  local A=$1; shift
  gen
  mkdir -p $OUT/crop/$A
  TOPO_NECKDBG=1 TOPO_PLANDUMP=$OUT/crop/$A/plan.json tools/route_board.sh $C/$N/problem/$N.kicad_pcb $OUT/crop/$A/$N/s0 --keep-copper --no-png \
    --set optimize.runs=1 "$@" > $OUT/crop/$A/route.log 2>&1 9>&-
  note "crop $A rc $?"
}
case ${1:-} in
  crop) shift; crop "$@" ;;
  diag)  # diag TAG: arm D (det: runs 1, rounds 1) and arm V (runs 1, default rounds: via-plan vp1's arm M), main + debug prints
    T=$2; gen
    hzq run --threads 2 --name $T-D -- bash $ROOT/tools/neckrg_job.sh crop D --set optimize.rounds=1 > $OUT/u-D.log 2>&1 9>&- &
    hzq run --threads 2 --name $T-V -- bash $ROOT/tools/neckrg_job.sh crop V > $OUT/u-V.log 2>&1 9>&- &
    wait; note "all done" ;;
  *) echo "usage: neckrg_job.sh crop ARM [sets]|diag TAG"; exit 2 ;;
esac
