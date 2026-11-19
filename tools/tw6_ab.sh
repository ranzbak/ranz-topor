#!/usr/bin/env bash
# tw6_ab.sh (branch tw6-via-sites, docs/dev/design/tw6-via-sites.md): TW6 engine parts on the VM.
#   tw6_ab.sh one <cfg>:<board>[:<threads>]
#   tw6_ab.sh tests          the TW6 tests + the tw4-r2 tests the parts touch (release)
#   tw6_ab.sh off            defaults rounds=1 on eprom/wiccon/esp/qmtech (off = main copper; compare with ladder_diff)
#   tw6_ab.sh pats           planned_crossing_window + site_or_stay + lead_via_end, K and M per cfg in PCFG (default "- s p sh shp")
#   tw6_ab.sh eq <cfgs>      equal time: esp seeds 0-4 + qmtech seed 0 per cfg (wall 300 s, 4 threads; cfg - = defaults)
#   tw6_ab.sh ac <cfgs>      tests + off + pats + eq <cfgs> (the A+C round)
#   tw6_ab.sh be <cfgs>      tests + lint (clippy) + pytest (CI=1, full) + pats + eq <cfgs> (the B search + E round)
# cfg: K = rounds=1; M = rounds=40; e<k> = equal time seed k; suffixes (any order, at the end):
#   s = [vias] site_price = "graded"; h = [capacity] via_disc_height = true; x = [capacity] pad_exits = true;
#   p = [vias] pair_room = true; b = [factors] weight via_site_social = $SITE_W (default 50);
#   m = [optimize] site_move = true; j = [optimize] branch_move = true.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;;
  pcw) echo $ROOT/tw6-job/pat/planned_crossing_window/problem/planned_crossing_window.kicad_pcb;;
  sos) echo $R/tw6p/site_or_stay/problem/site_or_stay.kicad_pcb;; lve) echo $R/tw6p/lead_via_end/problem/lead_via_end.kicad_pcb;; esac; }
one() {
  local c=${1%%:*} rest=${1#*:}; local b=${rest%%:*} th=1
  [ "$rest" != "$b" ] && th=${rest#*:}
  local SETS=(--set 'drawing.realise="erbs"' --set optimize.runs=1) only=0 cc=$c CLK=() KEEP=()
  local o=$R/$c/$b; mkdir -p $o
  while :; do
    if [ "${c%s}" != "$c" ]; then SETS+=(--set 'vias.site_price="graded"'); c=${c%s}
    elif [ "${c%h}" != "$c" ]; then SETS+=(--set capacity.via_disc_height=true); c=${c%h}
    elif [ "${c%x}" != "$c" ]; then SETS+=(--set capacity.pad_exits=true); c=${c%x}
    elif [ "${c%p}" != "$c" ]; then SETS+=(--set vias.pair_room=true); c=${c%p}
    elif [ "${c%b}" != "$c" ]; then SETS+=(--set "factors.weight={\"via_site_social\": ${SITE_W:-50}}"); c=${c%b}
    elif [ "${c%m}" != "$c" ]; then SETS+=(--set optimize.site_move=true); c=${c%m}
    elif [ "${c%j}" != "$c" ]; then SETS+=(--set optimize.branch_move=true); c=${c%j}
    else break; fi
  done
  case $c in
    K) SETS+=(--set optimize.rounds=1) ;;
    M) SETS+=(--set optimize.rounds=40) ;;
    e*) only=${c#e}; SETS+=(--set optimize.rounds=1000 --set optimize.stable_patience=1000); CLK=(--clock wall --timeout 300) ;;
    *) note "unknown cfg $c"; return ;;
  esac
  [ "$b" = lve ] && SETS+=(--set search.dangling_lead_mm=1.5)
  local t0=$(date +%s)
  if [ "$b" = lve ]; then
    TOPOR_UNRELEASED_SETTINGS=1 TOPO_VIADBG=1 tools/route_board.sh "$(board_path $b)" $o/$b/s0 --keep-copper --no-png "${SETS[@]}" > $R/$cc-$b.log 2>&1 9>&-
  else
    TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs $o "$(board_path $b)" --only $only --no-png --threads $th "${CLK[@]}" "${SETS[@]}" \
      > $R/$cc-$b.log 2>&1 9>&-
  fi
  note "run $cc $b th $th rc $? wall $(( $(date +%s) - t0 )) s"
}
tests() {
  ( cd engine && cargo test --release --test topo_site_price > $R/test-site.log 2>&1 9>&-; note "test topo_site_price rc $?"
    cargo test --release --test topo_site_social > $R/test-social.log 2>&1 9>&-; note "test topo_site_social rc $?"
    cargo test --release --test topo_g0 > $R/test-g0.log 2>&1 9>&-; note "test topo_g0 rc $?"
    cargo test --release --test topo_cm1_room --test topo_disc_height --test topo_pad_exit > $R/test-tw4r2.log 2>&1 9>&-; note "test cm1_room+disc_height+pad_exit rc $?"
    cargo test --release --lib > $R/test-lib.log 2>&1 9>&-; note "test lib rc $?" )
}
case ${1:-} in
  one) one "$2" ;;
  tests) tests ;;
  off)
    for b in eprom wiccon esp qmtech; do bash $ROOT/tools/tw6_ab.sh one K:$b & done; wait ;;
  pats)
    python -m harness.tw6_patterns $R/tw6p > $R/tw6p-gen.log 2>&1; note "gen rc $?"
    for c in ${PCFG:-- s p sh shp}; do [ "$c" = "-" ] && c=""; for k in K M; do for b in pcw sos lve; do bash $ROOT/tools/tw6_ab.sh one $k$c:$b & done; wait; done; done ;;
  eq)
    # long boards first; at most EQ_MAX (default 5) units in flight (AGENT_RULES: <= 4 waiting units per job)
    shift
    thr() { while [ "$(jobs -rp | wc -l)" -ge "${EQ_MAX:-5}" ]; do wait -n; done; }
    for c in "$@"; do
      [ "$c" = "-" ] && c=""
      thr; ( hzq run --threads 4 --name tw6-e0$c-qm -- bash $ROOT/tools/tw6_ab.sh one e0$c:qmtech:4 > $R/u-e0$c-qm.log 2>&1; note "unit e0$c qmtech rc $?" ) &
    done
    for c in "$@"; do
      [ "$c" = "-" ] && c=""
      for s in 0 1 2 3 4; do
        thr; ( hzq run --threads 4 --name tw6-e$s$c-esp -- bash $ROOT/tools/tw6_ab.sh one e$s$c:esp:4 > $R/u-e$s$c-esp.log 2>&1; note "unit e$s$c esp rc $?" ) &
      done
    done
    wait ;;
  lint) ( cd engine && rustup component add clippy > /dev/null 2>&1; cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; note "clippy rc $?" ) ;;
  pytest) ( CI=1 python -m pytest -q -rs -p no:cacheprovider > $R/pytest.log 2>&1 9>&-; note "pytest rc $?"; tail -3 $R/pytest.log >> $R/rc.txt ) ;;
  be)
    # B search + E round: tests + clippy + full pytest (Python touched) + pats (PCFG) + eq <cfgs>
    shift
    ( hzq run --threads 4 --name tw6-tests -- bash $ROOT/tools/tw6_ab.sh tests > $R/u-tests.log 2>&1; note "unit tests rc $?" ) &
    ( hzq run --threads 4 --name tw6-lint -- bash $ROOT/tools/tw6_ab.sh lint > $R/u-lint.log 2>&1; note "unit lint rc $?" ) &
    ( hzq run --threads 4 --name tw6-py -- bash $ROOT/tools/tw6_ab.sh pytest > $R/u-py.log 2>&1; note "unit pytest rc $?" ) &
    ( hzq run --threads 4 --name tw6-pats -- bash $ROOT/tools/tw6_ab.sh pats > $R/u-pats.log 2>&1; note "unit pats rc $?" ) &
    bash $ROOT/tools/tw6_ab.sh eq "$@"
    wait; note "all done" ;;
  ac)
    shift
    ( hzq run --threads 4 --name tw6-tests -- bash $ROOT/tools/tw6_ab.sh tests > $R/u-tests.log 2>&1; note "unit tests rc $?" ) &
    ( hzq run --threads 4 --name tw6-off -- bash $ROOT/tools/tw6_ab.sh off > $R/u-off.log 2>&1; note "unit off rc $?" ) &
    ( hzq run --threads 4 --name tw6-pats -- bash $ROOT/tools/tw6_ab.sh pats > $R/u-pats.log 2>&1; note "unit pats rc $?" ) &
    bash $ROOT/tools/tw6_ab.sh eq "$@"
    wait; note "all done" ;;
  *) echo "usage: tw6_ab.sh one <cfg:board[:th]> | tests | off | pats | eq <cfgs> | ac <cfgs>" >&2; exit 2 ;;
esac
