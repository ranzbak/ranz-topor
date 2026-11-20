#!/usr/bin/env bash
# spcost_job.sh (branch site-price-cost): deterministic seed-0 A/B of [optimize] escape_budget on arm X (every TW opt-in
# but [vias] site_price and preloop_p4 = false; recount-tw-20261119-2310 §4), rounds=1 runs=1, one hzq unit per
# arm+board (4 threads).
#   spcost_job.sh all              4 units: X-cost, X-units x esp, qmtech
#   spcost_job.sh one <arm> <board>
# Output: $HZ_RUN_DIR/<arm>/<board>/... (score_runs layout), rc lines in $HZ_RUN_DIR/rc.txt.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
note() { echo "$*" >> "$R/rc.txt"; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
X=(--set 'capacity.via_disc_height="on"' --set capacity.pad_exits=true --set capacity.pour_cuts=true
   --set capacity.via_best_point=true --set 'optimize.oscillation="bounded"' --set 'optimize.escape="layering"'
   --set 'optimize.escape_take="always"' --set optimize.relay_count_fade=0.8 --set optimize.loop_history=true
   --set optimize.float_wire=true --set optimize.loop_p4=true --set optimize.free_wire_cross=0.0 --set vias.pair_room=true
   --set optimize.site_move=true --set optimize.branch_move=true
   --set 'optimize.pattern_moves=["clinch","double","unlayerable","u_clinch","evasion"]' --set factors.detour=0.5
   --set optimize.rounds=1 --set optimize.runs=1)
one() {  # one <arm> <board>
  local arm=$1 b=$2 P SETS=("${X[@]}")
  case $b in esp) P=$E;; qmtech) P=$Q;; *) note "bad board $b"; return 2;; esac
  case $arm in cost) SETS+=(--set 'optimize.escape_budget="cost"');; units) SETS+=(--set 'optimize.escape_budget="units"');;
    *) note "bad arm $arm"; return 2;; esac
  local o=$R/$arm/$b t0; t0=$(date +%s)
  mkdir -p "$o"
  TOPOR_UNRELEASED_SETTINGS=1 TOPO_PROF=1 python -m harness.score_runs "$o" "$P" --only 0 --no-png "${SETS[@]}" > "$o/run.log" 2>&1 9>&-
  local rc=$?
  note "run $arm $b rc $rc wall $(( $(date +%s) - t0 )) s"
  return $rc
}
case ${1:-} in
  one) one "$2" "$3" ;;
  all)
    for b in qmtech esp; do for arm in units cost; do
      ( hzq run --threads 4 --name spc-$arm-$b -- bash "$ROOT/tools/spcost_job.sh" one $arm $b > /dev/null 2>&1; note "unit $arm $b rc $?" ) &
    done; done
    wait; note "all done" ;;
  *) echo "usage: see header"; exit 2 ;;
esac
