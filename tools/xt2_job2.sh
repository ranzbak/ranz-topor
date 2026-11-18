#!/usr/bin/env bash
# exist-term r2b: the lib tests after the from_three_points fix, and the round's mutants (the first job's mutant units ran
# against a failing base: the kept_polygons fuzz found a NaN arc). hz.sh job --boards eprom-to-kernal (the run-flow test).
set -u
R=${HZ_RUN_DIR:-/tmp/xt2}; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & sleep 1; }
u xt2b-lib 2 bash -c "cd engine && cargo test --release --lib > $R/lib.log 2>&1; echo \"lib rc \$?\" >> $R/lib.log; cargo test --release --test topo_exist_term >> $R/lib.log 2>&1; echo \"xt rc \$?\" >> $R/lib.log"
u xt2b-m1 2 bash tools/xt2_mut.sh m1 dang_retain_off lead_margin_off lead_margin_core_off exit_more_off tidy_4a_off
u xt2b-m2 2 bash tools/xt2_mut.sh m2 tidy_locked_off tidy_width_off tidy_touched_off tidy_collinear_off tidy_via_off
u xt2b-m3 2 bash tools/xt2_mut.sh m3 tidy_pad_off tidy_fold_off tidy_gone_off tidy_once_off remove_fwd_off reason_off reason_side_off three_points_off
u xt2b-m4 1 bash tools/xt2_mut.sh m4 py_route_fwd_off py_loop_ret_off py_commit_off
wait; echo "all done" >> "$R/rc.txt"
