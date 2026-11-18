#!/usr/bin/env bash
# exist-term r3 diagnosis (Hetzner): the private badge as in tools/exist_term_badge.sh, with the kept-end lead dump
# (TOPO_KEPTDBG), the forced-wire dump (TOPO_FORCEDBG) and the plan handed to the drawing (TOPO_PLANDUMP).
set -u
R=${HZ_RUN_DIR:-/tmp/xt3}; mkdir -p "$R"
P=$(ls scratch/boards/xt-badge/*.kicad_pcb | head -1)
hzq run --threads 4 --name xt3-dbg -- bash -c "TOPO_KEPTDBG=1 TOPO_FORCEDBG=1 TOPO_PLANDUMP=$R/plan.json tools/route_board.sh $P $R/dbg --keep-copper --no-png --set optimize.rounds=1 > $R/dbg.txt 2>&1; echo rc \$? >> $R/dbg.txt"
grep -h "KEPTLEAD" -r "$R"/dbg* 2>/dev/null | sort | uniq -c | sort -rn | head -400 > "$R/keptlead.txt"
echo done >> "$R/dbg.txt"
