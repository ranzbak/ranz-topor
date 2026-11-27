#!/usr/bin/env bash
# draw-absorb A3 r3 (walls sided against the present ERBS): det s0 C1 a3 on qmtech + esp (+ wiccon/eprom hash), off qm
# (main hash); A3 mutants on r3; clippy (all targets) and the targeted pytest of the schema / docs change.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-dab8}
C1="--set optimize.runs=1 --set optimize.rounds=1"
A3="--set drawing.band_passes=3"
export TOPOR_UNRELEASED_SETTINGS=1
unit() { local a=$1 n=$2 p=$3; shift 3; python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads 4 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
mut() { local k=$1; shift; python3 tools/draw_absorb_mut2.py $OUT $k "$@" > $OUT/mut-$k.log 2>&1 9>&-; echo "mut-$k rc $?" >> "$OUT/rc.txt"; }
chk() { (cd engine && CARGO_BUILD_JOBS=4 cargo clippy --release --all-targets > $OUT/clippy.log 2>&1; echo "clippy rc $?" >> $OUT/rc.txt); CI=1 python -m pytest -q tests/plugin/test_config.py tests/test_docs_links.py tests/test_engine_surface.py > $OUT/pytest.log 2>&1; echo "pytest rc $?" >> $OUT/rc.txt; (cd plugin && python -m topor.docs_gen --check > $OUT/docs_gen.log 2>&1; echo "docs_gen rc $?" >> $OUT/rc.txt); }
EP=$(ls scratch/boards/eprom-to-kernal/*/*.kicad_pcb | head -1)
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb; WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
export -f unit mut chk; export OUT C1 A3 EP QM ESP WIC
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
hzq run --threads 4 --name $TAG-m3 --pin -- bash -c 'CARGO_BUILD_JOBS=4 mut 3 a3_off a3_no_through_guard a3_side_flip a3_earlier_yields a3_no_swap a3_no_viol_price' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-chk --pin -- bash -c 'chk' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-qm -- bash -c 'TOPO_A3DBG=1 unit a3 qm $QM $C1 $A3' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-small -- bash -c 'unit off qm $QM $C1; for n in esp wic ep; do case $n in esp) P=$ESP;; wic) P=$WIC;; ep) P=$EP;; esac; unit a3 $n $P $C1 $A3; done' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
