#!/usr/bin/env bash
# limit-wait r3: 4 hzq units (rel tests + pytest, 2 mutant bundles, phase sweep + no-limit boards)
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
u rel 4 bash tools/limit_wait_r3_rel.sh "$R"
u mutA 4 python3 tools/limit_wait_r3_mut.py "$R" A scope chk1 chk2 chk3 due loopmsg latch v_within v_last v_idle v_cancel s_change s_every s_before
u mutB 4 python3 tools/limit_wait_r3_mut.py "$R" B s_inherit s_cycle s_pick s_cutwait d_reduce d_start d_erbs d_text d_hard s_post p_run p_postrun p_none p_cause
u sweep 4 bash tools/limit_wait_r3_sweep.sh "$R"
wait
echo "all done" >> "$R/rc.txt"
