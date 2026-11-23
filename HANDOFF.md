# HANDOFF thick-layer (2026-11-23 ~02:20, round 2 done, partial)

Worktree scratch/agent-wt/wt-thick-layer, branch thick-layer off main 65d7901b, tip 58d62c48 (do not merge; lead decides).
Target dir scratch/agent-wt/wt-thick-layer-runs/target. Reports: scratch/agent-wt/reports/thick-layer-20261123-0120.md
(round 1, F4 + design) and thick-layer-20261123-0215.md/.json (round 2, S1-S3 + A/B).

## State
- F4 plan report (round 1) merged into the branch, default on.
- S1 (M3): refine.rs `end_site_lane` (taut window between planned crossers, lane_room vs other taut tracks,
  no crosser slack) behind `[vias] site_planned` (default false); rank-wide lane minimum behind
  `[vias] site_lanes` (default false).
- S2 (M1): layers.rs `Omega`, `[vias] priority_weight` (default [0,0]); only via_mm weighted; LayerReport.vias_w
  in the loop objective.
- S3 (M2): core.rs one-side pair candidates `[vias] pair_one_side` (default false), gain-ordered, timing per kind in
  metrics `pair_move_kinds`. Ineffective: relayering after the move is free (ignored test thick_dive_grouping_one_side_move).
- Defaults all off from det s0 A/B (tl3, tl4 in scratch/agent-wt/hz-runs): reasons per connection in the round 2 report.

## Open (decisions for the user, see report)
1. crosser slack in the M3 measure (topo_reroute52.rs:83 fails with site_planned on);
2. P2 crossing price for layer-free crossers (wide_one_side detour 91.41 vs 86.00, init.rs:76 cross_mm);
3. ω with congestion-aware crosser via cost (esp IO4-IO7 into U3/U4);
4. one-side move holding the wire's layering.
- Verifier: copper = main at 58d62c48 with defaults off on 4 boards (off arm verified at 147e9302 only).

## Commands
- tests: `cd engine && CARGO_TARGET_DIR=../../wt-thick-layer-runs/target ../../slot.sh cargo test --release --test topo_thick_layer --test topo_reroute52 --test loop_local`
- A/B job scripts: scratch/boards/thick-layer/ab_job.sh, ab2_job.sh, pt_job.sh (patterns + PNGs).
