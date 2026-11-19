#!/usr/bin/env bash
# syncwait_job.sh (branch sync-wait, docs/dev/design/sync-wait.md): VM runs of the fair slots + [optimize] table_lag.
#   syncwait_job.sh hash      BULK: copper neutrality + determinism (fixed rounds): main vs tip lag 0 on 4 boards (4 threads),
#                             tip lag 0 / lag 2 on esp at 1/2/4 threads, tip lag 0 esp t4 twice
#   syncwait_job.sh ab        BULK: quality at equal 600 s wall (rounds/stable off, the dialog-like loop) on esp + qmtech,
#                             seeds 0-4: main, tip lag 0, 1, 2 (4 threads each)
#   syncwait_job.sh timing <board>   TIMING (one ccx33 slot): tip lag 0, 1, 2 one after another, 600 s wall, 4 threads
#   syncwait_job.sh one <cfg>:<board>:<seed>:<threads>   one run
# cfg: m = main tree (MAIN12, synced + built on the VM by hand), t0/t1/t2 = tip with table_lag 0/1/2;
#      prefix h = fixed rounds (12, qmtech 8; stable_patience the same; cpu clock 2400), prefix w = 600 s wall, no round cap,
#      no stable stop (rounds = stable_patience = 100000: the loop runs to its time share, as in the dialog); suffix r = rerun.
# Out: $HZ_RUN_DIR/<cfg>/<board>-t<threads>/<board>/s<seed>/ (score_runs layout), logs $HZ_RUN_DIR/<cfg>-<board>-s<seed>-t<threads>.log.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
MAIN12=${SW_MAIN:-524465c276bf}  # main tree to compare with (SW_MAIN=<12 hex> for a newer main)
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; esac; }
one() {
  local IFS=:; set -- $1; unset IFS
  local c=$1 b=$2 seed=$3 th=$4
  local tree=$ROOT py=python venv=${TOPOR_VENV:-} SETS=() CLK=()
  local cc=${c%r}
  case $cc in
    h*) local n=12; [ $b = qmtech ] && n=8; SETS+=(--set optimize.rounds=$n --set optimize.stable_patience=$n) ;;
    w*) SETS+=(--set optimize.rounds=100000 --set optimize.stable_patience=100000); CLK=(--clock wall --timeout 600) ;;
    *) note "unknown cfg $c"; return ;;
  esac
  case ${cc#?} in
    m) tree=/root/topor-$MAIN12; venv=/root/venv-$MAIN12; py=$venv/bin/python ;;
    t0|t1|t2) SETS+=(--set optimize.table_lag=${cc#??}) ;;
    *) note "unknown cfg $c"; return ;;
  esac
  local o=$R/$c/$b-t$th; mkdir -p $o
  local t0=$(date +%s)
  ( cd $tree && TOPOR_VENV=$venv TOPOR_TREE=$tree $py -m harness.score_runs $o "$(board_path $b)" --only $seed --no-png \
      --threads $th "${CLK[@]}" "${SETS[@]}" > $R/$c-$b-s$seed-t$th.log 2>&1 9>&- )
  note "run $c $b s$seed t$th rc $? wall $(( $(date +%s) - t0 )) s"
  rm -rf $o/*/s$seed/work 2>/dev/null  # only this run's own work dir (a glob over s* deleted sibling seeds' .kicad_pro mid-run: reports/rules-mismatch-forensic-20261119-1408.md)
}
lane() { for x in "$@"; do one "$x"; done; }
u() {  # name threads "lane1 runs" "lane2 runs" ...
  local n=$1 t=$2; shift 2
  local cmd="" l
  for l in "$@"; do cmd+="bash $ROOT/tools/syncwait_job.sh lane $l & "; done
  cmd+="wait"
  printf '%s\n' "$cmd" > $R/unit-$n.sh
  ( hzq run --pin --threads $t --name $n -- bash $R/unit-$n.sh > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & sleep 1
}
wait_main() {
  for i in $(seq 1 120); do ls /root/venv-$MAIN12/.built* >/dev/null 2>&1 && return 0; sleep 15; done
  note "main venv /root/venv-$MAIN12 not built"; return 1
}
tests() {  # targeted release tests (rule 14c) + clippy + the plugin's own pytest file + the generated docs check
  rustup component add clippy >/dev/null 2>&1
  ( cd engine && cargo clippy --release --all-targets 9>&- > $R/clippy.log 2>&1; echo "clippy rc $?" >> $R/rc.txt
    cargo test --release --lib -- topo::optimise topo::table topo::core::tests 9>&- > $R/test-lib.log 2>&1; echo "test lib rc $?" >> $R/rc.txt
    cargo test --release --test topo_tw3_table --test topo_stop_reason --test topo_tw3_drawonly --test topo_m2_draw_once 9>&- > $R/test-int.log 2>&1; echo "test int rc $?" >> $R/rc.txt )
  CI=1 python -m pytest -q tests/plugin/test_score_progress.py > $R/pytest.log 2>&1; note "pytest rc $?"
  PYTHONPATH=plugin python -m topor.docs_gen --check > $R/docs.log 2>&1; note "docs_gen rc $?"
}
case ${1:-} in
  one) one "$2" ;;
  tests) tests ;;
  pyfull)  # the full pytest (rule 7: Python touched), CI=1, as one 4-thread hzq unit
    ( hzq run --pin --threads 4 --name pytest -- bash -c "cd $ROOT && CI=1 python -m pytest -q -rs > $R/pytest-full.log 2>&1"; note "unit pytest rc $?" )
    note "all done" ;;
  tunit)  # the tests as one 4-thread hzq unit (the bulk VM's queue)
    ( hzq run --pin --threads 4 --name tests -- bash $ROOT/tools/syncwait_job.sh tests > $R/unit-tests.log 2>&1; note "unit tests rc $?" )
    note "all done" ;;
  mut)
    # mutants in their own tree copy + target dir (never beside a suite)
    mkdir -p $R/mt && git -C $ROOT archive HEAD 2>/dev/null | tar -x -C $R/mt 2>/dev/null || rsync -a --exclude target --exclude scratch $ROOT/ $R/mt/
    find $R/mt -name '*.rs' -exec touch {} +
    ( hzq run --pin --threads 4 --name mut -- bash -c "cd $R/mt && CARGO_TARGET_DIR=$R/mt/engine/target python3 tools/syncwait_mut.py $R/mt $R/mutants.tsv ${2:-0} > $R/mut.log 2>&1 9>&-"; note "unit mut rc $?" )
    note "all done" ;;
  lane) shift; lane "$@" ;;
  hash)
    nproc > $R/nproc.txt
    ( hzq run --pin --threads 4 --name tests -- bash $ROOT/tools/syncwait_job.sh tests > $R/unit-tests.log 2>&1; note "unit tests rc $?" ) & sleep 1
    # tip units first (main is synced + built by hand after the VM is up)
    u ht-q 4 "ht0:qmtech:0:4"
    u ht-e 4 "ht0:esp:0:4 ht0r:esp:0:4"
    u ht-e12 3 "ht0:esp:0:1 ht2:esp:0:1" "ht0:esp:0:2 ht2:esp:0:2"
    u ht-e2 4 "ht2:esp:0:4 ht0:wiccon:0:4 ht0:eprom:0:4"
    wait_main && {
      u hm-q 4 "hm:qmtech:0:4"
      u hm-e 4 "hm:esp:0:4 hm:wiccon:0:4 hm:eprom:0:4"
    }
    wait; note "all done" ;;
  ab)
    nproc > $R/nproc.txt
    for b in qmtech esp; do for s in 0 1 2 3 4; do u wt-$b-$s 4 "wt0:$b:$s:4 wt1:$b:$s:4"; u wt2-$b-$s 4 "wt2:$b:$s:4"; done; done
    wait_main && for b in qmtech esp; do for s in 0 1 2 3 4; do u wm-$b-$s 4 "wm:$b:$s:4"; done; done
    wait; note "all done" ;;
  timing)
    nproc > $R/nproc.txt; uptime >> $R/nproc.txt
    for l in 0 1 2; do one "wt$l:$2:0:4"; done
    note "all done" ;;
  pair)
    # TIMING, ONE job (one ccx33 slot's hold): lag 0 and lag 1 run at the same time, 4 threads each, same board/seed order
    # in both lanes, 600 s wall. usage: pair <board:seed,board:seed,...>
    nproc > $R/nproc.txt; uptime >> $R/nproc.txt
    for l in 0 1; do
      ( for bs in ${2//,/ }; do one "wt$l:${bs}:4"; uptime >> $R/nproc.txt; done ) &
    done
    wait; note "all done" ;;
  lagseq)
    # TIMING pair half: lag $2 on the boards given, one after another; the other slot runs the other lag at the same
    # time (same neighbour load in both slots: the ccx33's 2 slots share its 4 cores' hyperthreads)
    nproc > $R/nproc.txt; uptime >> $R/nproc.txt
    l=$2; shift 2
    for b in "$@"; do one "wt$l:$b:0:4"; uptime >> $R/nproc.txt; done
    note "all done" ;;
  *) echo "usage: $0 hash|ab|timing <board>|one <cfg>:<board>:<seed>:<threads>"; exit 2 ;;
esac
