#!/usr/bin/env bash
# first-draw-45 r9 (TEMP job script).
#   prof:   esp det s0 sweep, TOPO_PROF + TOPO_TIMES + TOPO_REDUCEDBG (copper-neutral debug lines):
#           unit espT on the job's wheel (clean timing), unit espP under perf on a frame-pointer wheel.
#           perf.data, work dirs and full logs stay on the VM in /root/fd45r9-keep (not fetched);
#           $OUT gets metrics, copper, the s0 log (gz) and the perf text reports.
#   ladder: det s0 x wiccon/eprom/esp/qmtech at the job's rev (args: extra --set options), forensic
#           env (TOPO_FREEDBG, copper-neutral) on esp and qmtech.
set -u
OUT=$HZ_RUN_DIR
TREE="${TOPOR_TREE:-$(pwd)}"
KEEP=/root/fd45r9-keep/$HZ_TAG
mkdir -p "$KEEP"
cd "$TREE"
S="--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
WIC=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
EP=$(ls scratch/boards/eprom*/*/*.kicad_pcb scratch/boards/eprom*/*.kicad_pcb 2>/dev/null | head -1)
EPX="--set capacity.compass_cut_check=true"
note() { echo "$*" >> "$OUT/rc.txt"; }
unit() { # name board extra...  (copies the s0 log into $OUT gz, keeps work dirs on the VM)
  local n=$1 b=$2; shift 2
  (cd "$TREE" && python -m harness.score_runs "$OUT/$n" "$b" --only 0 --no-png --threads 4 $S "$@") > "$OUT/$n.log" 2>&1
  local rc=$?
  for f in "$OUT/$n"/*-s0.log "$OUT/$n"/*/s0.log; do [ -f "$f" ] && gzip -c "$f" > "$OUT/$n.s0log.gz"; done
  mkdir -p "$KEEP/$n" && mv "$OUT/$n"/*-s0.log "$KEEP/$n/" 2>/dev/null
  [ -d "$OUT/$n" ] && for w in "$OUT/$n"/*/s0/work; do [ -d "$w" ] && mv "$w" "$KEEP/$n/work"; done
  note "$n rc $rc"
}
export -f unit note; export OUT S QM ESP WIC EP EPX TREE KEEP
mode=${1:-prof}; shift || true
if [ "$mode" = prof ]; then
  export TOPO_PROF=1 TOPO_TIMES=1 TOPO_REDUCEDBG=1
  hzq run --threads 4 --name "$HZ_TAG-espT" -- bash -c 'unit espT $ESP --set drawing.octi_order=sweep' 9>&- &
  # perf + frame-pointer wheel
  ( (apt-get install -y -qq linux-tools-common "linux-tools-$(uname -r)" > "$OUT/apt.log" 2>&1 || true)
    echo -1 > /proc/sys/kernel/perf_event_paranoid 2>/dev/null || true
    command -v perf >/dev/null && perf --version >/dev/null 2>&1 || { note "no perf"; exit 0; }
    cd "$TREE" && CARGO_TARGET_DIR=$KEEP/target-fp CARGO_PROFILE_RELEASE_DEBUG=1 RUSTFLAGS="-C force-frame-pointers=yes" CARGO_BUILD_JOBS=8 \
      maturin build --release --manifest-path engine/Cargo.toml --features python --out "$KEEP/wheels-fp" > "$OUT/build-fp.log" 2>&1 9>&- \
      && pip install -q --no-deps --target "$KEEP/site-fp" "$KEEP"/wheels-fp/topor_engine-*.whl >> "$OUT/build-fp.log" 2>&1 || { note "fp build fail"; exit 0; }
    note "fp build done"
    hzq run --threads 4 --name "$HZ_TAG-espP" -- bash -c 'TOPOR_ENGINE_SITE=$KEEP/site-fp perf record -F 499 -g -o $KEEP/perf.data -- python -m harness.score_runs "$OUT/espP" "$ESP" --only 0 --no-png --threads 4 $S --set drawing.octi_order=sweep > "$OUT/espP.log" 2>&1; note "espP rc $?";
      for f in "$OUT/espP"/*-s0.log; do [ -f "$f" ] && gzip -c "$f" > "$OUT/espP.s0log.gz" && mv "$f" $KEEP/; done
      for w in "$OUT/espP"/*/s0/work; do [ -d "$w" ] && mv "$w" $KEEP/espP-work; done
      perf report -i $KEEP/perf.data --stdio --no-children --sort dso,symbol --percent-limit 0.2 -g none > "$OUT/perf-self.txt" 2>/dev/null
      perf report -i $KEEP/perf.data --stdio --children --sort symbol --percent-limit 0.5 -g none > "$OUT/perf-incl.txt" 2>/dev/null
      perf report -i $KEEP/perf.data --stdio --no-children --sort symbol --percent-limit 3 -G > "$OUT/perf-callers.txt" 2>/dev/null
      note "perf reports done"' 9>&- ) &
  wait
  note "all done"; exit 0
fi
if [ "$mode" = ladder ]; then
  X="$*"
  q() { while [ "$(jobs -rp | wc -l)" -ge 4 ]; do wait -n; done; hzq run --threads 4 --name "$HZ_TAG-$1" -- bash -c "$2" 9>&- & }
  q qm "TOPO_FREEDBG=1 unit qm \$QM $X"; q esp "TOPO_FREEDBG=1 unit esp \$ESP $X"
  q wic "unit wic \$WIC $X"; q ep "unit ep \$EP \$EPX $X"
  wait
  note "all done"; exit 0
fi
