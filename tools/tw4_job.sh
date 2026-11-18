#!/usr/bin/env bash
# tw4_job.sh (branch tw4-eval, docs/dev/design/tw4-eval.md): TW4 EVAL phase-1 baseline on the VM.
#   tw4_job.sh base           start the hzq units (inside hz.sh job), wait, note "all done"
#   tw4_job.sh lane <run>...  one lane inside a unit: the runs one after another
#   tw4_job.sh one <run>      one run; <run> = <cfg>:<board>[:<threads>]
# Every run: harness.score_runs (cpu clock, 2400 s, [capacity] compass_cut_check on), seed 0 unless the cfg names one,
# optimize.runs=1, --no-png. realise = erbs (the tw2-metric default, set explicitly).
# cfg:
#   JK          rounds=1 (one fixed loop cycle), TOPO_ERBS_DEBUG=1 (per-run source of the ERBS drawing)
#   JF          JK without never-drop forcing (optimize.allow_violations=false): the open connections = the forced ones
#   JPK / JPF   JK / JF with [search] pad_exit_stubs = true (5.10, the existing pad-exit plan object)
#   r<N>s<k>    rounds=N, stable_patience=N (no stable stop before N cycles), seed k (0 = the default run)
# Out: $HZ_RUN_DIR/<cfg>/<board>/<board>/s<k>/ (score_runs layout), logs $HZ_RUN_DIR/<cfg>-<board>.log.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; esac; }
one() {
  local c=${1%%:*} rest=${1#*:}; local b=${rest%%:*} th=1
  [ "$rest" != "$b" ] && th=${rest#*:}
  local SETS=(--set 'drawing.realise="erbs"' --set optimize.runs=1) ENVS=() only=0 cc=$c CLK=()
  # suffix v: [capacity] via_cuts = true (tw4 T2), with TOPO_VIACHECK (stored vs fresh via prices)
  # suffix n: [factors] neck_depth = 1000 (tw4 T1, the violation weight); suffixes combine (vn)
  # suffix c: [search] dangling_lead_choice = true (tw4 T3); suffix d: [factors] dangling_turn = 4 (not with n)
  while :; do
    if [ "${c%v}" != "$c" ]; then SETS+=(--set capacity.via_cuts=true); ENVS+=(TOPO_VIACHECK=1); c=${c%v}
    elif [ "${c%n}" != "$c" ]; then SETS+=(--set 'factors.weight={"neck_depth": 1000}'); c=${c%n}
    elif [ "${c%c}" != "$c" ]; then SETS+=(--set search.dangling_lead_choice=true); c=${c%c}
    elif [ "${c%d}" != "$c" ]; then SETS+=(--set 'factors.weight={"dangling_turn": 4}'); c=${c%d}
    else break; fi
  done
  case $c in
    JK) SETS+=(--set optimize.rounds=1); ENVS+=(TOPO_ERBS_DEBUG=1) ;;
    JF) SETS+=(--set optimize.rounds=1 --set optimize.allow_violations=false) ;;
    JPK) SETS+=(--set optimize.rounds=1 --set search.pad_exit_stubs=true); ENVS+=(TOPO_ERBS_DEBUG=1) ;;
    JPF) SETS+=(--set optimize.rounds=1 --set search.pad_exit_stubs=true --set optimize.allow_violations=false) ;;
    e*) only=${c#e}   # equal time: wall 300 s, the loop runs until its time share (no round cap, no stable stop)
        SETS+=(--set optimize.rounds=1000 --set optimize.stable_patience=1000); CLK=(--clock wall --timeout 300) ;;
    r*s*) local n=${c#r}; n=${n%%s*}; only=${c##*s}
          SETS+=(--set optimize.rounds=$n --set optimize.stable_patience=$n) ;;
    *) note "unknown cfg $c"; return ;;
  esac
  c=$cc
  local bt=$b; [ "$th" -ge 4 ] && [ "$c" != "${c#J}" ] && bt=$b-t$th   # J* runs at 4+ threads: own dir (thread check)
  local o=$R/$c/$bt; mkdir -p $o
  local t0=$(date +%s)
  env "${ENVS[@]}" python -m harness.score_runs $o "$(board_path $b)" --only $only --no-png --threads $th "${CLK[@]}" "${SETS[@]}" \
    > $R/$c-$bt.log 2>&1 9>&-
  note "run $c $bt th $th rc $? wall $(( $(date +%s) - t0 )) s"
  rm -rf $o/*/s*/work 2>/dev/null
}
lane() { for x in "$@"; do one "$x"; done; }
u() {  # name threads "lane1 runs" "lane2 runs" ...
  local n=$1 t=$2; shift 2
  local cmd="" l
  for l in "$@"; do cmd+="bash $ROOT/tools/tw4_job.sh lane $l & "; done
  cmd+="wait"
  printf '%s\n' "$cmd" > $R/unit-$n.sh
  ( hzq run --threads $t --name $n -- bash $R/unit-$n.sh > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & sleep 1
}
case ${1:-} in
  one) one "$2" ;;
  lane) shift; lane "$@" ;;
  base)
    # long units first (qmtech), small ones bundled; every unit declares the threads its lanes use
    u q-k 4 "JK:qmtech:2" "JF:qmtech:2"
    u q-p 4 "JPK:qmtech:2" "JPF:qmtech:2"
    u q-r16 4 "r16s0:qmtech:4"
    u e-long 4 "r200s0:esp" "r64s0:esp" "r16s0:esp r4s0:esp" "r1s1:esp r1s2:esp"
    u e-seeds 4 "r1s3:esp r1s4:esp" "r16s1:esp" "r16s2:esp" "r16s3:esp r16s4:esp"
    u small 4 "JK:esp JF:esp" "JPK:esp JPF:esp" "JK:wiccon JF:wiccon JPK:wiccon JPF:wiccon" \
      "JK:eprom JF:eprom JPK:eprom JPF:eprom"
    wait; note "all done" ;;
  t2)
    # TW4 T2 (CM1 on the incremental measure): targeted tests; off = main copper (JK vs the tw4b base);
    # on: VIACHECK, thread independence (esp th 1 vs 4), cycle cost r16 off/on (esp s0-s2, qmtech, wiccon)
    ( cd engine && cargo test --release --test topo_cm1_room --test topo_compass --test topo_exits --test par_refine --test loop_local 9>&- > $R/cargo.log 2>&1; note "cargo rc $?" )
    ( cd engine && cargo test --release --lib viacut 9>&- > $R/cargo-lib.log 2>&1; note "cargo lib rc $?" )
    u t2-q 4 "JK:qmtech:2" "JKv:qmtech:2"
    u t2-qr 4 "r16s0:qmtech:2" "r16s0v:qmtech:2"
    u t2-s 4 "JK:esp JK:wiccon JK:eprom" "JKv:esp JKv:wiccon JKv:eprom" "JKv:esp:4" "r16s0:wiccon r16s0v:wiccon"
    u t2-e 4 "r16s0:esp r16s1:esp" "r16s0v:esp r16s1v:esp" "r16s2:esp" "r16s2v:esp"
    wait
    # mutant: a rejected move that keeps its trial via points must fail the freshness test
    cp engine/src/topo/refine.rs $R/refine.rs.orig
    sed -i 's/^        cm.vpts\[s.w\] = s.pts;$/        \/\/ mutant/' engine/src/topo/refine.rs
    if grep -q "// mutant" engine/src/topo/refine.rs; then
      ( cd engine && cargo test --release --test topo_cm1_room via_prices 9>&- > $R/mutant.log 2>&1; note "mutant (expect != 0) rc $?" )
    else note "mutant not applied"; fi
    cp $R/refine.rs.orig engine/src/topo/refine.rs
    note "all done" ;;
  eq)
    # TW4 T2 at equal time (lead 2026-11-18): via_cuts off vs on, wall 300 s, 4 threads, esp s0-s4 + qmtech s0
    u eq-1 16 "e0:esp:4" "e0v:esp:4" "e1:esp:4" "e1v:esp:4"
    u eq-2 16 "e2:esp:4" "e2v:esp:4" "e3:esp:4" "e3v:esp:4"
    u eq-3 16 "e4:esp:4" "e4v:esp:4" "e0:qmtech:4" "e0v:qmtech:4"
    wait; note "all done" ;;
  t1)
    # TW4 T1 part 1 ([factors] neck_depth): targeted tests; off = main copper (JK 4 boards, r16s0 esp vs tw4t2b);
    # A/B neck_depth 1000 per connection: JK 4 boards, r16 esp s0-s2, r16 qmtech, r16 wiccon
    ( cd engine && cargo test --release --lib factors 9>&- > $R/cargo-lib.log 2>&1; note "cargo lib rc $?" )
    ( cd engine && cargo test --release --test topo_compass --test loop_local --test topo_cm1_room 9>&- > $R/cargo.log 2>&1; note "cargo rc $?" )
    u t1-q 4 "JK:qmtech:2" "JKn:qmtech:2"
    u t1-qr 4 "r16s0:qmtech:2" "r16s0n:qmtech:2"
    u t1-s 4 "JK:esp JK:wiccon JK:eprom" "JKn:esp JKn:wiccon JKn:eprom" "r16s0:wiccon r16s0n:wiccon" "r16s1n:esp"
    u t1-e 4 "r16s0:esp r16s2n:esp" "r16s0n:esp"
    wait; note "all done" ;;
  t3)
    # TW4 T3 ([search] dangling_lead_choice): off = today's copper (JK 4 boards vs tw4t2d); on (c) on the 4 boards
    # (no kept copper there: score_runs strips the board) and on the private badge with its copper kept (route_board
    # --keep-copper, rounds=1): off / c / cd (dangling_turn 4); the proposed pattern dangling_lead_away: boards, PNG,
    # KiCad DRC of both boards, and its problem board routed off / cd (plugin flow, PNG)
    for n in off c cd; do
      ( hzq run --threads 2 --name t3-badge-$n -- bash tools/tw4_job.sh badge $n > $R/badge-$n.log 2>&1; note "badge $n rc $?" ) & sleep 1
    done
    u t3-q 4 "JK:qmtech:2" "JKc:qmtech:2"
    u t3-s 2 "JK:esp JK:wiccon JK:eprom" "JKc:esp JKc:wiccon JKc:eprom"
    ( hzq run --threads 1 --name t3-pat -- bash tools/tw4_job.sh pattern > $R/pattern.log 2>&1; note "pattern rc $?" ) & sleep 1
    wait; note "all done" ;;
  badge)
    # one private-badge run (copper kept, rounds=1): off | c (dangling_lead_choice) | cd (c + dangling_turn 4)
    BADGE=$(ls scratch/boards/xt-badge/*.kicad_pcb | head -1)
    A=(--keep-copper --no-png --set optimize.rounds=1)
    case $2 in
      off) ;;
      c) A+=(--set search.dangling_lead_choice=true) ;;
      cd) A+=(--set search.dangling_lead_choice=true --set 'factors.weight={"dangling_turn": 4}') ;;
    esac
    tools/route_board.sh "$BADGE" $R/badge/$2 "${A[@]}" > $R/badge-$2.txt 2>&1 9>&- ;;
  pattern)
    P=$R/patterns
    python -c "
from pathlib import Path
from harness import fit_patterns as fp
from harness.drc import run_drc
for k, v in fp.generate('dangling_lead_away', Path('$P')).items(): print(k, v)
d = Path('$P/dangling_lead_away')
for kind in ('problem', 'reference'):
    pb = d / kind / 'dangling_lead_away.kicad_pcb'
    r = run_drc(pb, pb.with_name('drc.json'))
    print(kind, 'DRC violations', [(getattr(v, 'type', ''), v.description) for v in r.violations], 'unconnected', len(r.unconnected))
"
    PB=$P/dangling_lead_away/problem/dangling_lead_away.kicad_pcb
    tools/route_board.sh $PB $P/routed-cd --keep-copper --set optimize.rounds=1 --set search.dangling_lead_mm=1.5 \
      --set search.dangling_lead_choice=true --set 'factors.weight={"dangling_turn": 4}' > $P/routed-cd.txt 2>&1; echo "routed cd rc $?"
    tools/route_board.sh $PB $P/routed-off15 --keep-copper --set optimize.rounds=1 --set search.dangling_lead_mm=1.5 \
      --set 'factors.weight={"dangling_turn": 4}' > $P/routed-off15.txt 2>&1; echo "routed off15 rc $?" ;;
  hooks)
    # TW4 unit 1 (meter hooks): targeted tests, then rounds=1 copper (must equal the tw4b base) + the new compass items
    ( cd engine && cargo test --release --test topo_compass --test topo_exits 9>&- > $R/cargo.log 2>&1; note "cargo rc $?" )
    ( cargo test --release --manifest-path engine/Cargo.toml --lib compass 9>&- > $R/cargo-lib.log 2>&1; note "cargo lib rc $?" )
    ( python -m pytest -q tests/test_compass.py > $R/pytest.log 2>&1; note "pytest rc $?" )
    u h-q 4 "JK:qmtech:2" "JPK:qmtech:2"
    u h-s 4 "JK:esp" "JPK:esp" "JK:wiccon JK:eprom" "JPK:wiccon"
    wait; note "all done" ;;
  *) echo "usage: tw4_job.sh base | lane <cfg:board[:threads]>... | one <cfg:board[:threads]>" >&2; exit 2 ;;
esac
