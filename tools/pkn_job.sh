#!/usr/bin/env bash
# pick-necks (bulk): rel = clippy + every release test binary once (rule 14c: default pick changes) + pytest + docs_gen;
# small = det s0 C1 tip eprom/wiccon/esp; qm = det s0 C1 tip qmtech. Base pick is derived offline from the same drawn rows.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-pkn1}
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
EP=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb; WI=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() { local a=$1 n=$2 p=$3; shift 3; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads ${TH:-4} "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
rel() {
  rustup component add clippy > /dev/null 2>&1
  (cd engine; CARGO_BUILD_JOBS=4 cargo clippy --release --all-targets > $OUT/clippy.log 2>&1 9>&-; echo "clippy rc $?" >> $OUT/rc.txt)
  (cd engine; RUST_TEST_THREADS=4 cargo test --release --no-fail-fast > $OUT/rel.log 2>&1 9>&-; echo "rel rc $?" >> $OUT/rc.txt)
  (CI=1 python -m pytest -q tests/plugin > $OUT/pytest.log 2>&1 9>&-; echo "pytest rc $?" >> $OUT/rc.txt)
  (cd plugin; PYTHONPATH=. python -m topor.docs_gen --check > $OUT/docs_gen.log 2>&1 9>&-; echo "docs_gen rc $?" >> $OUT/rc.txt)
}
small() { TH=2 unit tip ep $EP $C1 & TH=2 unit tip wi $WI $C1 & TH=2 unit tip esp $ESP $C1 & wait; }
qm() { unit tip qm $QM $C1; }
export -f unit rel small qm; export OUT C1 EP WI QM ESP
hzq run --threads 4 --name $TAG-rel --pin -- bash -c 'rel' 9>&- &
sleep 3
hzq run --threads 6 --name $TAG-small --pin -- bash -c 'small' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-qm --pin -- bash -c 'qm' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
