#!/usr/bin/env bash
# exist-term r3 mutants (Hetzner): the dangling_lead_via mutants of tools/xt2_mut.py in two hzq units.
set -u
hzq run --threads 2 --name xt3m-a -- bash tools/xt2_mut.sh a via_core_off via_skip_off via_layers_off 9>&- &
sleep 1
hzq run --threads 2 --name xt3m-b -- bash tools/xt2_mut.sh b via_hole_off via_fallback_off via_refine_off 9>&- &
wait
