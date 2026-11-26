#!/usr/bin/env bash
# neckrg_job.sh (branch neck-regrow): VM units.
#   neckrg_job.sh crop ARM [sets...]   the qmtech ADV7511 crop (window of via-plan's via_crop_qm_adv17), routed with kept
#                                      copper (runs 1 + sets), TOPO_NECKDBG + TOPO_PLANDUMP, under $OUT/crop/ARM
#   neckrg_job.sh pu TAG ARM LABEL [sets]  the proposed patterns (harness.neckrg_patterns) routed as ARM, PNGs, "all done"
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
P=$OUT/pats
NAMES="neck_both_ends neck_reentry"
pats() {  # pats ARM [sets]: the proposed patterns, kept copper, runs 1 rounds 1 (det s0), then PNGs of every arm present
  local A=$1; shift
  mkdir -p $P/fixtures; [ -d $P/boards ] || { python -m harness.neckrg_patterns $P/boards --drc > $P-gen.log 2>&1; note "pats gen rc $?"
    for n in $NAMES; do python -c "import sys; from pathlib import Path; from harness.radii_patterns import model_fixture as f; f(sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]))" $n $P/boards/$n/problem/$n.kicad_pcb $P/fixtures/model-$n.json >> $P-gen.log 2>&1; note "fixture $n rc $?"; done; }
  for n in $NAMES; do
    tools/route_board.sh $P/boards/$n/problem/$n.kicad_pcb $P/runs/$A/$n/$n/s0 --keep-copper --no-png --set optimize.runs=1 --set optimize.rounds=1 "$@" > $P/$A-$n.txt 2>&1 9>&-
    note "pat $A $n rc $?"
  done
  python -m harness.neckrg_patterns_png $P/runs $P/png > $P/png.log 2>&1; note "pats png rc $?"
}
case ${1:-} in
  crop) shift; crop "$@" ;;
  both)  # both TAG ARM LABEL [BASE_TAG]: patterns (pu) and the crop (TOPO_NECKDBG) as two hzq units; BASE_TAG: copy
    # that earlier job's pattern runs (/root/runs/BASE_TAG/pats/runs: its arms + labels) first, so the PNG shows them too
    T=$2; A=$3; L=$4; gen
    if [ -n "${5:-}" ] && [ -d /root/runs/$5/pats/runs ]; then mkdir -p $P/runs; cp -r /root/runs/$5/pats/runs/. $P/runs/; note "copied arms of $5"; fi
    bash $ROOT/tools/neckrg_job.sh pu $T $A $L > $OUT/u-pu.log 2>&1 &
    hzq run --threads 2 --name $T-crop -- bash $ROOT/tools/neckrg_job.sh crop $A --set optimize.rounds=1 > $OUT/u-crop.log 2>&1 9>&- &
    wait ;;
  pats) shift; mkdir -p $P/runs; pats "$@" ;;
  pu)  # pu TAG ARM LABEL [sets]: pats as one hzq unit, then "all done"
    T=$2; A=$3; L=$4; shift 4; mkdir -p $P/runs
    python - $P/runs/labels.json "$A" "$L" <<'PY'
import json, sys, os
p, a, l = sys.argv[1:]
d = json.load(open(p)) if os.path.exists(p) else {}
d[a] = l
json.dump(d, open(p, "w"))
PY
    hzq run --threads 2 --name $T-$A -- bash $ROOT/tools/neckrg_job.sh pats $A "$@" > $OUT/u-$A.log 2>&1 9>&-
    note "unit rc $?"; note "all done" ;;
  diag)  # diag TAG: arm D (det: runs 1, rounds 1) and arm V (runs 1, default rounds: via-plan vp1's arm M), main + debug prints
    T=$2; gen
    hzq run --threads 2 --name $T-D -- bash $ROOT/tools/neckrg_job.sh crop D --set optimize.rounds=1 > $OUT/u-D.log 2>&1 9>&- &
    hzq run --threads 2 --name $T-V -- bash $ROOT/tools/neckrg_job.sh crop V > $OUT/u-V.log 2>&1 9>&- &
    wait; note "all done" ;;
  *) echo "usage: neckrg_job.sh crop ARM [sets]|diag TAG"; exit 2 ;;
esac
