#!/usr/bin/env bash
# bundle-turn r3 step 1 (RCA of the user's qm detours): det s0 C1 on qmtech-bh26 (the user's Board Setup rules).
#   top2: item attribution arms: fork zones off (TOPO_FD45_OFF=fork), oblique_price 0
#   top: tip clean (metrics), tip dbg (TOPO_XRDBG + TOPO_LOOPDBG: crossing-pass groups, loop reroute tries), tip split3 (XRDBG only)
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $OUT/rc.txt; }
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
BH=scratch/boards/qmtech-bh26/qmtech-bh26.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
run() { local a=$1; shift; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a" "$BH" --only 0 --no-png --threads 4 $C1 "$@" > "$OUT/$a.log" 2>&1 9>&-; note "$a rc $?"; gzip -f "$OUT/$a.log"; }
case "${1:-}" in
  clean) run tip-bh ;;
  dbg) TOPO_XRDBG=1 TOPO_LOOPDBG=1 run dbg-bh ;;
  split) TOPO_XRDBG=1 run split3-bh --set optimize.crossing_split=3 ;;
  fork) TOPO_FD45_OFF=fork run fork-bh ;;
  obl0) run obl0-bh --set capacity.oblique_price=0 ;;
  prov) TOPO_RUNPROV=1 TOPO_STRDBG=1 TOPO_OCTIDBG=1 TOPO_LANEDBG=1 run prov-bh; zcat $OUT/prov-bh.log.gz >/dev/null; L=$OUT/prov-bh/qmtech-bh26-s0.log
        grep -E "w845|wire 845|w222|wire 222" $L > $OUT/prov-w845-w222.txt; grep -E "^(RUNPROV|OCTI|STR)" $L | cut -c1-300 | gzip > $OUT/prov-all.txt.gz; gzip -f $L ;;
  top3) hzq run --threads 4 --name bt3-prov -- bash $ROOT/tools/bundle_turn_r3_rca.sh prov 9>&-; note "all done" ;;
  board) # board ARM NAME PCB THREADS: det s0 C1 of one board
    TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$2-$3" "$4" --only 0 --no-png --threads $5 $C1 > "$OUT/$2-$3.log" 2>&1 9>&-; note "$2-$3 rc $?"; gzip -f "$OUT/$2-$3.log" ;;
  tests)
    rustup component add clippy >/dev/null 2>&1
    cargo clippy --release --all-targets > $OUT/clippy.log 2>&1 9>&-; note "clippy rc $? warnings $(grep -c '^warning' $OUT/clippy.log)"
    cargo test --release -p topor-engine --lib straighten > $OUT/t-lib.log 2>&1 9>&-; note "lib straighten rc $?"
    for t in topo_first_draw_45 topo_bundle_turn loopdbg_copper topo_neck_regrow; do cargo test --release -p topor-engine --test $t > $OUT/t-$t.log 2>&1 9>&-; note "$t rc $? $(grep -h '^test result' $OUT/t-$t.log | tr '\n' ' ')"; done
    # mutant: the skip removed (own tree + target dir)
    M=/root/bt3-mut; rm -rf $M; cp -r $ROOT $M; cd $M; touch engine/src/topo/octi.rs
    python3 - <<'PY'
p='engine/src/topo/octi.rs'; s=open(p).read(); o='            if self.refit_counts(wo, r).is_none() {\n                if dbg {'
assert o in s; s=s.replace(o,'            if false && self.refit_counts(wo, r).is_none() {\n                if dbg {',1); open(p,'w').write(s)
PY
    CARGO_TARGET_DIR=$M/target cargo test --release -p topor-engine --lib straightening_skips > $OUT/mut-skip.log 2>&1 9>&-; note "mutant no-skip rc $? (expect != 0: killed)" ;;
  vnew)
    hzq run --threads 4 --name bt3d-qm -- bash $ROOT/tools/bundle_turn_r3_rca.sh board new qmbh $BH 4 9>&- & sleep 2
    hzq run --threads 4 --name bt3d-ew -- bash -c "bash $ROOT/tools/bundle_turn_r3_rca.sh board new esp $ESP 2 & bash $ROOT/tools/bundle_turn_r3_rca.sh board new wic $WIC 2 & wait" 9>&- & sleep 2
    hzq run --threads 6 --name bt3d-tests -- bash $ROOT/tools/bundle_turn_r3_rca.sh tests 9>&- &
    wait; note "all done" ;;
  vbase)
    hzq run --threads 4 --name bt3e-ew -- bash -c "bash $ROOT/tools/bundle_turn_r3_rca.sh board base esp $ESP 2 & bash $ROOT/tools/bundle_turn_r3_rca.sh board base wic $WIC 2 & wait" 9>&-
    note "all done" ;;
  top2)
    for u in fork obl0; do hzq run --threads 4 --name bt3-$u -- bash $ROOT/tools/bundle_turn_r3_rca.sh $u 9>&- & sleep 3; done
    wait; note "all done" ;;
  top)
    for u in clean dbg split; do hzq run --threads 4 --name bt3-$u -- bash $ROOT/tools/bundle_turn_r3_rca.sh $u 9>&- & sleep 3; done
    wait; note "all done" ;;
  *) echo "usage: $0 top|clean|dbg|split" >&2; exit 2 ;;
esac
