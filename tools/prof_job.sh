#!/usr/bin/env bash
# TW0: profile one or two revs side by side on ONE exclusive ccx43 (timing numbers only come from there).
#   hz.sh job --exclusive --wait --rev <tw0 branch tip> -- bash tools/prof_job.sh --base 27c36203 --tip <loop-local tip> [--boards a,b,...] [--flame]
# (--rev = the checkout whose plugin/harness/tools drive both engines; --tip default = that checkout's HEAD)
# For each rev (base optional): build a wheel (sccache, frame pointers + debug info with --flame), then, one board at a time,
# tools/prof_run.sh <board> <out>/<tag>/<board> (seed 0, optimize.rounds=1 runs=1, RAYON_NUM_THREADS=1, --timeout 600
# wall, no extra options: same options for both revs); finally tools/prof_compare.py base vs tip -> <out>/compare.{md,json}.
# A rev that does not carry the hooks (tools/prof-hooks.patch) gets the patch applied in a throw-away worktree first.
# Output dir: $HZ_RUN_DIR (hz.sh) else scratch/prof-job. Progress lines in $OUT/rc.txt, "all done" at the end.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${HZ_RUN_DIR:-$ROOT/scratch/prof-job}; mkdir -p "$OUT"
BASE=""; TIP=""; BOARDS="eprom-to-kernal,wiccon-2023,esp-rust-board,qmtech,bornhack2026"; FLAME=""
while [ $# -gt 0 ]; do case $1 in --base) BASE=$2; shift 2;; --tip) TIP=$2; shift 2;; --boards) BOARDS=$2; shift 2;; --flame) FLAME=--flame; shift;; *) shift;; esac; done
note() { echo "$*" >> "$OUT/rc.txt"; }
export TOPO_PROF_THREADS=${TOPO_PROF_THREADS:-1} RAYON_NUM_THREADS=${TOPO_PROF_THREADS:-1}
PATH=/root/.cargo/bin:$PATH
build() {  # build <srcdir> <site>
  ( cd "$1" && CARGO_BUILD_JOBS=8 ${FLAME:+CARGO_PROFILE_RELEASE_DEBUG=true RUSTFLAGS="-C force-frame-pointers=yes"} \
      maturin build --release --manifest-path engine/Cargo.toml --features python --out "$2.wheels" > "$OUT/build-$(basename "$2").log" 2>&1 9>&- \
    && pip install -q --no-deps --target "$2" "$2".wheels/topor_engine-*.whl )
}
prep() {  # prep <rev> <tag>: a worktree of <rev> with the hooks (patch applied when the rev lacks them)
  local wt="$OUT/src-$2"; git -C "$ROOT" worktree add --detach "$wt" "$1" > /dev/null 2>&1 || { note "worktree $1 failed"; return 1; }
  if ! grep -q "TOPO_PROF" "$wt/engine/src/topo/prof.rs" 2>/dev/null; then
    python3 "$ROOT/tools/prof_hooks_apply.py" "$wt" "$ROOT/tools/prof-hooks.patch" > "$OUT/patch-$2.log" 2>&1 || { note "hooks patch does not apply on $1"; return 1; }
  fi
  echo "$wt"
}
run_rev() {  # run_rev <rev> <tag>
  local wt; wt=$(prep "$1" "$2") || return 1
  build "$wt" "$OUT/site-$2" || { note "BUILD FAIL $2"; return 1; }
  note "build done $2 ($1)"
  IFS=, read -ra BS <<< "$BOARDS"
  for b in "${BS[@]}"; do
    local bp=$b; [ "$b" = qmtech ] && bp=$ROOT/scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb  # not a BENCH_BOARDS key
    # plugin, harness and tools of THIS checkout drive both engines; only the engine wheel differs
    ( cd "$ROOT" && TOPOR_ENGINE_SITE="$OUT/site-$2" tools/prof_run.sh "$bp" "$OUT/$2/$b" --timeout 600 $FLAME ) > "$OUT/$2-$b.log" 2>&1
    note "$2 $b rc $?"
  done
}
[ -n "$BASE" ] && run_rev "$BASE" base
run_rev "${TIP:-$(git -C "$ROOT" rev-parse HEAD)}" tip
[ -n "$BASE" ] && python3 "$ROOT/tools/prof_compare.py" "$OUT/base" "$OUT/tip" --md "$OUT/compare.md" --json "$OUT/compare.json" > /dev/null 2>&1
note "all done"
