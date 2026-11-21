#!/usr/bin/env bash
# first-draw-45 r11 mutants (TEMP, removed before the merge). usage: fd45r11_mut.sh OUT
set -u
OUT=$1; SRC=$(pwd)
T=/root/fd45r11-mut-tree; export CARGO_TARGET_DIR=/root/fd45r11-mut-target CARGO_BUILD_JOBS=8 RUST_TEST_THREADS=4
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
run B1_no_cross_price src/topo/octi.rs 'n=s.replace("viol_nm: self.octi.viol_nm, wire: ow","viol_nm: 0.0, wire: ow")' "--test tw2_metric a_band_that"
run B2_no_cross_in_dp src/topo/octi.rs 'n=s.replace("cfg.room * rpre.room(pitch, a, b) + rpre.cross(a, b)","cfg.room * rpre.room(pitch, a, b)")' "--test tw2_metric a_band_that"
run B3_no_room_in_band src/topo/octi.rs 'n=s.replace("bands.push(Corridor { path: b.path.clone(), reach,","bands.push(Corridor { path: b.path.clone(), reach: 0.0 * reach + reach,") if False else s.replace("    for c in cors {\n        if lb.max.x < c.bb.min.x","    for c in cors.iter().filter(|c| c.viol_nm == 0.0) {\n        if lb.max.x < c.bb.min.x")' "--test tw2_metric a_band_that"
run B4_cross_len_uncapped src/topo/octi.rs 'n=s.replace("let cl = (2.0 * c.reach / sin).min(l);","let cl = 2.0 * c.reach / sin;")' "--lib band_crossing"
echo done
