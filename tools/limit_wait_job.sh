#!/usr/bin/env bash
# limit-wait VM job: tests, debug suite, mutants, bornhack s0 before (grace unbounded) / after (default 30 s).
set -u
chmod +x tools/limit_wait_*.sh
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
B="--clock cpu --timeout 2400 --no-png --set optimize.rounds=1 --set optimize.runs=1 --set run.timeout_overrun_s=600"
u bh-before 4 tools/route_board.sh bornhack2026 "$R/bh-before" $B --set run.limit_grace_s=-1
u bh-after 4 tools/route_board.sh bornhack2026 "$R/bh-after" $B
u cargo 4 tools/limit_wait_cargo.sh
u pytest 4 python -m pytest tests -q -p no:cacheprovider
u debug 4 tools/limit_wait_debug.sh
for m in inner_no_grace scope_ignored grace_ignored no_set_grace partial_offered no_loop_end no_hit default_off no_extra_stop; do
  u mut-$m 3 tools/limit_wait_mutant.sh $m
done
wait
echo "all done" >> "$R/rc.txt"
