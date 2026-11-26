#!/usr/bin/env bash
# dvn5_job.sh (draw-via-neck r5 / via-shift-free r4, split for separate verification): VM units, det seed 0.
#   dvn5_job.sh attr     esp one cycle (runs 1, rounds 1: the same plan in every arm, only the drawing differs):
#                        D = this rev, S = via_shift_mm 0, N = neck_trim false, T = pad_tip_anchor false (per-key attribution vs main)
#   dvn5_job.sh suite    every release test binary once (+ ignored none) and pytest CI=1 of the schema/doc files
#   dvn5_job.sh ladder   eprom / wiccon / esp / qmtech at real settings (score_runs defaults, det seed 0)
#   dvn5_job.sh dbg ARM  esp one cycle with TOPO_VIADBG + TOPO_TIPDBG + TOPO_NECKDBG (copper may differ: never hashed vs a default arm)
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
EP=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
note() { echo "$*" >> $OUT/rc.txt; }
unit() {  # unit <arm> <board-short> <pcb> [sets...]
  local a=$1 n=$2 p=$3; shift 3
  hzq run --threads 4 --name "d5-$a-$n" -- python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" \
    > "$OUT/$a-$n.log" 2>&1 9>&-
  note "$a-$n rc $?"
}
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
C1=(--set optimize.runs=1 --set optimize.rounds=1)
case ${1:-} in
  attr)
    unit D esp $ESP "${C1[@]}" & sleep 3
    unit S esp $ESP "${C1[@]}" --set drawing.via_shift_mm=0 & sleep 3
    unit N esp $ESP "${C1[@]}" --set drawing.neck_trim=false & sleep 3
    unit T esp $ESP "${C1[@]}" --set drawing.pad_tip_anchor=false &
    wait; note "all done" ;;
  dbg)
    A=${2:-G}; shift 2 || true
    (export TOPO_VIADBG=1 TOPO_VIAPLANDBG=1 TOPO_TIPDBG=1 TOPO_NECKDBG=1 TOPO_RUNPROV=1; unit $A esp $ESP "${C1[@]}" "$@")
    find $OUT -name '*.log' -size +20M -exec gzip -9 {} + 2>/dev/null
    note "all done" ;;
  suite)
    export CARGO_TARGET_DIR=$OUT/target-suite CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
    (cd engine && cargo test --release --no-fail-fast) > $OUT/suite.log 2>&1 9>&-; note "release suite rc $?"
    grep -E '^test result|FAILED|panicked' $OUT/suite.log | sort | uniq -c | sort -rn | head -40 > $OUT/suite-summary.txt
    CI=1 python -m pytest -q tests/test_engine_surface.py tests/test_docs_links.py tests/plugin/test_config_docs.py tests/plugin/test_toml_defaults.py \
      > $OUT/pytest.log 2>&1 9>&-; note "pytest rc $?" ;;
  ladder)
    unit L qm $QM & sleep 3
    unit L esp $ESP & sleep 3
    unit L wic $WIC & sleep 3
    unit L ep $EP &
    wait; note "all done" ;;
  ttA)  # tip A round: targeted tests + esp / qmtech one cycle (D) at this rev
    hzq run --threads 4 --name d5-tt -- bash $ROOT/tools/dvn5_job.sh ttAtests > $OUT/u-tt.log 2>&1 9>&- & sleep 3
    unit D esp $ESP "${C1[@]}" & sleep 3
    unit D qm $QM "${C1[@]}" &
    wait; note "all done" ;;
  ttAtests)
    export CARGO_TARGET_DIR=$OUT/target-tt CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
    T=""; for t in topo_dvn topo_thick_layer tw2_metric topo_vias54 topo_fit topo_m3_patterns topo_d0 topo_cm1_room topo_fx_survivors topo_exist_term; do T="$T --test $t"; done
    (cd engine && cargo test --release --no-fail-fast $T) > $OUT/tt.log 2>&1 9>&-; note "tt rc $?"
    grep -E '^test result|FAILED|panicked' $OUT/tt.log > $OUT/tt-summary.txt ;;
  *) echo "usage: dvn5_job.sh attr|dbg ARM [sets]|suite|ladder"; exit 2 ;;
esac
