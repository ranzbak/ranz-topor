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
