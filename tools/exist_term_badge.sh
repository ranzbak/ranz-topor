#!/usr/bin/env bash
# exist-term: the private half-routed badge (scratch/boards/xt-badge, --boards xt-badge) through the plugin
# run flow with its copper kept (route_board --keep-copper), deterministic (optimize.rounds=1), KiCad DRC.
# Usage (on the VM, in the job tree): bash tools/exist_term_badge.sh <name>
set -u
R=${HZ_RUN_DIR:-/tmp/xt}
N=${1:-badge}
P=$(ls scratch/boards/xt-badge/*.kicad_pcb | head -1)
hzq run --threads 4 --name "$N" -- bash -c "tools/route_board.sh $P $R/$N --keep-copper --no-png --set optimize.rounds=1 > $R/$N.txt 2>&1; echo rc \$? >> $R/$N.txt"
