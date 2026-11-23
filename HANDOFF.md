# HANDOFF thick-layer (2026-11-23 ~04:00, round 4 done)

- Worktree: scratch/agent-wt/wt-thick-layer.
- Branch: thick-layer, off main 65d7901b. Do not merge; the lead decides.
- Target dir: scratch/agent-wt/wt-thick-layer-runs/target.
- Reports: scratch/agent-wt/reports/thick-layer-20261123-0120 (round 1), -0215 (round 2), -0300 (round 3) and -0400 (round 4), each as .md and .json.

## State (options; defaults = main copper, verified at 12e55fee)

- `[vias] site_planned` (false). `site_slack` (1, M3 crosser slack, refine.rs `shifted_lanes_of` / `neighbour_stop_of`, shared by the planner and viaopt::relocate_vias since r4). `site_lanes` (false). `site_window` (farthest | taut, r4).
- Off copper at the r4 tip = main (tl9b hash on esp and qmtech).
- `[vias] priority_weight` ([0, 0]). `pair_one_side` (false). `pair_hold` (1, `State::layer_hold`, used in layers.rs `assign_two` and `assign_n`).
- `[search] free_crossing` (1, init.rs `arc_chords`, a connection with two drilled pads).

## Open

1. Real boards: planned slack sites are not realised in dense fan-outs (esp U3/U4 IO4-IO8, qmtech U6 JS2). The fixture is clean. Every site_planned arm adds DRC (s1s esp +19 / qm +26, s1st +41 / +30). Next: an RCA of one U6 via (JS2_5, moved 2.3 mm) with the VIACLAMP debug line plus an ERBS dump.
2. Ignored target `thick_dive_grouping_via_past_taut_crossing`: ERBS takes a via's side of a crosser from the geometry and never bends a crosser past a via. This needs a side-from-plan for ERBS (design note first).
3. The esp "flip" is no keep bug (design addendum RCA r4): arms part before the loop.
4. free_crossing 0.5 is the user's call.
5. Surviving mutants (equivalent on the fixture): m3 relocation window taut, and site_window s2/s3 alt lanes on in taut mode.

## Commands

- Run the tests: `cd engine && CARGO_TARGET_DIR=../../wt-thick-layer-runs/target ../../slot.sh cargo test --release --test topo_thick_layer --test topo_reroute52 --test loop_local`
- A/B job scripts: scratch/boards/thick-layer/ab3_job.sh, ab4_job.sh, ab5_job.sh, ab6_job.sh (tl8), ab7_job.sh (tl9 s1st) and ab7b_job.sh (tl9b off hash) (the arms are explicit in each).
- Pattern PNGs: render locally with `PYTHONPATH=$PWD:$PWD/plugin python3 ../../boards/thick-layer/pat/png_wrap.py ../../boards/thick-layer/pat/<p>.py <runs root> <out> <p>`.
