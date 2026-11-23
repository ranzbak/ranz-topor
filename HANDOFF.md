# HANDOFF thick-layer (2026-11-23 ~01:20, context swap at ~340k)

Worktree scratch/agent-wt/wt-thick-layer, branch thick-layer off main 65d7901b, tip b12437d1 (do not merge; lead decides).
Target dir scratch/agent-wt/wt-thick-layer-runs/target. Report: scratch/agent-wt/reports/thick-layer-20261123-0120.md/.json.

## Done
- Part 1 (F4) complete: `[output] plan_report` (default on). Code: engine/src/topo/plandump.rs, core.rs (after
  the compass block), refine.rs `CapModel::via_report` / `ViaRow`, route.rs `RouteResult::plan`;
  plugin/topor/report.py `_split_plan` -> plan.json.gz; toporlib/plandump.py (reader/CLI); docs: troubleshoot.md,
  debug.md, route-board.md, configuration.md. Tests: engine/tests/topo_plan_report.rs (3), tests/plugin/test_plan_report.py (4).
  Copper = main on eprom/wiccon/esp/qmtech det s0 (tags tlb/tlt, hash equal). Mutants 4+4 killed.
- Part 2 design: docs/dev/design/thick-layer.md, lead OK'd (S1 -> S2 -> S3, S4 measure-only).
  Lead notes to honour: ω multiplies only via_mm (not site prices, power/diffpair/BGA via prices, via_squeeze;
  say so in code doc); M2 candidates: runtime + CPU per candidate type in the plan report, ordered by gain, no
  cap; defaults by per-connection analysis of det s0 A/B; commit after each stage with a report line.
- S1 started: engine/tests/topo_thick_layer.rs = synthetic thick_dive_grouping fixture, `diag_thick_dive_grouping`
  (ignored, --nocapture prints P's plan) and the S1 target `thick_dive_grouping_predicts_what_it_draws` (ignored,
  FAILS now: drawn 6, predicted 0). Diagnosis is in the design note (§S1 diagnosis): the site of P's last via
  (refine::end_site, interval in traversal 7 between events F4 s .44 and B2 s .65) is computed on the
  pass-point polyline (State::polyline); on the taut lines it lies past B2 and the 1 mm F4-B2 gap holds no
  0.9 mm via. CM1 gap cuts start at static features and miss the crosser-to-crosser squeeze.

## Next (S1, M3)
1. In `end_site` / `Frame::rank` / `via_no_site` / apply's no-site count (refine.rs ~300-360, 881-951, 1064+):
   measure the via between the two crossers bounding its event pair at their TAUT positions
   (st.taut_points of the crossers, or the crossing points on w's taut line), gap along w minus
   (w_o/2 + s)/sinθ per side + d; graded deficit against the crossers' far-side slack (a crosser that can bend
   aside is not an error). Keep `apply` and `via_no_site` in step (doc says so). Option `[vias] site_planned`.
2. Un-ignore the S1 target once it passes; pick a mutant of the new measure.
3. Then S2 (ω in layers.rs:1407 edge weight, Costs::trans_at, refine wire_cost, loop evaluation via term),
   S3 (pair_move re-attach/group/one-side), per the design note.

## Patterns (PROPOSED, not in suite; sources scratch/boards/thick-layer/pat/, copies + PNGs in
reports/thick-layer-20261123-data/)
- thick_dive_grouping: main reproduces (4 DRC).
- wide_one_side r2 (P SMD F pads): main keeps P on F with 0 vias but S6 detours round P's right pad and S1/S2
  bend left for nothing (detours: worth showing the user). r1 (PTH P pads) was invalid: main's 0-via answer beat
  the expected one.
- bus_side_by_side: main 5 unconnected, 3 vias (reproduces). Setup panel labels the stacked F/B pads as B only (cosmetic).
- VM jobs: scratch/boards/thick-layer/{ladder_job.sh (base|tip), fix_job.sh}; use --boards ...,thick-layer.

## Budget
Night budget ≤ 2 ccx43 h. Used: shared topor-vfyppv (verifier's billed hour; hz noted extra deadline cost ≤ EUR 0.54).
EUR 7 left of the 100 total cap (hz status) - be frugal.
