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
TESTS="--test topo_neck_regrow --test topo_neck_depth --test topo_dvn --test topo_fit --test topo_pad_exit --test topo_q14 --test topo_mc --test topo_compass --lib"
xunit() {  # clippy, targeted release tests, the crop tool's tests, arm and source mutants (each must fail a test)
  cd $ROOT/engine
  rustup component add clippy > $OUT/rustup.log 2>&1
  cargo clippy --release --all-targets > $OUT/clippy.log 2>&1 9>&-; note "clippy rc $?"
  RUST_TEST_THREADS=4 cargo test --release --no-fail-fast $TESTS > $OUT/tests.log 2>&1 9>&-; note "tests rc $?"
  cd $ROOT; python -m pytest -q -p no:cacheprovider tests/test_abstract_crop.py > $OUT/pytest-crop.log 2>&1; note "pytest crop rc $?"; cd $ROOT/engine
  for a in nrg_join nrg_contig; do
    TOPO_FD45_OFF=$a cargo test --release --test topo_neck_regrow > $OUT/mut-$a.log 2>&1 9>&-; note "mutant arm $a rc $? (want != 0)"
  done
  M=$OUT/mut; mkdir -p $M; cp -r $ROOT/engine $M/; cp $ROOT/Cargo.* $M/ 2>/dev/null
  mut() {  # mut NAME FILE SED_EXPR TESTARGS...
    local n=$1 f=$2 e=$3; shift 3
    cp $ROOT/engine/$f $M/engine/$f; sed -i "$e" $M/engine/$f
    if cmp -s $ROOT/engine/$f $M/engine/$f; then note "mutant $n NOT APPLIED"; return; fi
    (cd $M/engine && CARGO_TARGET_DIR=/root/nrg-mut-target cargo test --release "$@" > $OUT/mut-$n.log 2>&1 9>&-); note "mutant $n rc $? (want != 0)"
    cp $ROOT/engine/$f $M/engine/$f
  }
  mut ownpad src/topo/state.rs 's/filter(|\&k| !inside(k))/filter(|\&k| { let _ = inside(k); true })/' --test topo_neck_regrow
  mut taut src/topo/state.rs 's/if too_close(\&self.taut_prefix(/if false \&\& too_close(\&self.taut_prefix(/' --test topo_neck_regrow
  mut axis src/topo/state.rs 's/pad_axis(\&me.poly, me.center, pts\[1\] - pts\[0\])/pad_axis(\&me.poly, me.center, pts[1] - pts[0]).filter(|_| false)/' --test topo_neck_regrow
  mut supply src/topo/core.rs 's/ \&\& w > dflt_w + 0.5//' --lib whole_supply
  mut supdesc src/topo/factors/necking.rs 's/, the whole wire (both ends/ (both ends/' --lib describe_names
}
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
bunit() { local a=$1 n=$2 pcb=$3; python -m harness.score_runs "$OUT/$a-$n" "$pcb" --only 0 --no-png --threads 4 $C1 > "$OUT/$a-$n.log" 2>&1 9>&-; note "$a-$n rc $?"; }
case ${1:-} in
  xunit) xunit ;;
  bunits) shift; EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1); ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
    WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
    for x in "$@"; do case $x in qm) bunit C1 qm $QM ;; ep) bunit C1 ep $EP ;; esp) bunit C1 esp $ESP ;; wic) bunit C1 wic $WIC ;; esac; done ;;
  final)  # final TAG: xunit + two board units (det s0 C1, as dvn7s), 3 hzq units
    T=$2
    hzq run --threads 6 --name $T-x -- bash $ROOT/tools/neckrg_job.sh xunit > $OUT/u-x.log 2>&1 9>&- &
    hzq run --threads 4 --name $T-b1 -- bash $ROOT/tools/neckrg_job.sh bunits qm ep > $OUT/u-b1.log 2>&1 9>&- &
    hzq run --threads 4 --name $T-b2 -- bash $ROOT/tools/neckrg_job.sh bunits esp wic > $OUT/u-b2.log 2>&1 9>&- &
    wait; note "all done" ;;
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
