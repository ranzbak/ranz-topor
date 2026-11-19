#!/usr/bin/env bash
# lastprev_job.sh (branch last-preview, docs/dev/design/last-preview.md): VM runs of the branch.
#   lastprev_job.sh merge   off copper K on eprom/wiccon/esp/qmtech (the config of tw4r2_job.sh one K incl. TOPOR_UNRELEASED_SETTINGS=1, compared with
#                           copper_hash --cmp-run against main's run), preview_cost on esp (3 reps on/off, 4 threads),
#                           clippy --release --all-targets, the whole release suite, pytest (CI=1)
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; esac; }
one() {
  local b=$1 o=$R/K/$1; mkdir -p $o; local t0=$(date +%s)
  TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs $o "$(board_path $b)" --only 0 --no-png --threads 1 --set 'drawing.realise="erbs"' \
    --set optimize.runs=1 --set optimize.rounds=1 > $R/K-$b.log 2>&1 9>&-
  note "run K $b rc $? wall $(( $(date +%s) - t0 )) s"
}
case ${1:-} in
  one) one "$2" ;;
  pytest)  # one full pytest (CI=1) at the branch tip
    ( hzq run --threads 4 --name lp-pytest -- bash -c "cd $ROOT && CI=1 python -m pytest -q -p no:cacheprovider" > $R/pytest.log 2>&1 9>&-; note "unit pytest rc $?" ) &
    wait; note "all done" ;;
  cost)  # timing on its own (no SCHED_IDLE pre-start: hzq run waits for real tokens)
    ( hzq run --threads 4 --name lp-cost -- python -m harness.preview_cost $E $R/cost --reps 3 > $R/cost.log 2>&1 9>&-; note "unit cost rc $?" ) &
    wait; note "all done" ;;
  merge)
    ( hzq run --threads 4 --name lp-off -- bash -c "for b in qmtech esp wiccon eprom; do bash $ROOT/tools/lastprev_job.sh one \$b & done; wait" > $R/u-off.log 2>&1; note "unit off rc $?" ) &
    ( hzq run --threads 4 --name lp-cost -- python -m harness.preview_cost $E $R/cost --reps 3 > $R/cost.log 2>&1 9>&-; note "unit cost rc $?" ) &
    ( hzq run --threads 4 --name lp-suite -- bash -c "cargo clippy --release --all-targets > $R/clippy.log 2>&1; echo clippy rc \$? >> $R/rc.txt; cd $ROOT/engine && cargo test --release --no-fail-fast" > $R/suite.log 2>&1 9>&-; note "unit suite rc $?" ) &
    ( hzq run --threads 4 --name lp-pytest -- bash -c "cd $ROOT && CI=1 python -m pytest -q -p no:cacheprovider" > $R/pytest.log 2>&1 9>&-; note "unit pytest rc $?" ) &
    wait; note "all done" ;;
  *) echo "usage: $0 merge | pytest | cost | one <board>"; exit 2 ;;
esac
