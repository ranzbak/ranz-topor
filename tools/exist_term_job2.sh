#!/usr/bin/env bash
# exist-term round 3 (Hetzner): the mutants again (clean reverts), the private badge tip, the CI engine suite.
set -u
export XT_MUT_A="dang_retain_off turn_cost_off lead_draw_off lead_off" XT_MUT_B="kept_end_not_free dead_ent_off planned_end_off"
R=${HZ_RUN_DIR:-/tmp/xt}; mkdir -p "$R"
hzq run --threads 4 --name xt-mutA -- bash tools/exist_term_mut.sh A $XT_MUT_A &
sleep 1
hzq run --threads 4 --name xt-mutB -- bash tools/exist_term_mut.sh B $XT_MUT_B &
sleep 1
bash tools/exist_term_badge.sh tip &
sleep 1
bash tools/exist_term_suite.sh cargo &
wait
echo "all done" > "$R/rc.txt"
