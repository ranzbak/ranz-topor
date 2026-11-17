#!/usr/bin/env bash
# fx_mut_tests.sh <run dir> <tree>: the topo_fx_survivors tests against the unmutated engine and every survivor mutant
# (killed = the test binary fails). One tree copy, the mutated source restored after each.
set -u
R=$1; T=$2; W=/root/fxt
mkdir -p $R/mt
rm -rf $W; mkdir -p $W; rsync -a --exclude target --exclude .git --exclude scratch --exclude .venv $T/ $W/
cd $W/engine
export CARGO_TARGET_DIR=/root/fxt-target CARGO_BUILD_JOBS=4
run() { cargo test --release --test topo_fx_survivors > $R/mt/$1.log 2>&1; echo "$1 rc $?" >> $R/mt/rc.txt; }
run base
cp -a src/topo $R/mt/orig-topo
for m in tip_side_alt tip_side_first tie_label vertex_leave_off feat_side corridor_no_push tpad_keep own_tip_keep end_skip_off erbs_x_any_layer h_exit_no_b h_edge_exit_whole h_inject_every_layer; do
  python3 $W/tools/fx_mutate.py $m apply $W > $R/mt/$m.apply 2>&1
  run $m
  rsync -a --delete $R/mt/orig-topo/ src/topo/
done
echo "mutants done" >> $R/mt/rc.txt
