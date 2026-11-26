#!/usr/bin/env bash
# dvn_job.sh (branch draw-via-neck, docs/dev/design/draw-via-neck.md): PROPOSED patterns on the VM.
#   dvn_job.sh job    the pats unit under hzq (hz.sh job -- bash tools/dvn_job.sh job)
#   dvn_job.sh pats   boards (KiCad DRC of the references), each pattern routed through the plugin flow (route_board.sh,
#                     product defaults, runs=1, seed 0, TOPO_VIADBG + TOPO_NECKDBG + TOPO_ERBS_DEBUG), PNGs + summary.json
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
NAMES="via_jam_shift via_interval_sliver neck_in_pad_row neck_shared_channel pad_axis_entry pad_side_approach"
case ${1:-} in
  pats)
    python -m harness.dvn_patterns $R/dvnp/boards --drc > $R/dvnp-gen.log 2>&1; note "gen rc $?"
    for n in $NAMES; do
      TOPOR_UNRELEASED_SETTINGS=1 TOPO_VIADBG=1 TOPO_NECKDBG=1 TOPO_ERBS_DEBUG=1 tools/route_board.sh $R/dvnp/boards/$n/problem/$n.kicad_pcb \
        $R/dvnp/runs/D/$n/$n/s0 --set optimize.runs=1 > $R/dvnp/D-$n.txt 2>&1 9>&-
      note "pat D $n rc $?"
    done
    python -m harness.dvn_patterns_png $R/dvnp/runs $R/dvnp/png > $R/dvnp-png.log 2>&1; note "png rc $?"
    note "all done" ;;
  repng) python -m harness.dvn_patterns_png /root/runs/$2/dvnp/runs $R/dvnp/png > $R/dvnp-png.log 2>&1; note "png rc $?"; note "all done" ;;
  tests)  # r2: targeted release tests of the files this branch changes (draw.rs, padend.rs, core.rs, schema) + schema pytest
    export CARGO_TARGET_DIR=$R/target-tests CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
    T=""; for t in topo_dvn topo_vias54 topo_first_draw_45 topo_pad_end topo_pad_exit topo_neck_depth topo_m2_draw_once topo_tw3_drawonly \
      topo_pad_snap topo_pad_pour_via topo_stitch_vias topo_drawing_grace topo_erbs_margin topo_fuse_octi topo_d2 via_rules check_via_holes; do T="$T --test $t"; done
    (cd engine && cargo test --release --no-fail-fast $T) > $R/tests.log 2>&1 9>&-; note "cargo tests rc $?"
    CI=1 python -m pytest -q tests/test_engine_surface.py tests/test_docs_links.py > $R/pytest.log 2>&1 9>&-; note "pytest rc $?" ;;
  crops)  # r2: Q2/Q3 crops routed with this branch (plugin flow, defaults, runs=1, seed 0) + 3-panel PNGs vs today (tw-on c7f8bc8f)
    B=scratch/boards/dvn-crops; mkdir -p $R/crops
    for q in q2 q3; do
      TOPOR_UNRELEASED_SETTINGS=1 TOPO_NECKDBG=1 TOPO_TIPDBG=1 TOPO_VIADBG=1 tools/route_board.sh $B/$q/problem/dvn_$q.kicad_pcb $R/crops/$q/s0 \
        --set optimize.runs=1 > $R/crops/route-$q.txt 2>&1 9>&-
      note "crop route $q rc $?"
    done
    python - "$R" <<'PY' > $R/crops/png.log 2>&1
import json, subprocess, sys
R = sys.argv[1]; B = "scratch/boards/dvn-crops"
specs = {
  "q2": dict(view=[94.3, 55.0, 101.6, 63.6], focus=["FPGA_UART1_CTS", "USB_C_D+", "USB_C_D-"], nominal={"FPGA_UART1_CTS": 0.25},
             marks=[[97.1, 60.3], [97.95, 60.85]], title="dvn_crop_q2 (user-approved 2026-11-24): public qmtech crop of the Q2 neighbourhood (U9 fan-out)",
             expected="CTS necked only through the choke beside the U9 column (~0.9 mm), full 0.25 to the terminal; USB_C_D+/- vias in free room, no shorts."),
  "q3": dict(view=[131.9, 68.6, 139.2, 75.0], focus=["ADV_AVDD"], nominal={"ADV_AVDD": 0.28}, marks=[[137.5, 70.0]],
             title="dvn_crop_q3 (user-approved 2026-11-24): public qmtech crop of the Q3 neighbourhood (ADV7511 pad row)",
             expected="The ADV_AVDD bar ends straight on the x 134.72 pad's axis leg below its tip at 0.28; no diagonal into the pad side, no neck at the row."),
}
for q, sp in specs.items():
    sp.update(ref=f"{B}/{q}/reference/dvn_{q}.kicad_pcb", today=f"{B}/{q}/today/dvn_{q}.kicad_pcb",
              branch=f"{R}/crops/{q}/s0/dvn_{q}-topor/dvn_{q}.kicad_pcb", out=f"{R}/crops/dvn_crop_{q}-r2.png",
              today_label="today (tw-on c7f8bc8f, defaults, runs=1, seed 0)", branch_label="draw-via-neck r2 (A+B+C on, defaults, runs=1, seed 0)")
    json.dump(sp, open(f"{R}/crops/spec-{q}.json", "w"))
    print(q, subprocess.run([sys.executable, "-m", "harness.dvn_crop_png", f"{R}/crops/spec-{q}.json"]).returncode, flush=True)
PY
    note "crop png rc $?" ;;
  r2)  # r2 job: boards esp + qmtech det s0 (score_runs default run, as res-tip), targeted tests, mutants, crops
    ROOTD=$(pwd)
    hzq run --threads 4 --name dvn-qm -- python -m harness.score_runs $R/res-qm qmtech-minimig --only 0 --no-png > $R/u-qm.log 2>&1 &
    hzq run --threads 4 --name dvn-esp -- python -m harness.score_runs $R/res-esp esp-rust-board --only 0 --no-png > $R/u-esp.log 2>&1 &
    hzq run --threads 4 --name dvn-mutA -- python3 $ROOTD/tools/dvn_mutants.py $R/mut a > $R/u-mutA.log 2>&1 &
    hzq run --threads 4 --name dvn-mutBC -- python3 $ROOTD/tools/dvn_mutants.py $R/mut bc > $R/u-mutBC.log 2>&1 &
    hzq run --threads 2 --name dvn-crops -- bash $ROOTD/tools/dvn_job.sh crops > $R/u-crops.log 2>&1 &
    hzq run --threads 4 --name dvn-tests -- bash $ROOTD/tools/dvn_job.sh tests > $R/u-tests.log 2>&1 &
    wait; note "all done" ;;
  espdbg)  # r2 RCA of esp's +2 DRC (IO8_SCL): the default run with TOPO_TIPDBG, and pad_tip_anchor off
    ROOTD=$(pwd); F="--clock cpu --timeout 2400 --set capacity.compass_cut_check=true --no-png"
    TOPOR_UNRELEASED_SETTINGS=1 TOPO_TIPDBG=1 hzq run --threads 4 --name dvn-esp-tip -- $ROOTD/tools/route_board.sh esp-rust-board $R/esp-tipdbg/s0 $F > $R/u-esp-tip.log 2>&1 &
    TOPOR_UNRELEASED_SETTINGS=1 hzq run --threads 4 --name dvn-esp-notip -- $ROOTD/tools/route_board.sh esp-rust-board $R/esp-notip/s0 $F --set drawing.pad_tip_anchor=false > $R/u-esp-notip.log 2>&1 &
    wait; note "all done" ;;
  job) hzq run --threads 2 --name dvnp-pats -- bash $ROOT/tools/dvn_job.sh pats > $R/u-pats.log 2>&1; note "unit pats rc $?" ;;
  *) echo "usage: $0 pats | job | r2 | tests | crops"; exit 2 ;;
esac
