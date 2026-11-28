#!/usr/bin/env bash
# bundle-turn opt-in round (user 2026-11-28: [capacity] oblique_price default 0): one hzq unit, 4 threads.
#   det s0 C1 (runs=1 rounds=1) on qmtech-bh26, the old qm copy and esp at the new defaults, then rule 14c
#   (a default change: every release test binary once, clippy) and the pytest files of schema.py + docs_gen --check.
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $OUT/rc.txt; }
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
BH=scratch/boards/qmtech-bh26/qmtech-bh26.kicad_pcb; QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
b() { TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$1" "$2" --only 0 --no-png --threads 4 $C1 > "$OUT/$1.log" 2>&1 9>&-; note "$1 rc $?"; gzip -f "$OUT/$1.log"; }
b new-bh $BH; b new-qm $QM; b new-esp $ESP
rustup component add clippy >/dev/null 2>&1
(cd engine && CARGO_BUILD_JOBS=4 cargo clippy --release --all-targets > $OUT/clippy.log 2>&1 9>&-; note "clippy rc $? warnings $(grep -c '^warning' $OUT/clippy.log)")
(cd engine && CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 cargo test --release --no-fail-fast > $OUT/rel.log 2>&1 9>&-; note "rel rc $? binaries $(grep -c '^     Running' $OUT/rel.log) failed $(grep -c 'FAILED\|panicked' $OUT/rel.log)")
CI=1 python -m pytest -q tests/plugin/test_config.py tests/test_docs_links.py tests/test_engine_surface.py > $OUT/pytest.log 2>&1 9>&-; note "pytest rc $? $(tail -1 $OUT/pytest.log)"
(cd plugin && python -m topor.docs_gen --check > $OUT/docs_gen.log 2>&1 9>&-; note "docs_gen rc $?")
note "all done"
