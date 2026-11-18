#!/usr/bin/env bash
# TW0: profile one or two revs side by side on ONE exclusive ccx43 (timing numbers only come from there).
#   hz.sh job --exclusive --wait --rev <tw0 branch tip> -- bash tools/prof_job.sh --base 27c36203 --tip <loop-local tip> [--boards a,b,...] [--flame]
# (--rev = the checkout whose plugin/harness/tools drive both engines; --tip default = that checkout's HEAD)
# Builds both revs first (sccache; frame pointers + debug info with --flame), then runs the 1-thread units in parallel, at most
# PROF_PAR (default 8 = half of a ccx43's 16 vCPUs) at once, each board's base and tip adjacent in the queue so both see the same load:
# tools/prof_run.sh <board> <out>/<tag>/<board> (seed 0, optimize.rounds=1 runs=1, RAYON_NUM_THREADS=1, --timeout 600
# wall, same options for both revs). Overhead: eprom and esp also run once with TOPO_PROF off (tip) -> overhead.md (router_s off vs on). Finally tools/prof_compare.py base vs tip -> <out>/compare.{md,json}.
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
build_rev() {  # build_rev <rev> <tag>
  local wt; wt=$(prep "$1" "$2") || return 1
  build "$wt" "$OUT/site-$2" || { note "BUILD FAIL $2"; return 1; }
  note "build done $2 ($1)"
}
unit() {  # unit <tag> <board> [off]
  local bp=$2; [ "$2" = qmtech ] && bp=$ROOT/scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb  # not a BENCH_BOARDS key
  local d=$1; [ "${3:-}" = off ] && d=$1-off
  # plugin, harness and tools of THIS checkout drive both engines; only the engine wheel differs
  ( cd "$ROOT" && TOPO_PROF_OFF=$([ "${3:-}" = off ] && echo 1 || echo 0) TOPOR_ENGINE_SITE="$OUT/site-$1" tools/prof_run.sh "$bp" "$OUT/$d/$2" --timeout 600 $FLAME ) > "$OUT/$d-$2.log" 2>&1
  note "$d $2 rc $?"
}
TIPREV=${TIP:-$(git -C "$ROOT" rev-parse HEAD)}
[ -n "$BASE" ] && { build_rev "$BASE" base || { note "all done"; exit 1; }; }
build_rev "$TIPREV" tip || { note "all done"; exit 1; }
# queue: pairs adjacent, the counters-off reference of eprom / esp next to its pair
Q=()
IFS=, read -ra BS <<< "$BOARDS"
for b in "${BS[@]}"; do
  [ -n "$BASE" ] && Q+=("base $b")
  Q+=("tip $b")
  case $b in eprom-to-kernal|esp-rust-board) Q+=("tip $b off");; esac
done
PAR=${PROF_PAR:-8}
for u in "${Q[@]}"; do
  while [ "$(jobs -rp | wc -l)" -ge "$PAR" ]; do wait -n; done
  unit $u &
done
wait
# overhead: router_s counters off vs on (same rev, same batch)
python3 - "$OUT" > "$OUT/overhead.md" <<'PY'
import glob, json, sys
o = sys.argv[1]
print("| board | router_s off | router_s on | overhead % |\n|---|---:|---:|---:|")
for b in ("eprom-to-kernal", "esp-rust-board"):
    v = []
    for d in ("tip-off", "tip"):
        f = glob.glob(f"{o}/{d}/{b}/run/*--metrics.json")
        v.append(json.load(open(f[0])).get("router_s") if f else None)
    if all(v) and v[0]:
        print(f"| {b} | {v[0]} | {v[1]} | {100 * (v[1] / v[0] - 1):+.1f} |")
PY
[ -n "$BASE" ] && python3 "$ROOT/tools/prof_compare.py" "$OUT/base" "$OUT/tip" --md "$OUT/compare.md" --json "$OUT/compare.json" --labels "base 27c36203 (pre-fp-clear),tip (pre-fp-clear)" > /dev/null 2>&1
note "all done"
