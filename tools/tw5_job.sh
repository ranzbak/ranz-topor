#!/usr/bin/env bash
# tw5_job.sh (branch tw5-escape, docs/dev/design/tw5-escape.md): TW5 ESCAPE runs on the VM.
#   tw5_job.sh <mode>          start the hzq units of <mode> (inside hz.sh job), wait, note "all done"
#   tw5_job.sh lane <run>...   one lane inside a unit: the runs one after another
#   tw5_job.sh one <run>       one run; <run> = <cfg>:<board>[:<threads>]
#   tw5_job.sh tests           targeted release tests (lib escape/layers, loop integration binaries)
# Every board run: tools/route_board.sh (as the main baseline nd2b2-dcf4c0e), cpu clock 2400 s, optimize.runs=1, --no-png.
# <board> = eprom|wiccon|esp|qmtech|bornhack, p-<pattern> (generated into $HZ_RUN_DIR/pat), q-<pattern> = p- with --declared-layers (In1.Cu routed),
#           d-<pattern> = a PROPOSED D8 pattern (harness/tw5_patterns.py, generated into $HZ_RUN_DIR/d8),
#           e-<X> = esp with min_copper_edge_clearance X mm (edge_boards).
# cfg = <opts><rounds>[s<seed>]:
#   opts  O = every TW5 option at its old value (the defaults since fa0b28f0+1; must give main's copper), N = every TW5 option on,
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
board_path() { case $1 in eprom) echo $P;; wiccon) echo $W;; esp) echo $E;; qmtech) echo $Q;; bornhack) echo $B;; p-*|q-*) echo $R/pat/${1#?-}/problem/${1#?-}.kicad_pcb;; d-*) echo $R/d8/${1#d-}/problem/${1#d-}.kicad_pcb;; e-*) echo $R/edge/e${1#e-}/esp-rust-board.kicad_pcb;; esac; }
edge_boards() {  # esp copies with min_copper_edge_clearance X (reports/edge-clear-20261117-1800.md: predicted over-full 10 -> 131)
  local S=$ROOT/$(dirname $E) x
  for x in "$@"; do
    mkdir -p $R/edge/e$x; cp $S/esp-rust-board.kicad_pcb $S/esp-rust-board.kicad_prl $R/edge/e$x/ 2>/dev/null
    sed "s/\"min_copper_edge_clearance\": [0-9.]*/\"min_copper_edge_clearance\": $x/" $S/esp-rust-board.kicad_pro > $R/edge/e$x/esp-rust-board.kicad_pro
    grep -q "\"min_copper_edge_clearance\": $x" $R/edge/e$x/esp-rust-board.kicad_pro || note "edge board $x: sed failed"
  done
}
VIA="via_pair_redundant via_pair_slide via_force_relocate branch_point_relocate branch_point_into_pad via_size_rule"
gen() {  # the synthetic patterns (fit all, via approved 6, radii all) into $R/pat, as looplocal_job.sh
  [ -d $R/pat ] && return
  mkdir -p $R/pat
  PYTHONPATH=$ROOT:$ROOT/plugin python - "$R/pat" $VIA > $R/gen.log 2>&1 <<'PY'
import sys
from pathlib import Path
from harness import fit_patterns, via_patterns, radii_patterns
out = Path(sys.argv[1])
for n in fit_patterns.PATTERNS:
    fit_patterns.generate(n, out, png=False)
for n in sys.argv[2:]:
    via_patterns.generate(n, out, png=False, drc=False)
for n in radii_patterns.PATTERNS:
    radii_patterns.generate(n, out, png=False, drc=False)
PY
  note "gen rc $? patterns: $(ls $R/pat | tr '\n' ' ')"
}
OLD=(--set 'optimize.oscillation="cooling"' --set 'optimize.escape="ties"' --set 'optimize.escape_take="not_worse"'
     --set optimize.relay_count_fade=1.0 --set optimize.loop_history=false --set optimize.float_wire=false --set optimize.preloop_p4=true)
NEW=(--set 'optimize.oscillation="bounded"' --set 'optimize.escape="layering"' --set 'optimize.escape_take="always"'
     --set optimize.relay_count_fade=0.8 --set optimize.loop_history=true --set optimize.float_wire=true --set optimize.preloop_p4=false)
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
    N) SETS+=("${NEW[@]}") ;;
    N[a-e]) mapfile -t x < <(part ${o#N}); SETS+=("${x[@]}") ;;
    F) SETS+=(--set optimize.free_wire_cross=0.0) ;;  # the defaults (TW5 off) with crossing-free wires crossed at their freedom
    NF) SETS+=("${NEW[@]}" --set optimize.free_wire_cross=0.0) ;;
    Nt) x=("${NEW[@]}"); x[5]='optimize.escape_take="not_worse"'; SETS+=("${x[@]}") ;;  # N with whole-board moves undone when worse
    *) note "unknown cfg $c"; return ;;
  esac
  SETS+=(--set optimize.rounds=$n --set optimize.stable_patience=$n)
  case $b in q-*) SETS+=(--declared-layers) ;; esac  # q-<pattern>: every declared signal layer routed (4-layer default: In1 gnd)
  [ "$seed" != 0 ] && SETS+=(--set optimize.seed=$seed)
  local out=$R/$c/$b; mkdir -p $out
  local t0=$(date +%s)
  env RAYON_NUM_THREADS=$th TOPO_TIMES=1 tools/route_board.sh "$(board_path $b)" $out --clock cpu --timeout 2400 --no-png "${SETS[@]}" \
    > $R/$c-$b.log 2>&1 9>&-
  note "run $c $b th $th rc $? wall $(( $(date +%s) - t0 )) s"
  rm -rf $out/work $out/*/work 2>/dev/null
}
lane() { for x in "$@"; do one "$x"; done; }
u() {  # name threads "lane1 runs" "lane2 runs" ...
  local n=$1 t=$2; shift 2
  local cmd="" l
  for l in "$@"; do cmd+="bash $ROOT/tools/tw5_job.sh lane $l & "; done
  cmd+="wait"
  printf '%s\n' "$cmd" > $R/unit-$n.sh
  ( hzq run --pin --threads $t --name $n -- bash $R/unit-$n.sh > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & sleep 1
}
mut() {  # name file test-args... ; the python snippet on stdin edits the file (as looplocal_job.sh)
  local n=$1 f=$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  ( cd $ROOT/engine && cargo test --release "$@" 9>&- > $R/mut-$n.log 2>&1 ); local rc=$?
  mv $f.orig $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
sub() { cat <<EOF2
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF2
}
mutants() {  # the TW5 changed lines; each mutant against the tests that guard it
  local E=$ROOT/engine/src/topo/escape.rs L=$ROOT/engine/src/topo/layers.rs C=$ROOT/engine/src/topo/core.rs
  local UE=(--lib -- escape::) UL=(--lib -- layers::tests) IL=(--test loop_local) IT=(--test tw5_escape)
  sub 't.need = t.cost(alive) * (1.0 / share - 1.0);' 't.need = 0.0;' | mut e1_first_wait $E "${UE[@]}"
  sub 'self.need = w * (1.0 / self.share - 1.0);' 'self.need = w * (0.5 / self.share - 1.0);' | mut e2_share $E "${UE[@]}"
  sub '*h *= self.fade;' '*h *= 1.0;' | mut e3_fade $E "${UE[@]}"
  sub 'self.h[e] += (1.0 + d) / (2.0 + d);' 'self.h[e] += 0.5;' | mut e4_depth $E "${UE[@]}"
  sub 'lo + (hi - lo) * b / (1.0 + b)' 'lo + (hi - lo) * b' | mut e5_amp $E "${UE[@]}"
  sub 'd += usize::from(x[k] != y[k]);' 'd += 1;' | mut e6_diff $E "${UE[@]}"
  sub 'if delta <= EPS {' 'if delta <= 1e9 {' | mut l1_walk_up $L "${UL[@]}"
  sub 'let p4 = if cost(o, x, u, w, false) == 0.0 { 2 } else { 1 };' 'let p4 = if cost(o, x, u, w, false) == 0.0 { 2 } else { 0 };' | mut l2_via_edges $L "${UL[@]}"
  sub 'if !reaches0 && rng.gen_range(0..2u8) == 1 {' 'if false {' | mut l3_parts $L "${UL[@]}"
  sub 'rep.walked = g0.plateau_walk(&mut o, p.walk.saturating_mul(rep.domains.max(1)), p.ties);' 'let _ = g0;' | mut l4_two_walk $L "${IL[@]}" the_layering_walk
  sub '        if c <= best + EPS {
            rep.walked' '        if false {
            rep.walked' | mut l5_n_pair $L "${IL[@]}" the_layering_walk
  sub 'cs.layer_kept += usize::from(self.take_layers(lr, seed));' 'let _ = lr;' | mut c1_take $C "${IL[@]}" layering_escapes
  sub 'if self.tw5.escape && self.tw5.throttle.due() && !stop() {' 'if self.tw5.escape && !stop() {' | mut c2_throttle $C "${IL[@]}" escapes
  sub 'let p4_rounds_max = if opts.opt_loop == "topor" && !opts.preloop_p4 { 0 } else { opts.reroute_rounds };' 'let p4_rounds_max = opts.reroute_rounds;' | mut c3_preloop $C "${IT[@]}"
  sub '                self.history_step(cs);
                cs.layer_s += tl.elapsed().as_secs_f64();' '                cs.layer_s += tl.elapsed().as_secs_f64();' | mut c4_history $C "${IL[@]}" layering_escapes
  note "mutants done"
}
tests() {
  cd $ROOT/engine
  cargo test --release --lib -- escape:: layers::tests 9>&- > $R/cargo-lib.log 2>&1; note "cargo lib rc $? $(grep 'test result' $R/cargo-lib.log | tail -1)"
  cargo test --release --no-fail-fast --test topo_p5_loop --test loop_local --test topo_g0 --test tw5_escape -- --nocapture 9>&- > $R/cargo-int.log 2>&1; note "cargo int rc $? $(grep -c 'test result: ok' $R/cargo-int.log) ok binaries"
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
  mutants) mutants ;;
  d8)
    # The PROPOSED D8 patterns (not in the suite): generate with KiCad DRC of the reference, route N (16 cycles) seeds 0-9
    # and O seed 0, PNG expected | topor + per-seed reach table.
    mkdir -p $R/d8; PYTHONPATH=$ROOT:$ROOT/plugin python -m harness.tw5_patterns $R/d8 --drc > $R/d8-gen.log 2>&1; note "d8 gen rc $?"
    l1=""; l2=""
    for n in layering_zero_vias oscillating_cross layering_escape_fig8; do
      for s_ in 0 1 2 3 4; do l1+="Nr16s$s_:d-$n "; done; for s_ in 5 6 7 8 9; do l2+="Nr16s$s_:d-$n "; done; l1+="Or16s0:d-$n "
    done
    u d 2 "$l1" "$l2"; wait
    PYTHONPATH=$ROOT:$ROOT/plugin python -m harness.tw5_patterns_png $R/d8 $R $R/d8out > $R/d8-png.log 2>&1; note "d8 png rc $? $(tr '\n' ';' < $R/d8-png.log)" ;;
  r3)
    # Round 3: tests (new tw5_escape + loop_local tests), mutants, inner_layer_room with its declared layers routed
    # (the pattern's KiCad types are all "signal", so topor's 4-layer default makes In1 gnd), O vs N, seeds 0-9.
    gen
    ( hzq run --threads 4 --name tw5-tests -- bash -c "bash $ROOT/tools/tw5_job.sh tests; bash $ROOT/tools/tw5_job.sh mutants" > $R/tests.log 2>&1; note "tests unit rc $?" ) & sleep 1
    lo=""; ln=""
    for s_ in 0 1 2 3 4 5 6 7 8 9; do lo+="Or16s$s_:q-inner_layer_room "; ln+="Nr16s$s_:q-inner_layer_room "; done
    u i 2 "$lo" "$ln"
    u q 4 "Nr16:qmtech:2" "Nr16s1:esp Nr16s2:esp" "Nr16s3:esp Nr16s4:esp"
    u e 2 "Nr16:esp Nr16:wiccon Nr16:eprom" "Nr1:esp Nr1:wiccon Nr1:eprom"
    wait; note "all done" ;;
  r4)
    # Round 4 (tip): tests + mutants; O = main copper at 1 cycle (vs nd2b2-dcf4c0e); N 16 cycles ladder (esp 5 seeds);
    # inner_layer_room declared layers N seeds 0-9; the D8 proposals (generate, route, PNG).
    ( hzq run --threads 4 --name tw5-tests -- bash -c "bash $ROOT/tools/tw5_job.sh tests; bash $ROOT/tools/tw5_job.sh mutants" > $R/tests.log 2>&1; note "tests unit rc $?" ) & sleep 1
    gen
    u o 4 "Or1:esp Or1:eprom" "Or1:wiccon" "Or1:qmtech" "Nr16:wiccon Nr16:eprom"
    u q 2 "Nr16:qmtech:2"
    u e 5 "Nr16:esp" "Nr16s1:esp" "Nr16s2:esp" "Nr16s3:esp" "Nr16s4:esp"
    ln=""; for s_ in 0 1 2 3 4 5 6 7 8 9; do ln+="Nr16s$s_:q-inner_layer_room "; done
    u i 1 "$ln"
    ( bash $ROOT/tools/tw5_job.sh d8 > $R/d8.log 2>&1 ) & sleep 1
    wait; note "all done" ;;
  r5)
    # Round 5 (escape refinement adds priced vias): N 16 cycles ladder (esp 5 seeds), inner_layer_room N seeds 0-9, D8.
    gen
    u q 2 "Nr16:qmtech:2"
    u e 5 "Nr16:esp Nr16:wiccon Nr16:eprom" "Nr16s1:esp" "Nr16s2:esp" "Nr16s3:esp" "Nr16s4:esp"
    ln=""; for s_ in 0 1 2 3 4 5 6 7 8 9; do ln+="Nr16s$s_:q-inner_layer_room "; done
    u i 1 "$ln"
    ( bash $ROOT/tools/tw5_job.sh d8 > $R/d8.log 2>&1 ) & sleep 1
    wait; note "all done" ;;
  r8)
    # Round 8 (free_wire_cross): targeted tests + its mutants; O = main copper at 1 cycle (vs tw5oc-fa0b28f);
    # F / NF on the D8 boards (seeds 0-4) and F at 16 cycles on esp s1, wiccon, qmtech.
    mkdir -p $R/d8; PYTHONPATH=$ROOT:$ROOT/plugin python -m harness.tw5_patterns $R/d8 --drc > $R/d8-gen.log 2>&1; note "d8 gen rc $?"
    l1=""; for n in layering_zero_vias oscillating_cross layering_escape_fig8; do for s_ in 0 1 2 3 4; do l1+="Fr16s$s_:d-$n NFr16s$s_:d-$n "; done; done
    u d 2 "$l1"
    u o 4 "Or1:esp" "Or1:eprom Or1:wiccon" "Or1:qmtech"
    u f 6 "Fr16s1:esp Fr16:wiccon Fr16:eprom" "Fr16:qmtech:2"
    ( hzq run --pin --threads 2 --name fw-tests -- bash -c "cd $ROOT/engine && cargo test --release --no-fail-fast --test loop_local --test tw5_escape --test topo_c0 -- --nocapture > $R/cargo-fw.log 2>&1; echo rc \$? >> $R/cargo-fw.log" > /dev/null 2>&1
      note "fw tests: $(grep -E '^test result' $R/cargo-fw.log | tr '\n' ' ') $(tail -1 $R/cargo-fw.log)"
      hzq run --pin --threads 2 --name fw-mut -- bash $ROOT/tools/tw5_job.sh mutfw > $R/mutfw.log 2>&1; note "mutfw rc $?" ) & sleep 1
    wait
    PYTHONPATH=$ROOT:$ROOT/plugin python -m harness.tw5_patterns_png $R/d8 $R $R/d8out > $R/d8-png.log 2>&1; note "d8 png rc $? $(tr '\n' ';' < $R/d8-png.log)"
    note "all done" ;;
  mutfw)
    I=$ROOT/engine/src/topo/init.rs
    sub 'cross += wt.cross_mm * f' 'cross += wt.cross_mm' | mut f1_share $I --test loop_local a_wire_that_crosses_nothing
    sub 'if self.free_now.get(w).copied().unwrap_or(false) {' 'if false {' | mut f2_free_held $I --test loop_local a_wire_that_crosses_nothing
    sub 'self.cross_k = (0..self.wires.len()).map(|w| a + (1.0 - a) * k(w) as f64 / (1.0 + k(w) as f64)).collect();' 'self.cross_k = (0..self.wires.len()).map(|_| a).collect();' | mut f3_graded $I --test loop_local a_wire_that_crosses_nothing ;;
  r7)
    # Round 7 (TW5 default off, main 1503c13b merged, inner_layer_room declares In2.Cu power): the fit fixtures
    # regenerated (In1 routed) into the tree, then the merge trial; qmtech 16 cycles single-part ablations for the
    # via-site loss (Nb, Nt, Na); inner_layer_room O vs N seeds 0-9 (KiCad types, no --declared-layers); D8.
    # Units pinned (tw5r4/tw5r6: moved units' outputs did not come back).
    mkdir -p $R/fitpat $R/fixtures
    PYTHONPATH=$ROOT:$ROOT/plugin python - $R/fitpat > $R/fitpat.log 2>&1 <<'PY'
import sys
from pathlib import Path
from harness import fit_patterns
for n in fit_patterns.PATTERNS:
    fit_patterns.generate(n, Path(sys.argv[1]), png=False)
PY
    note "fitpat rc $?"
    PYTHONPATH=$ROOT:$ROOT/plugin python -m harness.fit_fixtures $R/fitpat $R/fixtures > $R/fixtures.log 2>&1; note "fixtures rc $?"
    for f in $R/fixtures/*.json; do cmp -s $f engine/tests/fixtures/fit/$(basename $f) || note "fixture changed: $(basename $f)"; done
    cp $R/fixtures/*.json engine/tests/fixtures/fit/
    gen
    l1=""; l2=""; for s_ in 0 1 2 3 4; do l1+="Or16s$s_:p-inner_layer_room Nr16s$s_:p-inner_layer_room "; done
    for s_ in 5 6 7 8 9; do l2+="Or16s$s_:p-inner_layer_room Nr16s$s_:p-inner_layer_room "; done
    u i 2 "$l1" "$l2"
    u q 6 "Nbr16:qmtech:2" "Ntr16:qmtech:2" "Nar16:qmtech:2"
    ( bash $ROOT/tools/tw5_job.sh final > $R/final-mode.log 2>&1 ) & sleep 1
    ( bash $ROOT/tools/tw5_job.sh d8 > $R/d8.log 2>&1 ) & sleep 1
    wait; note "all done" ;;
  r6)
    # Round 6: the esp copper-to-edge sweep (O and N, 1 cycle and N 16 cycles) and the merge trial (final).
    edge_boards 0.02 0.025 0.03 0.04 0.05
    lo=""; l1=""; l2=""; for x in 0.02 0.025 0.03 0.04 0.05; do lo+="Or1:e-$x "; l1+="Nr1:e-$x "; l2+="Nr16:e-$x "; done
    u x 3 "$lo" "$l1" "$l2"
    ( bash $ROOT/tools/tw5_job.sh final > $R/final-mode.log 2>&1 ) & sleep 1
    ( bash $ROOT/tools/tw5_job.sh d8 > $R/d8.log 2>&1 ) & sleep 1
    wait; note "all done" ;;
  ocheck)
    # O = main copper at 1 cycle on the tip (vs nd2b2-dcf4c0e), after the assign_n refactor and the main merge.
    u o 4 "Or1:esp" "Or1:eprom Or1:wiccon" "Or1:qmtech"
    wait; note "all done" ;;
  final)
    # Merge trial on the tip (main merged in): every engine test, clippy, the full pytest (CI=1).
    ( hzq run --pin --threads 6 --name tw5-final -- bash -c "cd $ROOT/engine && cargo test --release --no-fail-fast > $R/cargo-all.log 2>&1; echo rc \$? >> $R/cargo-all.log; cargo clippy --release --all-targets > $R/clippy.log 2>&1; echo rc \$? >> $R/clippy.log; cd $ROOT && CI=1 python -m pytest -q > $R/pytest.log 2>&1; echo rc \$? >> $R/pytest.log" > $R/final.log 2>&1; note "final unit rc $?" ) & sleep 1
    wait
    note "cargo: $(grep -E '^test result' $R/cargo-all.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}'), $(tail -1 $R/cargo-all.log); FAILED: $(grep -E '^test .* FAILED$' $R/cargo-all.log | awk '{print $2}' | tr '\n' ' ')"
    note "clippy: $(grep -cE '^(warning|error)' $R/clippy.log) warnings/errors, $(tail -1 $R/clippy.log)"
    note "pytest: $(tail -2 $R/pytest.log | tr '\n' ' ')" ;;
  r2)
    # Round 2: O vs N at 16 cycles on the ladder (esp 5 seeds), the patterns O vs N at 16 cycles, targeted tests.
    gen
    ( hzq run --threads 2 --name tw5-tests -- bash $ROOT/tools/tw5_job.sh tests > $R/tests.log 2>&1; note "tests unit rc $?" ) & sleep 1
    u q 4 "Or16:qmtech:2" "Nr16:qmtech:2"
    u e 5 "Or16:esp Or16s1:esp" "Nr16:esp Nr16s1:esp" "Or16s2:esp Or16s3:esp Or16s4:esp" "Nr16s2:esp Nr16s3:esp Nr16s4:esp" \
          "Or16:wiccon Nr16:wiccon Or16:eprom Nr16:eprom"
    pats=$(ls $R/pat); po=""; pn=""
    for n in $pats; do po+="Or16:p-$n "; pn+="Nr16:p-$n "; done
    u p 2 "$po" "$pn"
    wait; note "all done" ;;
  *) echo "usage: see header" ;;
esac
