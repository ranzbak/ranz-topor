#!/usr/bin/env bash
# limit-wait r3d: rerun of the mutants the strengthened last-step test / hardened runner touch (one hzq unit)
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
( hzq run --threads 4 --name mutD -- python3 tools/limit_wait_r3_mut.py "$R" D s_post v_last p_run p_postrun p_none p_cause scope chk1 s_pick v_cancel d_hard > "$R/mutD.log" 2>&1; echo "mutD rc $?" >> "$R/rc.txt" )
echo "all done" >> "$R/rc.txt"
