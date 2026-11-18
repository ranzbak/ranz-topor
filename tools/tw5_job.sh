#!/usr/bin/env bash
# tw5_job.sh (branch tw5-escape, docs/dev/design/tw5-escape.md): TW5 ESCAPE runs on the VM.
#   tw5_job.sh <mode>          start the hzq units of <mode> (inside hz.sh job), wait, note "all done"
#   tw5_job.sh lane <run>...   one lane inside a unit: the runs one after another
#   tw5_job.sh one <run>       one run; <run> = <cfg>:<board>[:<threads>]
#   tw5_job.sh tests           targeted release tests (lib escape/layers, loop integration binaries)
# Every board run: tools/route_board.sh (as the main baseline nd2b2-dcf4c0e), cpu clock 2400 s, optimize.runs=1, --no-png.
# cfg = <opts><rounds>[s<seed>]:
#   opts  O = every TW5 option at its old value (must give main's copper), N = the TW5 defaults,
#         Na/Nb/Nc/Nd/Ne = O with only that part on (a oscillation, b escape+take, c loop history, d float_wire, e no pre-loop P4)
#   rounds  r<N> = optimize.rounds=N, stable_patience=N (no stable stop before N cycles)
# Out: $HZ_RUN_DIR/<cfg>/<board>/, logs $HZ_RUN_DIR/<cfg>-<board>.log.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
P=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
W=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
B=scratch/boards/bornhack2026/21c5f478f1af/bornhack2026-hardware.kicad_pcb
board_path() { case $1 in eprom) echo $P;; wiccon) echo $W;; esp) echo $E;; qmtech) echo $Q;; bornhack) echo $B;; esac; }
OLD=(--set 'optimize.oscillation="cooling"' --set 'optimize.escape="ties"' --set 'optimize.escape_take="not_worse"'
     --set optimize.relay_count_fade=1.0 --set optimize.loop_history=false --set optimize.float_wire=false --set optimize.preloop_p4=true)
part() {  # the OLD settings with part $1 at its TW5 value
  local a=("${OLD[@]}") i
  case $1 in
    a) a[1]='optimize.oscillation="bounded"' ;;
    b) a[3]='optimize.escape="layering"'; a[5]='optimize.escape_take="always"' ;;
    c) a[9]='optimize.loop_history=true' ;;
    d) a[11]='optimize.float_wire=true' ;;
    e) a[13]='optimize.preloop_p4=false' ;;
  esac
  printf '%s\n' "${a[@]}"
}
one() {
  local c=${1%%:*} rest=${1#*:}; local b=${rest%%:*} th=1
  [ "$rest" != "$b" ] && th=${rest#*:}
  local SETS=(--set optimize.runs=1) seed=0 o=${c%%r*} rr=${c#*r}
  local n=${rr%%s*}; [ "$rr" != "$n" ] && seed=${rr#*s}
  case $o in
    O) SETS+=("${OLD[@]}") ;;
    N) ;;
    N[a-e]) mapfile -t x < <(part ${o#N}); SETS+=("${x[@]}") ;;
    *) note "unknown cfg $c"; return ;;
  esac
  SETS+=(--set optimize.rounds=$n --set optimize.stable_patience=$n)
  [ "$seed" != 0 ] && SETS+=(--set optimize.seed=$seed)
  local out=$R/$c/$b; mkdir -p $out
  local t0=$(date +%s)
  env RAYON_NUM_THREADS=$th TOPO_TIMES=1 tools/route_board.sh "$(board_path $b)" $out --clock cpu --timeout 2400 --no-png "${SETS[@]}" \
    > $R/$c-$b.log 2>&1 9>&-
  note "run $c $b th $th rc $? wall $(( $(date +%s) - t0 )) s"
}
lane() { for x in "$@"; do one "$x"; done; }
u() {  # name threads "lane1 runs" "lane2 runs" ...
  local n=$1 t=$2; shift 2
  local cmd="" l
  for l in "$@"; do cmd+="bash $ROOT/tools/tw5_job.sh lane $l & "; done
  cmd+="wait"
  printf '%s\n' "$cmd" > $R/unit-$n.sh
  ( hzq run --threads $t --name $n -- bash $R/unit-$n.sh > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & sleep 1
}
tests() {
  cd $ROOT/engine
  cargo test --release --lib -- escape:: layers::tests 9>&- > $R/cargo-lib.log 2>&1; note "cargo lib rc $? $(grep 'test result' $R/cargo-lib.log | tail -1)"
  cargo test --release --test topo_p5_loop --test loop_local --test topo_g0 9>&- > $R/cargo-int.log 2>&1; note "cargo int rc $? $(grep -c 'test result: ok' $R/cargo-int.log) ok binaries"
  cd $ROOT
}
case ${1:-} in
  one) one "$2" ;;
  lane) shift; lane "$@" ;;
  tests) tests ;;
  r1)
    # Round 1: O = main copper (rounds=1 vs nd2b2-dcf4c0e), N first numbers (rounds 1 and 16), targeted tests.
    u q 4 "Or1:qmtech" "Nr1:qmtech" "Nr16:qmtech:2"
    u e 4 "Or1:esp Or1:wiccon Or1:eprom" "Nr1:esp Nr1:wiccon Nr1:eprom" "Nr16:esp" "Nr16:wiccon Nr16:eprom"
    ( hzq run --threads 2 --name tw5-tests -- bash $ROOT/tools/tw5_job.sh tests > $R/tests.log 2>&1; note "tests unit rc $?" ) & sleep 1
    wait; note "all done" ;;
  *) echo "usage: see header" ;;
esac
