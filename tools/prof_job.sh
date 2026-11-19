#!/usr/bin/env bash
# TW0: profile a BASE tree and a TIP tree side by side on ONE exclusive ccx43 (timing numbers only come from there).
#   hz.sh job --exclusive --wait --rev <tip branch> -- bash tools/prof_job.sh [--boards a,b] [--flame] [--args "--set k=v ..."]
# TIP = this checkout (engine, plugin, harness, tools all from it). BASE = the shipped tree prof-src/base (job branch: a full
# tree of the base rev with the hooks: engine plugin harness toporlib tools Cargo.*), driving its OWN engine, so each side is
# exactly its rev. Both: build a wheel (sccache), then the 1-thread units run in parallel, at most PROF_PAR (default 8 = half of a
# ccx43's 16 vCPUs), each board's base and tip adjacent in the queue so both see the same load:
#   tools/prof_run.sh <board> <out>/<tag>/<board> (seed 0, optimize.rounds=1 runs=1 unless --args overrides, --timeout 600 wall)
# --set-csv optimize.rounds=1000,optimize.stable_patience=1000: the long run (no cycle cap, ends at the 600 s limit). (hz.sh flattens
# quoting: no spaces inside one option value; --args "..." works only when run directly.)
# Overhead: eprom and esp also run once with TOPO_PROF off (tip) -> overhead.md. Finally tools/prof_compare.py -> compare.{md,json}.
# Output dir: $HZ_RUN_DIR (hz.sh) else scratch/prof-job. Progress lines in $OUT/rc.txt, "all done" at the end.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${HZ_RUN_DIR:-$ROOT/scratch/prof-job}; mkdir -p "$OUT"
BOARDS="eprom-to-kernal,wiccon-2023,esp-rust-board,qmtech,bornhack2026"; FLAME=""; XARGS=""; LABELS="base,tip"
while [ $# -gt 0 ]; do case $1 in --boards) BOARDS=$2; shift 2;; --flame) FLAME=--flame; shift;; --args) XARGS=$2; shift 2;; --set-csv) XARGS=$(echo "$2" | tr , "\n" | sed "s/^/--set /" | tr "\n" " "); shift 2;; --labels) LABELS=$2; shift 2;; *) shift;; esac; done
note() { echo "$*" >> "$OUT/rc.txt"; }
export RAYON_NUM_THREADS=${TOPO_PROF_THREADS:-1}
PATH=/root/.cargo/bin:$PATH
BASE_TREE=$ROOT/prof-src/base
tree_of() { [ "$1" = base ] && echo "$BASE_TREE" || echo "$ROOT"; }
build_tag() {  # build_tag <tag>
  local t site="$OUT/site-$1"; t=$(tree_of "$1")
  [ -f "$t/engine/Cargo.toml" ] || { note "no tree for $1"; return 1; }
  ( cd "$t" && CARGO_BUILD_JOBS=8 ${FLAME:+CARGO_PROFILE_RELEASE_DEBUG=true RUSTFLAGS="-C force-frame-pointers=yes"} \
      maturin build --release --manifest-path engine/Cargo.toml --features python --out "$site.wheels" > "$OUT/build-$1.log" 2>&1 9>&- \
    && pip install -q --no-deps --target "$site" "$site".wheels/topor_engine-*.whl ) || { note "BUILD FAIL $1"; return 1; }
  note "build done $1"
}
unit() {  # unit <tag> <board> [off]
  local t bp=$2; t=$(tree_of "$1")
  [ "$2" = qmtech ] && bp=$ROOT/scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb  # not a BENCH_BOARDS key
  [ -e "$t/scratch" ] || ln -sfn "$ROOT/scratch" "$t/scratch"  # boards for the shipped base tree
  local d=$1; [ "${3:-}" = off ] && d=$1-off
  ( cd "$t" && TOPO_PROF_OFF=$([ "${3:-}" = off ] && echo 1 || echo 0) TOPOR_ENGINE_SITE="$OUT/site-$1" tools/prof_run.sh "$bp" "$OUT/$d/$2" --timeout 600 $FLAME $XARGS ) > "$OUT/$d-$2.log" 2>&1
  note "$d $2 rc $?"
}
build_tag base || { note "all done"; exit 1; }
build_tag tip || { note "all done"; exit 1; }
Q=()
IFS=, read -ra BS <<< "$BOARDS"
for b in "${BS[@]}"; do
  Q+=("base $b" "tip $b")
  case $b in eprom-to-kernal|esp-rust-board) Q+=("tip $b off");; esac
done
PAR=${PROF_PAR:-8}
for u in "${Q[@]}"; do
  while [ "$(jobs -rp | wc -l)" -ge "$PAR" ]; do wait -n; done
  unit $u &
done
wait
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
python3 "$ROOT/tools/prof_compare.py" "$OUT/base" "$OUT/tip" --md "$OUT/compare.md" --json "$OUT/compare.json" --labels "$LABELS" > /dev/null 2>&1
note "all done"
