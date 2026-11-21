#!/usr/bin/env bash
# first-draw-45 r10 (TEMP job script, removed from the branch before the merge): ladder = 4 boards det s0; suite = release cargo suite --no-fail-fast + pytest; mut = mutant bundle.
# mode base: 4 boards det s0. Keeps only json, the routed .kicad_pcb (gz), result.json and logs (gz) in $OUT.
set -u
OUT=$HZ_RUN_DIR
TREE="${TOPOR_TREE:-$(pwd)}"
BULK=/root/vfd45-bulk/$HZ_TAG
mkdir -p "$BULK"
cd "$TREE"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
EP=$(ls scratch/boards/eprom*/*/*.kicad_pcb scratch/boards/eprom*/*.kicad_pcb 2>/dev/null | head -1)
EPX="--set capacity.compass_cut_check=true"
note() { echo "$*" >> "$OUT/rc.txt"; }
unit() { # name board extra...
  local n=$1 b=$2; shift 2; local t0=$(date +%s)
  (cd "$TREE" && python -m harness.score_runs "$OUT/$n" "$b" --only 0 --no-png --threads 4 $S "$@") > "$OUT/$n.log" 2>&1
  local rc=$?
  for d in "$OUT/$n"/*/s0; do
    [ -d "$d" ] || continue
    for f in "$d"/*-topor/*.kicad_pcb; do [ -f "$f" ] && gzip -k "$f"; done
    for r in "$d"/reports/*/*/; do [ -f "$r/log.txt" ] && gzip "$r/log.txt"; mkdir -p "$BULK/$n"; for x in "$r"/*; do case "$x" in *result.json|*log.txt.gz|*drc.json) ;; *) mv "$x" "$BULK/$n/" 2>/dev/null;; esac; done; done
    for x in "$d"/work "$d"/*-input "$d"/*-hand "$d"/*-topor/*.kicad_pcb "$d"/*-s0.log; do [ -e "$x" ] && mv "$x" "$BULK/$n/$(basename "$x").$RANDOM"; done
  done
  for f in "$OUT/$n"/*-s0.log; do [ -f "$f" ] && gzip "$f"; done
  note "$n rc $rc $(( $(date +%s)-t0 )) s"
}
export -f unit note; export OUT S QM ESP WIC EP EPX TREE BULK
mode=${1:-ladder}
q() { while [ "$(jobs -rp | wc -l)" -ge "${QMAX:-4}" ]; do wait -n; done; hzq run --threads "${3:-4}" --name "$HZ_TAG-$1" -- bash -c "$2" 9>&- & }
if [ "$mode" = ladder ]; then
  q qm 'unit qm $QM'; q esp 'unit esp $ESP'; q wic 'unit wic $WIC'; q ep 'unit ep $EP $EPX'
  wait; note "all done"; exit 0
fi
if [ "$mode" = suite ]; then
  q rel 'cd $TREE && s=$(date +%s); CARGO_TARGET_DIR=/root/fd45r10-rel-target CARGO_BUILD_JOBS=8 RUST_TEST_THREADS=8 cargo test --release --workspace --no-fail-fast > $OUT/cargo-rel.log 2>&1 9>&-; note "rel rc $? $(( $(date +%s)-s )) s"; gzip -k $OUT/cargo-rel.log' 8
  q py 'cd $TREE && s=$(date +%s); CI=1 python -m pytest -q -p no:cacheprovider -rfEs > $OUT/pytest.log 2>&1; note "py rc $? $(( $(date +%s)-s )) s"' 4
  q mutA 'cd $TREE && bash tools/fd45r10_mut.sh $OUT A > $OUT/mutA.log 2>&1 9>&-; note "mutA rc $?"' 6
  q mutB 'cd $TREE && bash tools/fd45r10_mut.sh $OUT B > $OUT/mutB.log 2>&1 9>&-; note "mutB rc $?"' 6
  wait; note "all done"; exit 0
fi
