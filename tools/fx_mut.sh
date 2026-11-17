#!/usr/bin/env bash
# fx_mut.sh <run dir>: patterns -> boards, then base + every survivor mutant (serial, nice), results in <run dir>/mut/
set -u
R=$1; T=$(pwd)
[ -z "${DISPLAY:-}" ] && exec xvfb-run -a bash "$0" "$@"
mkdir -p $R
python -m harness.fx_survivors $R/boards > $R/gen.log 2>&1
for m in base tip_side_alt tip_side_first tie_label vertex_leave_off feat_side corridor_no_push tpad_keep own_tip_keep end_skip_off erbs_x_any_layer h_exit_no_b h_edge_exit_whole h_inject_every_layer; do
  bash tools/fx_mut_one.sh $m $R $T
done
echo "all done" >> $R/mut/done.txt
