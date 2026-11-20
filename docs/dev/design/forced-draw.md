# forced-draw: the never-drop pass draws by the first-draw rules (A1c step 2, P2)

**What.** The forced (never-drop) run, draw.rs forced branch (`!ok && self.force`, today 3575-3750), keeps its
candidate generation: ladder attempts with violations, relaxed exact, funnels, `pick_forced`. Two parts change:

1. **Graded octilinear DP for forced runs.** Today `octilinear(.., relax)` treats every clearance shortfall up to
   `relax` (= the run's deepest violation, 0.02-0.53 mm on qmtech) as *free* and every deeper one as *forbidden*:
   a threshold. qmtech det s0 (TOPO_OCTIWHYF, debug, copper equal to base): of the legs the 18 forced DPs tested,
   44-70 % were refused for clearance (strip/wall refusals are small), and 30-90 % of the "legal" legs were below the
   full rule, so free. The result: either no octilinear form ("none": 72 off-45 segments), or a form the checker
   then finds worse ("rejected": 44). The new DP runs at the full rule (`relax` 0 in `Wp`) and, in forced mode,
   **prices** clearance instead of refusing it. Every 45° leg and every taut piece (in a forced run the taut piece
   is not "legal as drawn") pays `W_violation x sum violation_amount(depth, len)` over the near objects it comes
   too close to. Depth = need - distance (same `need_of`, same entities, own-net pads included). Len = the leg's
   length inside the need band, found by bisection from the closest point (the distance along a segment is convex
   for a convex core). The topology tests (region, strip, walls) stay hard, as in the first draw. Everything else is
   the first draw's DP unchanged: length, bend price, jog, `any_angle`, `acute_bend`, `planned_room` zones/gates of
   the undrawn wires (the other forced wires), bundle follow, pad exits / pad-centre ends (`run_end`, `pad_exit`).
   All terms are in mm on one scale: `violation` 1000 per unit, `any_angle` 15 + 2/mm, so a leg below the rule is
   never free.
2. **One graded choice instead of the gate.** Today the octilinear form is taken only if its price is no higher
   *and* `key_le` (a monotone gate). New: the taut candidate and its octilinear form are both measured by the
   checker (`viols_of` + `graze_viols`, as now) and the one with the lower
   `(short amount, W_violation x amount + W_any x any_angle amounts + W_acute x acute amounts + length mm)` is
   drawn. Shorts stay first: KiCad Cleanup deletes a shorting track, so a short is not a graded judgement call
   (existing `viol_key` rule; "KiCad tools are the spec"). Weights come from the factor registry the octi config
   already reads, so `[factors]` overrides apply.

**r1 finding: shorts first in the DP too (qmtech `Net-(U9-~{RST})`).** The DP above priced a leg's
overlap with foreign copper like any clearance shortfall, while the key ranks shorts first. On U9 the
DP's form cut through the corner of a foreign via (checker: short, 46.8 units) where the taut run only
missed the clearance (37.1 units, no short); the key rejected it and the run kept 4.2 mm free angle.
Fix: `seg_deficit_parts_near` splits each object's amount into clearance and short (copper over
foreign copper that is not a hole, the edge or a keepout = KiCad `shorting_items`, the checker's
`gap < 0`), and the DP runs in two price variants: a short priced like a clearance unit (`viol_nm`,
the DP above) and shorts first (`short_nm = viol_nm x SHORT_FIRST`, 1e4: far above a run's clearance
+ style + length, graded by the short's depth and length). Both forms are measured by the checker;
the key picks among taut and the forms (`forced_form_pick`, strictly cheaper, first on ties). Why two:
the DP prices per leg, the key per object (`merge_viols`: one violation per object and layer, max
depth x summed length, a short as a whole once any piece overlaps), so the shorts-first variant
alone made some forced runs worse by the key (qmtech: 6 of 18 forms with more short amount, e.g. the
DP crosses a track at a steeper angle but runs longer along it in its clearance band, which the key
counts at the overlap depth). Per run the key is then at most the one of either variant alone.
A per-object (non-additive) DP price is the follow-up, not r1.

**Counted, never silent.** Each `any_angle` stretch is already an event and a warning (core.rs `settle`). New: a
forced-pass counter and one warning line, "never drop: N forced runs keep M free-angle stretches (X mm); their
octilinear form costs more violation (layer change: the plan's decision)", plus `stats.forced_free_*`.

**Layer first.** In this step a forced run with a blocked 45° stretch cannot change layer: its vias were placed
before the run. octi-fallback-layer-first puts that decision in the plan (P4 / layering sees the forced wire,
which feeds back like an unrouted one, core.rs:9047). Proposed follow-up, not in r1: a drawing-level "via pair to
the other layer" candidate for forced runs.

**Literature.** Polubasov 2010 (Электроника НТБ 6/2010, «Многовариантная оптимизация»): a design-rule violation is
not compensated by length, hence violation 1000 against style 15 on one scale, not a lexicographic order beyond
shorts. Bast, Brosi & Storandt 2020 sections 3-4 (octilinear DP, prices per bend and leg) and section 4.3 p. 7 (room
for later lines as a cost) are the existing DP's basis. Luzin & Polubasov 2002 (costly-not-failed: route
everything with conflicts, then optimise them away).

**Invariants.**
- First-draw copper is unchanged (`relax == 0` path is byte-identical; jst_fanout_45_close and the forced-free
  boards keep their hash).
- The forced DP's chosen chain never prices above its own taut chain under the same prices. The taut chain is
  always a candidate chain.
- The drawn forced form is the cheaper of the two by the key above.
- Determinism: lanes and memo give the same copper for any lane count.

**Tests (targeted, rule 14).**
- (T1) `seg_deficit` fuzz against a slow sampling reference (Pt / Seg / convex Poly cores, degenerate legs).
- (T2) `octilinear_shape_cache_matches_the_reference_loop` extended: forced mode in both loops, bit-equal.
- (T3) Forced DP chain price is at most the taut price (random fields, `with_octi_fields`).
- (T4) The choice key as a pure function: shorts first; +1 violation unit loses to one 10 mm free-angle stretch;
  equal violation, fewer stretches wins.
- (T5) The warning text and its counters.
- Mutants on each changed line set.
- Board: qmtech det s0 base vs tip with `TOPO_RUNPROV` (forced/ladder/other writer per segment, off-45 per writer,
  MCP-DRC classes per writer), each changed forced connection judged.
- jst_fanout_45_close recorded as an observation only.
