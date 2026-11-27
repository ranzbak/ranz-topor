#!/usr/bin/env bash
# draw-absorb r2 final tip (bulk, ccx43): full suites once (the r1 verifier's were cut off) + r2 mutants.
# U1 debug cargo suite (2 buckets in parallel) + doc (8 thr); U2 clippy + release suite + full pytest CI=1 +
# docs_gen --check (4 thr); U3 r2 mutants (tools/draw_absorb_r2_mut.py, 4 thr).
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab16}
dbg() {
  ls engine/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | sort > $OUT/bins.txt
  split -n l/2 -d $OUT/bins.txt $OUT/bucket.
  (cd engine; CARGO_TARGET_DIR=$OUT/tgt-dbg cargo test --no-run --lib --tests 9>&- > $OUT/dbg-build.log 2>&1)
  for b in 00 01; do
    ( args=""; for t in $(cat $OUT/bucket.$b); do args="$args --test $t"; done
      [ $b = 00 ] && args="--lib $args"
      cd engine; RUST_TEST_THREADS=4 CARGO_TARGET_DIR=$OUT/tgt-dbg cargo test $args --no-fail-fast 9>&- > $OUT/dbg-$b.log 2>&1; echo "dbg-$b rc $?" >> $OUT/rc.txt ) &
  done; wait
  (cd engine; CARGO_TARGET_DIR=$OUT/tgt-dbg cargo test --doc 9>&- > $OUT/dbg-doc.log 2>&1; echo "dbg-doc rc $?" >> $OUT/rc.txt)
}
rel() {
  rustup component add clippy > /dev/null 2>&1
  (cd engine; CARGO_BUILD_JOBS=4 cargo clippy --release --all-targets > $OUT/clippy.log 2>&1 9>&-; echo "clippy rc $?" >> $OUT/rc.txt)
  (cd engine; RUST_TEST_THREADS=4 cargo test --release --no-fail-fast > $OUT/rel.log 2>&1 9>&-; echo "rel rc $?" >> $OUT/rc.txt)
  CI=1 python -m pytest -q > $OUT/pytest.log 2>&1 9>&-; echo "pytest rc $?" >> $OUT/rc.txt
  (PYTHONPATH=plugin python -m topor.docs_gen --check > $OUT/docs_gen.log 2>&1; echo "docs_gen rc $?" >> $OUT/rc.txt)
}
mut() { CARGO_BUILD_JOBS=4 python3 tools/draw_absorb_r2_mut.py $OUT/mutw > $OUT/mut-r2.log 2>&1 9>&-; echo "mut-r2 rc $?" >> $OUT/rc.txt; cp $OUT/mutw/mut-r2.json $OUT/ 2>/dev/null; }
export -f dbg rel mut; export OUT
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 8 --name $TAG-dbg --pin -- bash -c 'dbg' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-rel --pin -- bash -c 'rel' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-mut --pin -- bash -c 'mut' 9>&- &
wait
rm -rf $OUT/tgt-dbg $OUT/mutw/tree-* $OUT/mutw/target-*
echo "all done" >> $OUT/rc.txt
