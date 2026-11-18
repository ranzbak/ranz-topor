#!/usr/bin/env bash
# exist-term r2 (fix round 2) tip job: targeted tests, the private badge with its copper kept, the round's mutants.
# hz.sh job --boards eprom-to-kernal,xt-badge. Long units first (badge, tests), mutants bundled (4 units).
set -u
R=${HZ_RUN_DIR:-/tmp/xt2}; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & sleep 1; }
u xt2-badge 4 bash tools/exist_term_badge.sh tip
u xt2-tests 4 bash tools/xt2_tests.sh
u xt2-m1 2 bash tools/xt2_mut.sh m1 dang_retain_off lead_margin_off lead_margin_core_off exit_more_off tidy_4a_off
u xt2-m2 2 bash tools/xt2_mut.sh m2 tidy_locked_off tidy_width_off tidy_touched_off tidy_collinear_off tidy_via_off
u xt2-m3 2 bash tools/xt2_mut.sh m3 tidy_pad_off tidy_fold_off tidy_gone_off tidy_once_off remove_fwd_off reason_off reason_side_off
u xt2-m4 1 bash tools/xt2_mut.sh m4 py_route_fwd_off py_loop_ret_off py_commit_off
wait; echo "all done" >> "$R/rc.txt"
