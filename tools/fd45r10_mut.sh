#!/usr/bin/env bash
# first-draw-45 r10 mutants (TEMP, removed from the branch before the merge). usage: fd45r10_mut.sh OUT [A|B]
set -u
OUT=$1; PART=${2:-A}; SRC=$(pwd)
T=/root/fd45r10-mut-tree-$PART; export CARGO_TARGET_DIR=/root/fd45r10-mut-target-$PART CARGO_BUILD_JOBS=8 RUST_TEST_THREADS=4
mkdir -p $T
run() { # name file python-replace test-args
  rsync -a --delete --exclude target "$SRC/" "$T/" && find $T/engine/src $T/engine/tests -name '*.rs' -exec touch {} +
  python3 - "$T/engine/$2" "$3" <<'PY'
import sys
p,code=sys.argv[1],sys.argv[2]
s=open(p).read(); n=s
exec(code)
assert n!=s, "mutant did not apply"
open(p,'w').write(n)
PY
  [ $? -ne 0 ] && { echo "$1 NOT APPLIED" | tee -a $OUT/mut.tsv; return; }
  (cd $T/engine && cargo test --release $4 > $OUT/mut-$1.log 2>&1 9>&-); rc=$?
  echo "$1	rc $rc	$(grep -c "panicked" $OUT/mut-$1.log) panics	$( [ $rc -ne 0 ] && echo killed || echo SURVIVED)" | tee -a $OUT/mut.tsv
}
if [ "$PART" = A ]; then
run MF1_forced_style src/topo/draw.rs 'n=s.replace("(forced.is_none() && style(&[a, c]) >= style(&[a, b, c]))","(style(&[a, c]) >= style(&[a, b, c]))")' "--lib forced_pull"
run MF2_pays_true src/topo/draw.rs 'n=s.replace("let pays = !(octi_corner && p5) ||","let pays = true || !(octi_corner && p5) ||")' "--lib forced_pull"
run W1_no_weight src/topo/erbs.rs 'n=s.replace("let w = weight.get(&(a, b)).copied().unwrap_or(1.0);","let w = 1.0;")' "--lib sweep_weights"
run W2_no_abs src/topo/erbs.rs 'n=s.replace("        v[2] += (f * e.y).abs();\n","")' "--lib sweep_weights"
run LF1_no_loserfirst src/topo/core.rs 'n=s.replace("if !super::octi::fd45_off(\"loserfirst\") {","if false {")' "--test tw2_metric a_band"
run LC1_no_corridor_price src/topo/octi.rs 'n=s.replace("    if l < 1.0 || cors.is_empty() {\n        return 0.0;","    if true {\n        return 0.0;")' "--lib loser_corridor"
run P2W_no_erbs_walls src/topo/octi.rs 'n=s.replace("                    if !crosses {\n                        walls.extend(","                    if false {\n                        walls.extend(")' "--test tw2_metric a_band"
else
run CH1_no_chord_filter src/topo/octi.rs 'n=s.replace("if own_pad.iter().flatten().any(|&(poly, c, ext)| stub_price(poly, c, ext, wi.w, q, 1.0) > 0.0) {","if false {")' "--test topo_pad_snap"
run PL1_no_pad_leave src/topo/octi.rs 'n=s.replace("-> bool { pads[end].as_ref()","-> bool { false && pads[end].as_ref()")' "--test topo_pad_snap"
run SF1_no_stub_floor src/topo/octi.rs 'n=s.replace("(4.0 * self.bend_nm).max(pitch).min(STUB_VIOL_SHARE","(4.0 * self.bend_nm).min(STUB_VIOL_SHARE")' "--test topo_pad_snap"
run BT1_no_bend_tie src/topo/octi.rs 'n=s.replace("let mut p = cfg.bend_nm.max(BEND_TIE_NM) * t / 45.0;","let mut p = cfg.bend_nm * t / 45.0;")' "--lib staircase_costs"
run JF1_no_jog_floor src/topo/octi.rs 'n=s.replace("p += 2.0 * cfg.bend_nm.max(BEND_TIE_NM) * (1.0 - l_in / leff).powi(2);","p += 2.0 * cfg.bend_nm * (1.0 - l_in / leff).powi(2);")' "--lib staircase_costs"
run M3P_probe src/topo/core.rs 'n=s.replace("erbs draw order {}","erbs order {}")' "--test topo_m3_class_keep"
fi
echo "done $PART"
