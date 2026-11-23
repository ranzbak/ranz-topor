#!/usr/bin/env bash
# dvn_job.sh (branch draw-via-neck, docs/dev/design/draw-via-neck.md): PROPOSED patterns on the VM.
#   dvn_job.sh pats   boards (KiCad DRC of the references), each pattern routed through the plugin flow (route_board.sh,
#                     product defaults, runs=1, seed 0, TOPO_VIADBG + TOPO_NECKDBG + TOPO_ERBS_DEBUG), PNGs + summary.json
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
NAMES="via_jam_shift neck_in_pad_row neck_shared_channel pad_axis_entry"
case ${1:-} in
  pats)
    python -m harness.dvn_patterns $R/dvnp/boards --drc > $R/dvnp-gen.log 2>&1; note "gen rc $?"
    for n in $NAMES; do
      TOPOR_UNRELEASED_SETTINGS=1 TOPO_VIADBG=1 TOPO_NECKDBG=1 TOPO_ERBS_DEBUG=1 tools/route_board.sh $R/dvnp/boards/$n/problem/$n.kicad_pcb \
        $R/dvnp/runs/D/$n/$n/s0 --set optimize.runs=1 > $R/dvnp/D-$n.txt 2>&1 9>&-
      note "pat D $n rc $?"
    done
    python -m harness.dvn_patterns_png $R/dvnp/runs $R/dvnp/png > $R/dvnp-png.log 2>&1; note "png rc $?"
    note "all done" ;;
  *) echo "usage: $0 pats"; exit 2 ;;
esac
