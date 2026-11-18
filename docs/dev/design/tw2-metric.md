# TW2 METRIC: ERBS as the default metric stage, no cascade (design note, branch tw2-metric)

Plan: ~/.claude/plans/topor-way-plan.md §4 row TW2, §3 row T9. Base: main 27c36203. Evidence: reports/route-improve-plan-20261118
(M6 phase-1 runs, seed 0, rounds=1). Status: draft for the lead. No engine code yet. Board runs on hold until TW1 lands.

## What
1. **Default `[drawing] realise = "erbs"`** (model.rs:1558, schema.py:189, configuration.md). `"sequential"` stays an option.
   It is also the fallback for every run whose band is not kept: the ladder, then the forced never-drop pass (fixed user rule).
2. **Joint band commit (new; draw.rs + core.rs, after `erbs::realise` at core.rs:5029).** Today the ERBS mode is still
   sequential. Wire by wire in the precedes order, each run's ERBS is checked against the copper drawn *so far*. A rejected
   band's ladder copper then takes room that later bands own, so they are rejected too: the 15 cascades, route-improve
   App. B ("reasons name drawn items of other nets"). New, before the per-wire loop:
   - (a) Every run whose two ends are fixed (pad end or placed via; a branch end set later by `end_at` is excluded) gets
     its band items (`run_items`). All of them are committed to the index at once.
   - (b) Each band is checked once against the index. A violation against a static item (pad, via, hole, outline,
     existing copper) rejects the band (class *static*). A violation against another band adds a **conflict edge**:
     class *predicted* when `check_layer` predicted it (over-full cut, planned crossing), else *gap* (`RunBand.gap`).
   - (c) In each conflict component, keep a maximum independent set. Lexicographic order: most kept bands of the highest
     priority tier first, then of the next tier, ties by yield rank (the route-order key: tier, wide, short; D12 "the
     sort only says who yields"). Exact for a component of ≤ 20 bands, greedy by rank above that. That is a work cap,
     and those components are counted. The bands not kept are uncommitted.
   - (d) Kept bands stay committed as their wire's copper (`reserved`). In `draw_wire`, attempt -1 of such a run adopts
     the reserved handles and does not draw again. The steps after it stay: its octilinear form and the length-factor
     competitor (round i). Each replaces the band only when legal against everything, and only then uncommits the band.
   - (e) Every other run (rejected band, no band, junction end) goes through today's ladder (-1, 0..5). It runs in the
     precedes order against the index, which already holds every kept band. The forced pass is unchanged; its candidates
     include the run's own band with its violations.
3. **Octilinear retry sweep.** Default style is octilinear; the ERBS is any-angle. Today an inner wire is converted
   while its outer neighbour is not drawn yet. With reservations, an inner octagon can hit the outer neighbour's
   any-angle band. A retry sweep re-tries the conversion on the reserved runs still any-angle, outer ones first, and
   stops when a sweep changes nothing (at most 3 sweeps, counted). This keeps today's octilinear share. The real fix
   is the geometric transform of the whole ERBS result (Dayan PDF p. 26 "geometrical transformation [62]" = Staepelaere
   1992; TAUT-PACK follow-up, D11, out of scope).
4. **Counting.** For each run: from its ERBS, octilinear form, competitor replaced it, or cut back to sequential. A
   cut-back run has a class: static / predicted / gap / no band (sleeve failed, junction end, timed out). The counts go
   in one ERBS line and in result.json `route.erbs`. Target: cut back ≤ 5 % of the runs.

## Why (literature)
- Dayan 1997, PDF p. 26 (printed 15): the ERBS branch paths "satisfy the spacing requirements if, and only if, the
  topological class ... is routable". They are "a minimal length member" of the class, and the method "is constructive".
  PDF p. 25: routable iff no cut overflows (Leiserson-Maley). So with no over-full cut no bands conflict, and with no
  conflicts nothing depends on order: every band is kept.
- Dayan PDF p. 17: sequential routing is sensitive to net order. "In simultaneous routing ... no net gets higher
  priority than others."
- TopoR manual 6.3: p. 175, FreeStyle: "Optimal wire geometry is immediately calculated automatically ... maintaining
  the specified clearance". p. 177, Refine: geometry changes "without modifying the topology". p. 179: via moving (F7).
  p. 183: TopoR "routes all connections, even where this causes constraint violations. Most violations are eliminated
  by ... automatic via shifting ... and wire path optimization". So a metric stage keeps the shapes the topology gives.
  Violations stay where the plan is over-full, and they are priced (costly, not failed). The cascade is not part of
  this method.
- Our own reasoning, no source: picking an independent set by tier, then count, then rank. It is the smallest rule
  that keeps "order-free" exact when there are no conflicts and bounded when there are.
- No rip-up or feedback in the drawing (M6, abstract-first): what stays over-full goes back to the plan (TW4/TW5).

## Invariants (tested over every path, AGENT_RULES rule 12)
- **I1, no cascade.** A kept band leaves the copper only through its own wire. That means its octilinear form, its
  competitor, its own wire's failure with force off, or its own forced redraw. Every other copper is checked against the
  index that holds it: ladder, forced runs, forced vias (placed earlier; the bands are computed with them), octilinear
  sweep, pairs (`realise_pairs`), and the `cycles` feedback. Counter `erbs_lost_to_other` = 0, with a debug assertion.
- **I2, order-free.** The kept set and the kept copper (before octilinear) depend only on the plan, the static items and
  the yield rank. Permuting `live` does not change them.
- **I3.** Kept bands are pairwise legal and legal against the static items (by the checker that decides).
- **I4.** Never drop is unchanged. `realise = "sequential"` gives main's copper hash on every board and pattern.

## Tests
- Unit: the selector, which is a pure function. Cases: lexicographic tier and count, rank ties, permutation
  invariance, exact against brute force on random graphs of ≤ 12 nodes, and the greedy above the cap.
- Fixtures: the approved m3, fx and erbs sets only, so no new pattern. With the hook "fail erbs sleeve" (one band
  removed), every other run still comes from its ERBS. On main, two cases cascade (local probe, below). Hook "reverse
  draw order": the kept set and the band copper hash stay equal (smooth). Default = erbs. Sequential = main's hash.
- **Measured on main 27c36203** (local probe, approved fixtures, erbs, rounds 1; file wt-tw2-runs/probe-main.txt).
  The hook removes one band:
  - nested_wraps smooth: one more band is rejected by the fallback's copper, and the length goes 44.96 → 50.16 mm.
  - serpentine_bundle octilinear: one more band is rejected, and the engine violations go 14 → 50.
  - The other 15 fixture × style cases: no cascade.
  - Without the hook, smooth still rejects: river_wrap 5, gap_squeeze 4, swap_vias 3, distant_radius 2, qfn_fan 1.
    The joint check will show which of these are order (fallback copper) and which are static or gap.
  - **Counter gap:** forced runs are not in the ERBS line. serpentine_bundle smooth counts 2 of 3 runs. qfn_two_ends
    counts 0 of its runs, with 4 violations. The new counts close this gap.
- Rust and Python: every class of the ERBS line and of `route.erbs`, plus the schema text. Mutants on each changed line.
- After TW1 lands (VM): ladder eprom → wiccon → esp → qmtech (bornhack indicative), seq vs tip, force on and off
  (`allow_violations=false`), rounds=1, seed 0. Every changed connection is judged against App. A/B of the route-improve
  report. Showcase and pattern sets both styles. qmtech drawing time from the profile line.

## Risks, open
- Some of erbsOFF's 57 closures may have been a fallback squeezing into a band's room. Protecting the band then turns
  that closure into a forced run on the yielding wire. This is judged per connection, not gated.
- Showcase smooth: serpentine_bundle 36 and river_wrap 13 DRC are `erbs_gap` model gaps (worst 0.136 / 0.114 mm,
  erbs-margin report), not order. Step 2 alone does not fix them. I will dump them locally (TOPO_ERBS_DUMP). Any rule
  that comes out of that gets its own addendum to this note before I commit it.
- Q1: is the octilinear retry sweep in scope, or should default octilinear quality wait for D11?
- Q2: should the independent set put tier first (as proposed), or maximise the count only?

## Lead decisions 2026-11-18
- Q1: no octilinear retry sweep in TW2. When a reserved band's octilinear form hits another band, the band keeps its
  legal any-angle form. These runs are counted as their own class (`octi_vs_band`) in route.erbs, per board, for the
  D11 transform. If the count is large on the ladder, I report it before building anything.
- Q2: tier first. It is an option: `[drawing] erbs_conflict_keep = "tier" | "count"`, default "tier", documented in
  configuration.md.
- Counter gap: forced runs go into the ERBS line and result.json.

## Addendum A (draft for the lead): spokes at a passed corner (erbs_gap root cause, showcase)
**Finding** (local dumps, serpentine_bundle smooth, main 27c36203; wt-tw2-runs/sb-sleeve.txt):
- The 2 erbs_gap (0.136 / 0.121 mm, the 24 engine violations on M2) are at a post corner v = (9.2, -15.4). Three
  stretches pass v on the same side.
- The band does not wrap v. It crosses three cuts from v in turn, at 0°, 21.4° and 90°.
- So the obstacle points round v are only the spoke tips on those cuts, at the stack radii 0.325 / 0.775 / 1.225 mm.
  There is also v's grown outline, which holds only the innermost stretch.
- Between the tips at 21.4° and 90° (68.6° apart, one triangle at v) the outer stretch has no point. The funnel takes
  the chord between the two tips, and it sags r (1 − cos 34.3°) = 0.775 × 0.174 = 0.135 mm into the inner wire's room.
- river_wrap smooth looks the same: 0.019-0.021 mm sags between consecutive tips of one vertex, and 0.114 mm on R3-R4.
  I will confirm this with the fix.
- This is not a cascade and not order. It is the ERBS model, and the plan predicts it nowhere (cuts: 0 over-full).

**Rule.** When a stretch's band crosses two consecutive cuts A and B of one triangle, and both cuts have the same end
vertex v on the same side of the band, the stretch gets arc points round v. The radius is the smaller of its two tip
radii there (equal when one stack passes both cuts, as here). The points lie strictly between the directions of A and
B. They are the same samples as a wrap's (octagon vertices in the octagonal norm, the circumscribed `arc_step_deg`
polygon otherwise). They carry v's side label and detour like grown points (`Detour::Auto`). Only this point set
changes. The check, tips and radii stay as they are.

**Source.** Dayan 1997, PDF p. 32 (printed 21): spoke creation "pushes wires away from fixed objects ... The number of
spokes required at each point is related to the final geometry (Figure 11) ... octilinear uses up to eight". PDF p. 33,
Fig. 11: for any-angle routing "the octilinear spokes [are] an approximation to the arc". So the spokes belong to the
point, in every direction the wire passes it, not only along the triangulation's cuts. Popov & Popov 2012 p. 2,
Fig. 1: the taut thread runs along the arcs round the circles.

**Invariant.** For a stretch that passes v between A and B, the path's distance to v is at least the smaller of its
two tip radii there, so the arc and the tips agree. A wire is never pushed past either tip.

**Tests.**
- Unit, erbs.rs: three stretches past a corner across cuts 68° apart. The outer path stays ≥ r − 1 nm from v, and
  `check_layer` finds no erbs_gap. Before the fix: 0.135 mm.
- Showcase: serpentine_bundle and river_wrap smooth go to 0 erbs_gap.
- Mutants: the point set dropped, the radius as max, the wedge bounds inclusive.
- Boards later: the number of erbs_gap per board before and after.

**Approved 2026-11-18 with conditions. Answers:**
- **Sampling.** The new points form the polygon circumscribed about the wedge's own arc, not global directions. The
  arc from tip A (at θ_A, on the circle) to tip B (at θ_B) is split into m = ceil((θ_B − θ_A) / step) parts of
  Δ = (θ_B − θ_A) / m. The vertices sit at θ_A + (i + ½) Δ, i = 0..m−1, at radius r / cos(Δ/2).
  - The tips are the tangent points of the first and last edge. So the whole chain tip A, V_0 ... V_{m−1}, tip B lies on
    tangents of the circle and is convex. A taut path past it never comes closer to v than r (no sag).
  - The octagonal norm uses the octagon's own vertices, as the wraps do.
- **The wrap arc today** uses global directions k · step at r / cos(step/2) (norm_dirs, erbs.rs:303). That is the
  circumscribed polygon, so its edges are tangent and do not sag inside the wedge.
  - Its wedge runs from −d_in to d_out (wedge_share, erbs.rs:293), about 90° past the band's normals on each side. The
    path's tangent points then lie well inside the sampled range.
  - The only latent sag is a tangent point within step/2 of a wedge end. The unit test measures it on a wrap too. If it
    shows, the wrap gets the same local polygon.
- **Extra length** per passed corner, against the true arc r α: Δ²/12 of the arc (Δ ≤ step = 22.5° gives ≤ 1.3 %). In
  the serpentine case (α = 68.6°, m = 4) it is 0.935 against 0.928 mm, +7 µm.
  - Against the illegal chord (0.873 mm) the cost is +0.061 mm, which is the clearance itself.
  - The detour counters on the approved patterns are compared before and after. The change must not be noticeable.
- **Unit test**: the segment distance to v along the whole outer polyline is ≥ r − 1 nm, for both the passed-corner case
  and a wrap case.

## Implementation (2026-11-18)
- `engine/src/topo/joint.rs`: `Drawer::reserve_bands` (steps a-c), `release_bands` (a wire the time limit or a test
  fault skips), `hits_band` (octi_vs_band), `cut_class`, `tally` and the ERBS line text. Called in core.rs right after
  `erbs::realise`. Keys: (connection tier, position in the route order `draw_order`), so the kept set does not depend
  on `live` (hook "reverse draw order" reverses it in tests).
- draw.rs `draw_wire`: attempt -1 adopts a reserved band (no new check; its handles become the run's copper). The band
  leaves the index only through its own run: its octilinear form, the length-factor competitor, an `any_forbidden`
  continue, or the run's failure. `JointState::on_uncommit` counts any other removal (`lost_to_other`, I1; a
  `debug_assert` in core.rs).
- Every drawn run carries a `RunSrc` (`WireOut::run_src`). The counts come from the final wires after the never-drop
  pass, once per run, so forced runs are counted (the counter gap). result.json: `route.erbs` (`ErbsRunReport`).
- Partition: runs = from_erbs + replaced + cut_back + forced_erbs; cut_back = static + predicted + gap + no band +
  junction + redrawn (legal sequential runs and forced runs off their band).
- Octilinear form in the commit (`octi_rescued`, lead: not the Q1 sweep): as attempt -1 does, a band whose any-angle
  form breaks a static rule, or loses a conflict, is kept in its octilinear form when that form is legal against the
  kept bands (losers in key order, so order-free). swap_vias octilinear 8 → 0 violations.

## Lead decision 2026-11-18 (b): gap class keeps both bands
- Finding: with the winner fixed, the loser of a model-gap conflict (0.02-0.13 mm) had no legal room and went to never
  drop (smooth: distant_radius 0 → 4, gap_squeeze 0 → 12, river_wrap 6 → 18 engine violations). Before, its sequential
  copper pushed the neighbours off their bands legally (a cascade that resolved the gap by displacement).
- Rule: a conflict where one of the two bands has a model gap (`RunBand::gap`, no over-full cut predicts it) and whose
  deepest overlap of the clearance is at most `[drawing] erbs_gap_keep_max_mm` (default 0.15 mm) keeps both bands. The
  gap stays as a clearance violation of the band drawn second (`Drawer::gap_viols`, against drawn copper only, so each
  gap is reported once). A deeper overlap or a predicted conflict goes to the selector. 0 = every conflict to it.
- Counted: `route.erbs.gap_kept`, `gap_kept_max_mm`, `gap_kept_list` (yielding wire/run, other wire, overlap, place,
  layer): the TW2b starting list. Source: Dayan PDF p. 26 (routable class ⇒ spacing holds, so the gap is the model's).
- Cost seen on the fixtures (smooth): qfn_fan 0 → 2 and swap_vias 0 → 4 engine violations, where the loser had found a
  legal sequential path; gap_squeeze 12 → 6, distant_radius 4 (two overlaps of 6.6 µm and 0.1 µm), river_wrap 18.
