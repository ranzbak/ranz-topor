#!/usr/bin/env bash
# exist-term work-round job (Hetzner): targeted release tests, the round's mutants (tools/exist_term_mut.sh, two units;
# XT_MUT_A / XT_MUT_B override the lists), and the private half-routed badge (scratch/boards/xt-badge, --boards xt-badge)
# through the plugin run flow with its copper kept (tools/exist_term_badge.sh; XT_BADGE=0 skips it).
set -u
R=${HZ_RUN_DIR:-/tmp/xt}
mkdir -p "$R"
A=${XT_MUT_A:-dang_retain_off pre_join_off turn_cost_off lead_draw_off lead_off kept_end_not_free}
B=${XT_MUT_B:-pad_map_off via_map_off kept_tracks_off dead_ent_off planned_end_off}
hzq run --threads 4 --name xt-tests -- bash -c "cd engine && { cargo test --release --test topo_exist_term && cargo test --release --lib exist_end_maps; } > $R/tests.log 2>&1; echo rc \$? >> $R/tests.log" &
sleep 1
hzq run --threads 4 --name xt-mutA -- bash tools/exist_term_mut.sh A $A &
sleep 1
hzq run --threads 4 --name xt-mutB -- bash tools/exist_term_mut.sh B $B &
if [ "${XT_BADGE:-1}" = 1 ] && ls scratch/boards/xt-badge/*.kicad_pcb > /dev/null 2>&1; then
  sleep 1
  bash tools/exist_term_badge.sh tip &
fi
wait
echo "all done" > "$R/rc.txt"
