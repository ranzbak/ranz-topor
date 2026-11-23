# HANDOFF thick-layer (2026-11-23 ~03:00, round 3 done, partial)

- Worktree: scratch/agent-wt/wt-thick-layer.
- Branch: thick-layer, off main 65d7901b. Do not merge; the lead decides.
- Target dir: scratch/agent-wt/wt-thick-layer-runs/target.
- Reports: scratch/agent-wt/reports/thick-layer-20261123-0120 (round 1), -0215 (round 2) and -0300 (round 3), each as .md and .json.

## State (options; defaults = main copper, verified at 12e55fee)

- `[vias] site_planned` (false). `site_slack` (1, M3 crosser slack, refine.rs `shifted_lanes` / `neighbour_stop`). `site_lanes` (false).
- `[vias] priority_weight` ([0, 0]). `pair_one_side` (false). `pair_hold` (1, `State::layer_hold`, used in layers.rs `assign_two` and `assign_n`).
- `[search] free_crossing` (1, init.rs `arc_chords`, a connection with two drilled pads).

## Open

1. The drawing does not realise M3 slack. viaopt::relocate_vias bounds each via by the taut crossings and measures against taut tracks, so it marks sites tight and the planned site is missed. Ignored target: `thick_dive_grouping_predicts_what_it_draws_with_slack`.
2. esp trajectory flip. One extra kept pair move leads to 2 floating-up moves (+3V3), and then IO4–IO7 go into the U3/U4 fan-out. Any perturbation triggers it (site_planned, ω).
3. free_crossing 0 causes J1 through-pin link detours on esp. See the report for the 0.5 arm.
4. A mutant survives: the neighbour cut in neighbour_stop has no killing test.

## Commands

- Run the tests: `cd engine && CARGO_TARGET_DIR=../../wt-thick-layer-runs/target ../../slot.sh cargo test --release --test topo_thick_layer --test topo_reroute52 --test loop_local`
- A/B job scripts: scratch/boards/thick-layer/ab3_job.sh, ab4_job.sh and ab5_job.sh (the arms are explicit in each).
- Pattern PNGs: render locally with `PYTHONPATH=$PWD:$PWD/plugin python3 ../../boards/thick-layer/pat/png_wrap.py ../../boards/thick-layer/pat/<p>.py <runs root> <out> <p>`.
