#!/usr/bin/env bash
# fx_mut_one.sh <mutant|base> <run dir> <tree>: build the engine with one mutant (tools/fx_mutate.py) from <tree>, route the 6 proposed
# patterns with realise = erbs, write "pattern copper <hash> dump <hash>" lines to <run dir>/mut/<mutant>.txt.
set -u
M=$1; R=$2; T=$3
if [ -z "${DISPLAY:-}" ]; then exec xvfb-run -a bash "$0" "$@"; fi
mkdir -p $R/mut /root/fxm /root/fxm-wheels /root/fxm-site
W=/root/fxm/$M
rm -rf $W; mkdir -p $W
rsync -a --exclude target --exclude .git --exclude scratch --exclude .venv $T/ $W/
[ $M = base ] || python3 $W/tools/fx_mutate.py $M apply $W > $R/mut/$M.build.log 2>&1 || { echo "mutate failed" >> $R/mut/$M.txt; exit 1; }
rm -rf /root/fxm-wheels/$M /root/fxm-site/$M
( cd $W/engine && CARGO_TARGET_DIR=/root/fxm-target CARGO_BUILD_JOBS=4 nice -n 5 maturin build --release --manifest-path Cargo.toml --features python --out /root/fxm-wheels/$M >> $R/mut/$M.build.log 2>&1 ) || { echo "build failed" >> $R/mut/$M.txt; exit 1; }
pip install -q --no-deps --target /root/fxm-site/$M /root/fxm-wheels/$M/*.whl >> $R/mut/$M.build.log 2>&1
cd $W
for n in edge_tangent collinear_row diag_exit via_fold qfn_two_ends edge_pads; do
  o=$R/mut/$M-$n
  rm -rf $o; mkdir -p $o/dump
  TOPO_ERBS_DUMP=$o/dump TOPOR_ENGINE_SITE=/root/fxm-site/$M tools/route_board.sh $R/boards/$n/problem/$n.kicad_pcb $o/run --no-png --timeout 120 --set optimize.rounds=1 --set drawing.realise=erbs > $o/out.txt 2> $o/err.txt
  pcb=$(ls $o/run/*-topor/*.kicad_pcb 2>/dev/null | head -1)
  echo "$n $(python3 $W/tools/fx_hash.py $pcb $o/dump 2>/dev/null || echo nohash)" >> $R/mut/$M.txt
  rm -rf $o/run
done
echo "$M done" >> $R/mut/done.txt
