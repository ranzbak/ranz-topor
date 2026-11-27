#!/usr/bin/env bash
# bundle-turn fix round r2, second wave (inside topor-bt2f's paid hour): the crossing_split fix.
#   bundle_turn_r2g_job.sh top     the units below, then "all done"
#   fix: qm + esp + wic det s0 C1 at the defaults (crossing_split 3), TOPO_XRDBG + TOPO_LOOPDBG
#   main: qm + esp det s0 C1 on main (the tree /root/topor-316e86ef3110 synced before), for the crops
#   mut: the item-2 / item-4 mutants (bundle_turn_r2fix_job.sh mut, MUT_NO_ENV) and then the crops
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $OUT/rc.txt; }
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
unit() {  # unit ARM BOARD PCB THREADS [sets]
  local a=$1 n=$2 p=$3 th=$4; shift 4
  TOPO_XRDBG=1 TOPO_LOOPDBG=1 TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads $th $C1 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-
  note "$a-$n rc $?"; gzip -f "$OUT/$a-$n.log"
}
case "${1:-}" in
  fix)
    unit tip qm $QM 4 & unit tip esp $ESP 2 & unit tip wic $WIC 2 & wait ;;
  main)
    M=/root/topor-316e86ef3110
    ( cd $M && export PATH=/root/venv-316e86ef3110/bin:$PATH TOPOR_VENV=/root/venv-316e86ef3110 TOPOR_TREE=$M && unit main qm $QM 4 & cd $M && export PATH=/root/venv-316e86ef3110/bin:$PATH TOPOR_VENV=/root/venv-316e86ef3110 TOPOR_TREE=$M && unit main esp $ESP 2 & wait ) ;;
  mut)
    # the crops' middle panel: the oblique-0 arms of the first wave (same code but crossing_split)
    for b in qm esp; do ln -sfn /root/runs/bt2f/obl0-$b $OUT/obl0-$b; note "obl0-$b rc 0 (first wave, /root/runs/bt2f)"; done
    MUT_NO_ENV=1 bash $ROOT/tools/bundle_turn_r2fix_job.sh mut ;;
  top)
    hzq run --threads 8 --name bt2g-fix -- bash $ROOT/tools/bundle_turn_r2g_job.sh fix 9>&- & sleep 3
    hzq run --threads 6 --name bt2g-main -- bash $ROOT/tools/bundle_turn_r2g_job.sh main 9>&- & sleep 3
    hzq run --threads 6 --name bt2g-mut -- bash $ROOT/tools/bundle_turn_r2g_job.sh mut 9>&- &
    wait
    note "all done" ;;
  *) echo "usage: $0 top|fix|main|mut" >&2; exit 2 ;;
esac
