#!/usr/bin/env bash
# exist-term final tip check (Hetzner): targeted checks and the private badge tip, in parallel.
set -u
R=${HZ_RUN_DIR:-/tmp/xt}; mkdir -p "$R"
bash tools/exist_term_tests.sh &
sleep 1
bash tools/exist_term_badge.sh tip &
wait
echo "all done" > "$R/rc.txt"
