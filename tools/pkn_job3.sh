#!/usr/bin/env bash
# pick-necks r2: rule 14c (default pick changed): every release test binary once + full tests/plugin pytest + docs_gen;
# det s0 esp (rounds 8) and qm (rounds 4), same settings as pkn2, to compare the pick with r1 and with main's order.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-pkn3}
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() { local a=$1 n=$2 p=$3; shift 3; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads ${TH:-4} "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
rel() {
  (cd engine; RUST_TEST_THREADS=4 cargo test --release --no-fail-fast > $OUT/rel.log 2>&1 9>&-; echo "rel rc $?" >> $OUT/rc.txt)
  (CI=1 python -m pytest -q tests/plugin > $OUT/pytest.log 2>&1 9>&-; echo "pytest rc $?" >> $OUT/rc.txt)
  (cd plugin; PYTHONPATH=. python -m topor.docs_gen --check > $OUT/docs_gen.log 2>&1 9>&-; echo "docs_gen rc $?" >> $OUT/rc.txt)
}
esp() { TH=2 unit r8 esp $ESP --set optimize.runs=1 --set optimize.rounds=8 --set capacity.compass_cut_check=true; }
qm() { unit r4 qm $QM --set optimize.runs=1 --set optimize.rounds=4 --set capacity.compass_cut_check=true; }
export -f unit rel esp qm; export OUT QM ESP
hzq run --threads 4 --name $TAG-rel --pin -- bash -c 'rel' 9>&- &
sleep 3
hzq run --threads 2 --name $TAG-esp --pin -- bash -c 'esp' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-qm --pin -- bash -c 'qm' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
