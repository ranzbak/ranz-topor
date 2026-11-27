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
  cd $ROOT; python -m pytest -q -p no:cacheprovider tests/test_abstract_crop.py tests/test_engine_surface.py tests/test_docs_links.py tests/plugin/test_config_docs.py > $OUT/pytest-crop.log 2>&1; note "pytest crop rc $?"; cd $ROOT/engine
  for a in ${XARMS:-nrg_join nrg_contig nrg_relax nrg_near nrg_edge nrg_ownpad nrg_taut nrg_axis}; do
    TOPO_FD45_OFF=$a cargo test --release --test topo_neck_regrow --test topo_dvn --lib > $OUT/mut-$a.log 2>&1 9>&-; note "mutant arm $a rc $? (want != 0)"
  done
  M=$OUT/mut; mkdir -p $M; cp -r $ROOT/engine $M/; cp $ROOT/Cargo.* $M/ 2>/dev/null
  mut() {  # mut NAME FILE SED_EXPR TESTARGS...
    local n=$1 f=$2 e=$3; shift 3
    cp $ROOT/engine/$f $M/engine/$f; sed -i "$e" $M/engine/$f
    if cmp -s $ROOT/engine/$f $M/engine/$f; then note "mutant $n NOT APPLIED"; return; fi
    (cd $M/engine && CARGO_TARGET_DIR=/root/nrg-mut-target cargo test --release "$@" > $OUT/mut-$n.log 2>&1 9>&-); note "mutant $n rc $? (want != 0)"
    cp $ROOT/engine/$f $M/engine/$f
  }
  if [ -n "${R6MUT:-}" ]; then  # r6: source mutants of the lines this round changed only
    mut fitcmp src/topo/draw.rs 's/v < b.0 || (v == b.0 \&\& l < b.1 - 1.0)/l < b.1 - 1.0/' --lib topo::draw
    mut monotie src/topo/draw.rs 's/let pick = if cost(\&pre) <= cost(\&suf)/let pick = if cost(\&pre) < cost(\&suf)/' --lib topo::draw
    mut monoany src/topo/draw.rs 's/pick.iter().zip(ws).any(|(a, b)| \*a < b - 0.5)/pick.iter().zip(ws).any(|(a, b)| *a < b + 0.5)/' --lib topo::draw
    mut stretchadj src/topo/draw.rs 's/Some(s) if s.1 + 1 == k => {/Some(s) => {/' --lib topo::draw
    mut necksrec src/topo/draw.rs 's/if new.is_some() || st.len() != out.necks.len() {/if new.is_some() {/' --lib topo::draw
    mut neckpos src/topo/draw.rs 's/(if i > 0 \&\& j + 1 == n { at(j, false) } else { at(i, true) }, wn, l)/(at(i, true), wn, l)/' --lib topo::draw
    mut monopair src/topo/draw.rs 's/ || self.model.rules.pair_disc(wi.net) || super::octi::fd45_off("nrg_mono")/ || super::octi::fd45_off("nrg_mono")/' --lib topo::draw
    mut monofrompad src/topo/draw.rs 's/if !self.opts.neck_from_pad || self.widths.is_some()/if self.widths.is_some()/' --lib topo::draw
    mut monoline src/topo/core.rs 's/(s.mono_wires > 0).then/(s.mono_wires >= 0).then/' --lib mono_line
    return
  fi
  mut ownpad src/topo/state.rs 's/(ownpad || !inside(k))/true/' --test topo_neck_regrow
  mut taut src/topo/state.rs 's/        if tc.is_none() {/        if false \&\& tc.is_none() {/' --test topo_neck_regrow
  mut axis src/topo/state.rs 's/pad_axis(\&me.poly, me.center, pts\[1\] - pts\[0\])/pad_axis(\&me.poly, me.center, pts[1] - pts[0]).filter(|_| false)/' --test topo_neck_regrow
  mut supply src/topo/core.rs 's/ \&\& w > dflt_w + 0.5//' --lib whole_supply
  mut supdesc src/topo/factors/necking.rs 's/, the whole wire (both ends/ (both ends/' --lib describe_names
}
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
bunit() { local a=$1 n=$2 pcb=$3; python -m harness.score_runs "$OUT/$a-$n" "$pcb" --only 0 --no-png --threads 4 $C1 > "$OUT/$a-$n.log" 2>&1 9>&-; note "$a-$n rc $?"; }
case ${1:-} in
  xunit) xunit ;;
  r6x)  # r6: clippy, targeted tests, the r6 arms + source mutants only, the touched pytest files
    XARMS="nrg_fitslot nrg_fitcheck nrg_trimcheck nrg_mono nrg_pairjoin" R6MUT=1 xunit
    cd $ROOT; CI=1 python -m pytest -q -p no:cacheprovider tests/plugin/test_postrun.py tests/plugin/test_messages.py tests/test_abstract_crop.py > $OUT/pytest-r6.log 2>&1; note "pytest r6 rc $?" ;;
  r6pats)  # r6: the patterns at the default (T) and with [search] exit_taut on (taut), PNGs; then both with TOPO_NECKDBG
    mkdir -p $P/runs; python - $P/runs/labels.json <<'PY'
import json, sys
json.dump({"T": "tip (exit_taut false)", "taut": "tip, exit_taut true"}, open(sys.argv[1], "w"))
PY
    pats T; pats taut --set search.exit_taut=true
    for a in T taut; do for n in $NAMES; do d=$OUT/patdbg/$a/$n; mkdir -p $d
      ( [ $a = taut ] && set -- --set search.exit_taut=true || set --; TOPO_NECKDBG=1 tools/route_board.sh $P/boards/$n/problem/$n.kicad_pcb $d/s0 --keep-copper --no-png --set optimize.runs=1 --set optimize.rounds=1 "$@" > $d/route.log 2>&1 9>&- ); note "patdbg $a $n rc $?"
    done; done ;;
  dbg2)  # the verifier's pending debug bucket 2 + doc tests (vfy-dvn6 split)
    B=scratch/boards/vfy-dvn6; a=$(python3 $B/dbgsplit.py $B/dbgtimes.txt 3 2 2> $OUT/dbg2.split)
    CARGO_TARGET_DIR=/root/nrg6-dbg-target RUST_TEST_THREADS=4 cargo test --workspace --no-fail-fast $a > $OUT/cargo-debug2.log 2>&1 9>&-; note "debug2 rc $?"
    CARGO_TARGET_DIR=/root/nrg6-dbg-target cargo test --workspace --doc > $OUT/cargo-debugDoc.log 2>&1 9>&-; note "debugDoc rc $?" ;;
  pyfull) CI=1 python -m pytest -q -rs -p no:cacheprovider > $OUT/pytest-full.log 2>&1 9>&-; note "pytest full rc $?" ;;
  r6)  # r6 TAG: 5 hzq units, the long ones first
    T=$2
    hzq run --threads 4 --name $T-dbg2 -- bash $ROOT/tools/neckrg_job.sh dbg2 > $OUT/u-dbg2.log 2>&1 9>&- & sleep 3
    hzq run --threads 4 --name $T-py -- bash $ROOT/tools/neckrg_job.sh pyfull > $OUT/u-py.log 2>&1 9>&- & sleep 3
    hzq run --threads 4 --name $T-b1 -- bash $ROOT/tools/neckrg_job.sh bunits qm ep > $OUT/u-b1.log 2>&1 9>&- & sleep 3
    hzq run --threads 4 --name $T-b2 -- bash $ROOT/tools/neckrg_job.sh bunits esp wic > $OUT/u-b2.log 2>&1 9>&- & sleep 3
    hzq run --threads 4 --name $T-x -- bash -c "bash $ROOT/tools/neckrg_job.sh r6x; bash $ROOT/tools/neckrg_job.sh r6pats" > $OUT/u-x.log 2>&1 9>&- &
    wait; find $OUT -name '*.log' -size +20M -exec gzip -9 {} + 2>/dev/null; note "all done" ;;
  espdbg)  # esp det s0 C1 with TOPO_NECKDBG (the plan's exit decisions)
    ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; mkdir -p $OUT/espdbg
    TOPO_NECKDBG=1 tools/route_board.sh $ESP $OUT/espdbg/s0 --no-png --set optimize.runs=1 --set optimize.rounds=1 > $OUT/espdbg/route.log 2>&1 9>&-; note "espdbg rc $?"
    grep -E "EXITTIGHT|EXITTAUT|PLANNECK" $OUT/espdbg/route.log | sort | uniq -c | sort -rn | head -400 > $OUT/espdbg/exit.txt; gzip -9 $OUT/espdbg/route.log ;;
  r3esp)  # r3: esp det s0 with the own-pad / taut / axis arms off each; then TOPO_NECKDBG + plan dumps of the tip and of main's exit test
    ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
    for a in nrg_ownpad nrg_taut nrg_axis; do TOPO_FD45_OFF=$a bunit A$a esp $ESP; done
    for a in tip nrg_exit,nrg_join; do d=$OUT/dbg-${a//,/+}; mkdir -p $d
      ( [ $a = tip ] || export TOPO_FD45_OFF=$a; TOPO_NECKDBG=1 TOPO_PLANDUMP=$d/plan.json tools/route_board.sh $ESP $d/s0 --no-png --set optimize.runs=1 --set optimize.rounds=1 > $d/route.log 2>&1 9>&- ); note "dbg $a rc $?"
      grep -E "EXITTIGHT|EXITTAUT|PLANNECK" $d/route.log | sort | uniq -c | sort -rn | head -600 > $d/exit.txt; gzip -9 $d/route.log; gzip -9 $d/plan.json 2>/dev/null
    done ;;
  r4arms)  # r4: which regrow fix moves esp / qm copper (nrg_join, nrg_contig off each), then the arm + source mutants
    ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
    for a in nrg_join nrg_contig; do TOPO_FD45_OFF=$a bunit A$a esp $ESP; TOPO_FD45_OFF=$a bunit A$a qm $QM; done
    xunit ;;
  r5qm)  # r5: qm det s0 with TOPO_NECKDBG + TOPO_OCTIDBG + plan dump, tip and the nrg_join arm (= main's drawing)
    for a in tip nrg_join; do d=$OUT/qdbg-$a; mkdir -p $d
      ( [ $a = tip ] || export TOPO_FD45_OFF=$a; TOPO_NECKDBG=1 TOPO_OCTIDBG=1 TOPO_PLANDUMP=$d/plan.json tools/route_board.sh $QM $d/s0 --no-png --set optimize.runs=1 --set optimize.rounds=1 > $d/route.log 2>&1 9>&- ); note "qdbg $a rc $?"
      gzip -9 $d/route.log
    done ;;
  r3qm)  # r3: the qmtech crop at the tip (T) + qm det s0 C1 with main's exit test (nrg_exit + nrg_join)
    crop T --set optimize.rounds=1
    TOPO_FD45_OFF=nrg_exit,nrg_join bunit Anrg_exit+nrg_join qm $QM ;;
  esparms2)  # esp det s0: main's exit test (nrg_exit), plus the drawing join off, plus everything of this branch off
    ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
    for a in nrg_exit nrg_exit,nrg_join nrg_exit,nrg_join,nrg_contig; do TOPO_FD45_OFF=$a bunit A${a//,/+} esp $ESP; done ;;
  esparms)  # esp det s0 C1 with one fix arm off each (which change moves esp's DRC)
    ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
    for a in ${ARMS:-nrg_join nrg_relax nrg_near nrg_edge}; do TOPO_FD45_OFF=$a bunit A$a esp $ESP; done ;;
  dvndbg)  # topo_dvn + topo_neck_regrow with TOPO_NECKDBG (why a neck runs the whole wire)
    cd $ROOT/engine; TOPO_NECKDBG=1 RUST_TEST_THREADS=1 cargo test --release --no-fail-fast --test topo_dvn --test topo_neck_regrow -- --nocapture > $OUT/dvndbg.log 2>&1 9>&-; note "dvndbg rc $?"
    cargo test --release --lib whole_supply > $OUT/lib-supply.log 2>&1 9>&-; note "lib supply rc $?" ;;
  bunits) shift; EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1); ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
    WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
    for x in "$@"; do case $x in qm) bunit C1 qm $QM ;; ep) bunit C1 ep $EP ;; esp) bunit C1 esp $ESP ;; wic) bunit C1 wic $WIC ;; pats) mkdir -p $P/runs; pats T ;; esac; done ;;
  all)  # all TAG BASE_TAG: both (patterns T + crop, the base tag's arms joined) first, then final
    T=$2; bash $ROOT/tools/neckrg_job.sh both $T T tip-det-s0 $3 > $OUT/u-both.log 2>&1; note "both rc $?"
    bash $ROOT/tools/neckrg_job.sh final $T > $OUT/u-final.log 2>&1; note "final rc $?" ;;
  final)  # final TAG: xunit + two board units (det s0 C1, as dvn7s), 3 hzq units
    T=$2
    hzq run --threads 6 --name $T-x -- bash $ROOT/tools/neckrg_job.sh xunit > $OUT/u-x.log 2>&1 9>&- &
    hzq run --threads 4 --name $T-b1 -- bash $ROOT/tools/neckrg_job.sh bunits qm ep > $OUT/u-b1.log 2>&1 9>&- &
    hzq run --threads 4 --name $T-b2 -- bash $ROOT/tools/neckrg_job.sh bunits esp wic pats > $OUT/u-b2.log 2>&1 9>&- &
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
