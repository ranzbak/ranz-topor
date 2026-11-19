#!/usr/bin/env bash
# tw4r2_job.sh (branch tw4-r2, docs/dev/design/tw4-r2.md): TW4 round 2 runs on the VM.
#   tw4r2_job.sh one <run>   one run; <run> = <cfg>:<board>[:<threads>]
#   tw4r2_job.sh tests       cargo tests of the round (viacut unit tests, topo_cm1_room)
#   tw4r2_job.sh io6         tests + esp/qmtech rounds=1 for every option of item (2)
#   tw4r2_job.sh dh          (2b): tests, defaults rounds=1 on 4 boards (off = main copper), Kh/Kbh/Krh/Kbrh esp+qmtech, vgap probe
#   tw4r2_job.sh verify      mutants (own tree copy), the whole release test suite, pytest (CI=1)
#   tw4r2_job.sh eq <cfgs>   equal time: esp seeds 0-4 + qmtech seed 0 per cfg (wall 300 s, 4 threads; cfg - = defaults),
#                            plus eprom/wiccon rounds=1 with the defaults (off = main copper)
# Every run: harness.score_runs (cpu clock 2400 s unless equal time), optimize.runs=1, --no-png, realise erbs.
# cfg: K = rounds=1; M = rounds=40; e<k> = equal time (wall 300 s) seed k; suffixes (any order, at the end):
#   b = [capacity] via_best_point = true; r = [capacity] via_over_ramp = 0.05; R = via_over_ramp = 0.1;
#   h = [capacity] via_disc_height = true; p = [capacity] pour_cuts = true; q / Q = pour_cut_cells 250000 / 100000 (pour mode: PCFG = the cfgs).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;;
  pring) echo $R/pat/pour_ring/problem/pour_ring.kicad_pcb;; vgap) echo $R/pat/via_gap_best_point/problem/via_gap_best_point.kicad_pcb;; esac; }
one() {
  local c=${1%%:*} rest=${1#*:}; local b=${rest%%:*} th=1
  [ "$rest" != "$b" ] && th=${rest#*:}
  local SETS=(--set 'drawing.realise="erbs"' --set optimize.runs=1) only=0 cc=$c CLK=()
  local o=$R/$c/$b; mkdir -p $o
  while :; do
    if [ "${c%b}" != "$c" ]; then SETS+=(--set capacity.via_best_point=true); c=${c%b}
    elif [ "${c%r}" != "$c" ]; then SETS+=(--set capacity.via_over_ramp=0.05); c=${c%r}
    elif [ "${c%R}" != "$c" ]; then SETS+=(--set capacity.via_over_ramp=0.1); c=${c%R}
    elif [ "${c%p}" != "$c" ]; then SETS+=(--set capacity.pour_cuts=true); c=${c%p}
    elif [ "${c%h}" != "$c" ]; then SETS+=(--set capacity.via_disc_height=true); c=${c%h}
    elif [ "${c%q}" != "$c" ]; then SETS+=(--set capacity.pour_cut_cells=250000); c=${c%q}
    elif [ "${c%Q}" != "$c" ]; then SETS+=(--set capacity.pour_cut_cells=100000); c=${c%Q}
    else break; fi
  done
  case $c in
    K) SETS+=(--set optimize.rounds=1) ;;
    M) SETS+=(--set optimize.rounds=40) ;;
    e*) only=${c#e}; SETS+=(--set optimize.rounds=1000 --set optimize.stable_patience=1000); CLK=(--clock wall --timeout 300) ;;
    *) note "unknown cfg $c"; return ;;
  esac
  local t0=$(date +%s)
  TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs $o "$(board_path $b)" --only $only --no-png --threads $th "${CLK[@]}" "${SETS[@]}" \
    > $R/$cc-$b.log 2>&1 9>&-
  note "run $cc $b th $th rc $? wall $(( $(date +%s) - t0 )) s"
}
tests() {
  ( cd engine && cargo test --release --lib -- viacut pourplan optimise::tests::the_pour > $R/test-lib.log 2>&1 9>&-; note "test lib viacut+pour rc $?"
    cargo test --release --test topo_cm1_room > $R/test-cm1.log 2>&1 9>&-; note "test topo_cm1_room rc $?"
    cargo test --release --test topo_disc_height --test topo_d3 > $R/test-dh.log 2>&1 9>&-; note "test topo_disc_height+d3 rc $?" )
}
case ${1:-} in
  one) one "$2" ;;
  tests) tests ;;
  io6)
    ( hzq run --threads 4 --name t4r2-tests -- bash $ROOT/tools/tw4r2_job.sh tests > $R/u-tests.log 2>&1; note "unit tests rc $?" ) &
    for b in esp qmtech; do
      ( hzq run --threads 4 --name t4r2-$b -- bash -c "for c in K Kb Kr KR; do bash $ROOT/tools/tw4r2_job.sh one \$c:$b & done; wait" > $R/u-$b.log 2>&1; note "unit $b rc $?" ) &
    done
    wait; note "all done" ;;
  pat)
    python -m harness.tw4r2_patterns $R/pat > $R/gen.log 2>&1; note "gen rc $?"
    ( hzq run --threads 4 --name t4r2-pat -- bash -c "for r in ${PATRUNS:-M:pring Mp:pring M:vgap Mb:vgap Mbr:vgap}; do bash $ROOT/tools/tw4r2_job.sh one \$r & done; wait" > $R/u-pat.log 2>&1; note "unit pat rc $?" ) &
    wait; note "all done" ;;
  verify)
    ( hzq run --threads 4 --name t4r2-mut -- bash -c "mkdir -p $R/mt && tar -C $ROOT --exclude=./engine/target --exclude=./target --exclude=./scratch -cf - . | tar -C $R/mt -xf - && cd $R/mt && CARGO_TARGET_DIR=$R/mt/target-mut python3 tools/tw4r2_mutate.py run $R/mut" > $R/u-mut.log 2>&1 9>&-; note "unit mut rc $?" ) &
    ( hzq run --threads 4 --name t4r2-suite -- bash -c "cd $ROOT/engine && cargo test --release --no-fail-fast" > $R/suite.log 2>&1 9>&-; note "unit suite rc $?" ) &
    ( hzq run --threads 4 --name t4r2-pytest -- bash -c "cd $ROOT && CI=1 python -m pytest -q -p no:cacheprovider" > $R/pytest.log 2>&1 9>&-; note "unit pytest rc $?" ) &
    wait; note "all done" ;;
  dh)
    python -m harness.tw4r2_patterns $R/pat > $R/gen.log 2>&1; note "gen rc $?"
    ( hzq run --threads 4 --name t4r2-tests -- bash $ROOT/tools/tw4r2_job.sh tests > $R/u-tests.log 2>&1; note "unit tests rc $?" ) &
    ( hzq run --threads 4 --name t4r2-off -- bash -c "for b in eprom wiccon esp qmtech; do bash $ROOT/tools/tw4r2_job.sh one K:\$b & done; wait" > $R/u-off.log 2>&1; note "unit off rc $?" ) &
    for b in esp qmtech; do
      ( hzq run --threads 4 --name t4r2-h$b -- bash -c "for c in Kh Kbh Krh Kbrh; do bash $ROOT/tools/tw4r2_job.sh one \$c:$b & done; wait" > $R/u-h$b.log 2>&1; note "unit h$b rc $?" ) &
    done
    ( hzq run --threads 4 --name t4r2-hpat -- bash -c "for r in Kh:vgap Kbh:vgap Mh:vgap Mbh:vgap; do bash $ROOT/tools/tw4r2_job.sh one \$r & done; wait" > $R/u-hpat.log 2>&1; note "unit hpat rc $?" ) &
    wait; note "all done" ;;
  pour)
    for b in esp qmtech; do
      ( hzq run --threads 3 --name t4r2-p$b -- bash -c "for c in ${PCFG:-Kpq KpQ}; do bash $ROOT/tools/tw4r2_job.sh one \$c:$b & done; wait" > $R/u-p$b.log 2>&1; note "unit p$b rc $?" ) &
    done
    wait; note "all done" ;;
  eq)
    shift
    ( hzq run --threads 2 --name t4r2-off -- bash -c "bash $ROOT/tools/tw4r2_job.sh one K:eprom & bash $ROOT/tools/tw4r2_job.sh one K:wiccon & wait" > $R/u-off.log 2>&1; note "unit off rc $?" ) &
    for c in "$@"; do
      [ "$c" = "-" ] && c=""
      for s in 0 1 2 3 4; do
        ( hzq run --threads 4 --name t4r2-e$s$c-esp -- bash $ROOT/tools/tw4r2_job.sh one e$s$c:esp:4 > $R/u-e$s$c-esp.log 2>&1; note "unit e$s$c esp rc $?" ) &
      done
      ( hzq run --threads 4 --name t4r2-e0$c-qm -- bash $ROOT/tools/tw4r2_job.sh one e0$c:qmtech:4 > $R/u-e0$c-qm.log 2>&1; note "unit e0$c qmtech rc $?" ) &
    done
    wait; note "all done" ;;
esac
