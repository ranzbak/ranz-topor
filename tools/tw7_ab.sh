#!/usr/bin/env bash
# tw7_ab.sh (branch tw7-moves, docs/dev/design/tw7-moves.md): TW7 parts A, B, D on the VM.
#   tw7_ab.sh one <cfg>:<board>[:<threads>]
#   tw7_ab.sh off            K (rounds=1) defaults on eprom/wiccon/esp/qmtech with TOPO_PROF + TOPO_PATDBG
#                            (off = main copper: compare with ladder_diff / copper_hash; xr_* profile; detour attribution)
#   tw7_ab.sh k <cfgs>       K per cfg on esp + qmtech (deterministic per-connection comparison)
#   tw7_ab.sh eq <cfgs>      equal time: esp seeds 0-4 + qmtech seed 0 per cfg (wall 300 s, 4 threads; cfg - = defaults)
#   tw7_ab.sh all <cfgs>     pyt + tests + off + k <cfgs> + eq - <cfgs>
#   tw7_ab.sh pytall         the full pytest (CI=1; Python touched, AGENT_RULES)
#   tw7_ab.sh pyt | tests    targeted pytest (config docs/defaults) | the TW7 cargo tests (release)
#   tw7_ab.sh tq <unit>...   TIMING slot (hz.sh job --timing, round 3): the units cfg:board:4 one after another, no hzq
#                            (the slot is the job's 4 threads); "all done" at the end
#   tw7_ab.sh bulk           round 3 bulk checks (hzq): pyt + tests + off (K x4, copper vs main)
# cfg: K = rounds=1; e<k> = equal time seed k; suffixes (any order, at the end):
#   u = [optimize] pattern_moves = all five kinds; c = only "u_clinch", "evasion";
#   d = [factors] detour = $DETOUR_K (default 1); x = [optimize] crossing_rules = false;
#   h = [optimize] pattern_share = $SHARE_H (default 0.25); q = pattern_share = $SHARE_Q (default 0.1).
#   round 4: z = [optimize] pattern_stuck = 0 (the excess-length order alone); l / m = [factors] detour = 0.25 / 0.5;
#   v = pattern_moves = the five kinds + "via_trade" (part C); p = [capacity] pour_cuts = true (part E needs it).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; esac; }
one() {
  local c=${1%%:*} rest=${1#*:}; local b=${rest%%:*} th=1
  [ "$rest" != "$b" ] && th=${rest#*:}
  local SETS=(--set 'drawing.realise="erbs"' --set optimize.runs=1) only=0 cc=$c CLK=()
  local o=$R/$c/$b; mkdir -p $o
  while :; do
    if [ "${c%u}" != "$c" ]; then SETS+=(--set 'optimize.pattern_moves=["clinch","double","unlayerable","u_clinch","evasion"]'); c=${c%u}
    elif [ "${c%c}" != "$c" ]; then SETS+=(--set 'optimize.pattern_moves=["u_clinch","evasion"]'); c=${c%c}
    elif [ "${c%d}" != "$c" ]; then SETS+=(--set factors.detour=${DETOUR_K:-1}); c=${c%d}
    elif [ "${c%x}" != "$c" ]; then SETS+=(--set optimize.crossing_rules=false); c=${c%x}
    elif [ "${c%h}" != "$c" ]; then SETS+=(--set optimize.pattern_share=${SHARE_H:-0.25}); c=${c%h}
    elif [ "${c%q}" != "$c" ]; then SETS+=(--set optimize.pattern_share=${SHARE_Q:-0.1}); c=${c%q}
    elif [ "${c%z}" != "$c" ]; then SETS+=(--set optimize.pattern_stuck=0); c=${c%z}
    elif [ "${c%l}" != "$c" ]; then SETS+=(--set factors.detour=0.25); c=${c%l}
    elif [ "${c%m}" != "$c" ]; then SETS+=(--set factors.detour=0.5); c=${c%m}
    elif [ "${c%v}" != "$c" ]; then SETS+=(--set 'optimize.pattern_moves=["clinch","double","unlayerable","u_clinch","evasion","via_trade"]'); c=${c%v}
    elif [ "${c%p}" != "$c" ]; then SETS+=(--set capacity.pour_cuts=true); c=${c%p}
    else break; fi
  done
  case $c in
    K) SETS+=(--set optimize.rounds=1) ;;
    e*) only=${c#e}; SETS+=(--set optimize.rounds=1000 --set optimize.stable_patience=1000); CLK=(--clock ${TW7_CLOCK:-wall} --timeout ${TW7_TIMEOUT:-300}) ;;
    *) note "unknown cfg $c"; return ;;
  esac
  local t0=$(date +%s)
  TOPOR_UNRELEASED_SETTINGS=1 TOPO_PROF=1 TOPO_PATDBG=1 python -m harness.score_runs $o "$(board_path $b)" --only $only --no-png --threads $th "${CLK[@]}" "${SETS[@]}" \
    > $R/$cc-$b.log 2>&1 9>&-
  note "run $cc $b th $th rc $? wall $(( $(date +%s) - t0 )) s"
}
thr() { while [ "$(jobs -rp | wc -l)" -ge "${EQ_MAX:-5}" ]; do wait -n; done; }
case ${1:-} in
  one) one "$2" ;;
  off) for b in eprom wiccon esp qmtech; do bash $ROOT/tools/tw7_ab.sh one K:$b & done; wait ;;
  k) shift; for c in "$@"; do for b in esp qmtech; do bash $ROOT/tools/tw7_ab.sh one K$c:$b & done; done; wait ;;
  eq)
    shift
    for c in "$@"; do [ "$c" = "-" ] && c=""
      thr; ( hzq run --threads 4 --name tw7-e0$c-qm -- bash $ROOT/tools/tw7_ab.sh one e0$c:qmtech:4 > $R/u-e0$c-qm.log 2>&1; note "unit e0$c qmtech rc $?" ) &
    done
    for c in "$@"; do [ "$c" = "-" ] && c=""
      for s in 0 1 2 3 4; do
        thr; ( hzq run --threads 4 --name tw7-e$s$c-esp -- bash $ROOT/tools/tw7_ab.sh one e$s$c:esp:4 > $R/u-e$s$c-esp.log 2>&1; note "unit e$s$c esp rc $?" ) &
      done
    done
    wait ;;
  ec)  # cpu-clock A/B on the shared VM (fair between cfgs, not a timing run): ec <seeds> <boards> <cfgs...>
    shift; S=$1 B=$2; shift 2
    for c in "$@"; do [ "$c" = "-" ] && c=""
      for b in $B; do for s in $S; do
        thr; ( TW7_CLOCK=cpu TW7_TIMEOUT=1200 hzq run --threads 4 --name tw7-e$s$c-$b -- env TW7_CLOCK=cpu TW7_TIMEOUT=1200 bash $ROOT/tools/tw7_ab.sh one e$s$c:$b:4 > $R/u-e$s$c-$b.log 2>&1; note "unit e$s$c $b rc $?" ) &
      done; done
    done
    wait; note "all done" ;;
  pytall) ( CI=1 python -m pytest -q -rs -p no:cacheprovider > $R/pytest-all.log 2>&1 9>&-; note "pytest all rc $?"; tail -3 $R/pytest-all.log >> $R/rc.txt; note "all done" ) ;;
  pyt) ( CI=1 python -m pytest -q -rs -p no:cacheprovider tests/plugin/test_config_docs.py tests/plugin/test_toml_defaults.py > $R/pytest.log 2>&1 9>&-; note "pytest config rc $?"; tail -2 $R/pytest.log >> $R/rc.txt ) ;;
  tests) ( cd engine && cargo test --release --test topo_tw7_patterns > $R/test-tw7.log 2>&1 9>&-; note "test topo_tw7_patterns rc $?"
           cargo test --release --lib pourplan > $R/test-pourplan.log 2>&1 9>&-; note "test lib pourplan rc $?"
           cargo test --release --lib -- pattern_share the_pattern_moves_line > $R/test-lib-tw7.log 2>&1 9>&-; note "test lib share/line rc $?" ) ;;
  tq) shift; for u in "$@"; do bash $ROOT/tools/tw7_ab.sh one "$u"; done; note "all done" ;;
  bulk)
    ( hzq run --threads 2 --name tw7-chk -- bash -c "bash $ROOT/tools/tw7_ab.sh pyt; bash $ROOT/tools/tw7_ab.sh tests" > $R/u-chk.log 2>&1; note "unit chk rc $?" ) &
    ( hzq run --threads 4 --name tw7-off -- bash $ROOT/tools/tw7_ab.sh off > $R/u-off.log 2>&1; note "unit off rc $?" ) &
    wait; note "all done" ;;
  all)
    shift
    ( hzq run --threads 2 --name tw7-chk -- bash -c "bash $ROOT/tools/tw7_ab.sh pyt; bash $ROOT/tools/tw7_ab.sh tests" > $R/u-chk.log 2>&1; note "unit chk rc $?" ) &
    ( hzq run --threads 4 --name tw7-off -- bash $ROOT/tools/tw7_ab.sh off > $R/u-off.log 2>&1; note "unit off rc $?" ) &
    for c in "$@"; do ( hzq run --threads 2 --name tw7-k$c -- bash $ROOT/tools/tw7_ab.sh k $c > $R/u-k$c.log 2>&1; note "unit k$c rc $?" ) & done
    bash $ROOT/tools/tw7_ab.sh eq - "$@"
    wait; note "all done" ;;
  *) echo "usage: see header"; exit 2 ;;
esac
