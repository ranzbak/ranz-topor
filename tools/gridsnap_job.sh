#!/usr/bin/env bash
# grid-snap r1 on the VM (run from the rev's tree root): 4 hzq units, det seed 0 (score_runs --only 0, rounds=1, runs=1),
# grid 0.254 mm (the user's KiCad User Grid, 10 mil; origin from each board), arms off / prefer / fallback:
#   U1 qmtech prefer; U2 qmtech off, then fallback; U3 esp + eprom x 3 arms; U4 wiccon x 3 arms + prefer at 0.1 mm,
#   the pattern grid_snap_bends (off + prefer) with its PNG, clippy, the targeted release tests and pytest.
# After each board run: tools/gridsnap_measure.py on the routed copy (bends / vias on the grid, survivors by reason).
#   tools/gridsnap_job.sh            (the job)      tools/gridsnap_job.sh unit <kind> ...   (one unit)
set -u
R=${HZ_RUN_DIR:?}; ROOT=$PWD
note() { echo "$*" >> $R/rc.txt; }
A=(--no-png --set optimize.rounds=1 --set optimize.runs=1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
EP=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
board() {  # arm name pcb [grid_mm]
  local arm=$1 name=$2 pcb=$3 g=${4:-0.254} S=()
  [ "$arm" != off ] && S=(--set "drawing.grid_snap=\"${arm%%-*}\"" --set "drawing.grid_mm=$g")
  local t0=$(date +%s)
  python -m harness.score_runs $R/$arm/$name $pcb --only 0 --threads ${THR:-4} "${A[@]}" "${S[@]}" > $R/$arm-$name.log 2>&1 9>&-
  note "$arm-$name rc $? wall $(( $(date +%s) - t0 )) s"
  local out=$(ls $R/$arm/$name/*/s0/*-topor/*.kicad_pcb 2>/dev/null | head -1)
  [ -n "$out" ] && python tools/gridsnap_measure.py "$out" --grid-mm $g --json $R/$arm-$name-grid.json >> $R/measure.log 2>&1 9>&-
}
if [ "${1:-}" = unit ]; then
  case $2 in
    qm-prefer) board prefer qmtech $QM ;;
    qm-off-fb) board off qmtech $QM; board fallback qmtech $QM ;;
    esp-ep) for a in off prefer fallback; do board $a esp $ESP; board $a eprom $EP; done ;;
    wic-rest)
      for a in off prefer fallback; do board $a wiccon $WIC; done
      board prefer-g01 wiccon $WIC 0.1
      B=$R/pboards; P=$R/pruns
      python -m harness.drc_patterns $B grid_snap_bends --drc > $R/pattern.log 2>&1 9>&-
      for a in off prefer; do
        S=(); [ $a = prefer ] && S=(--set 'drawing.grid_snap="prefer"' --set drawing.grid_mm=0.254)
        mkdir -p $P/$a/grid_snap_bends
        TOPOR_HAND_LABEL=problem tools/route_board.sh $B/grid_snap_bends/problem/grid_snap_bends.kicad_pcb $P/$a/grid_snap_bends --keep-copper --no-png "${S[@]}" >> $R/pattern.log 2>&1 9>&-
        note "pattern $a rc $?"
      done
      python -m harness.drc_patterns_png $P $R/png grid_snap_bends >> $R/pattern.log 2>&1 9>&-; note "png rc $?"
      rustup component add clippy >/dev/null 2>&1
      ( cd engine && CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/mt-target-grid-snap cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; note "clippy rc $?"
        CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/mt-target-grid-snap cargo test --release --lib -- grid octilinear pad_stub own_pad_end stub_price leave_point connectors relocat > $R/tests.log 2>&1 9>&-; note "lib tests rc $?"
        CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/mt-target-grid-snap cargo test --release --test topo_g0 --test topo_pad_snap >> $R/tests.log 2>&1 9>&-; note "int tests rc $?" )
      CI=1 python -m pytest -q tests/plugin/test_grid.py tests/plugin/test_config.py tests/plugin/test_config_docs.py > $R/pytest.log 2>&1 9>&-; note "pytest rc $?" ;;
  esac
  exit 0
fi
hzq run --threads 4 --name gs-qm-prefer -- bash tools/gridsnap_job.sh unit qm-prefer &
sleep 2
hzq run --threads 4 --name gs-qm-off-fb -- bash tools/gridsnap_job.sh unit qm-off-fb &
sleep 2
hzq run --threads 4 --name gs-esp-ep -- bash tools/gridsnap_job.sh unit esp-ep &
sleep 2
THR=2 hzq run --threads 2 --name gs-wic-rest -- env THR=2 bash tools/gridsnap_job.sh unit wic-rest &
wait
note "all done"
