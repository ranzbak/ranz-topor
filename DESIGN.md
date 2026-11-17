# DESIGN: ERBS by funnel (radii-m3, M3 step c rebuild, 2026-11-14)

Status: already implemented (erbs.rs, commits c60428f..1399b7a); this note is sent after the fact (rule came mid-round).

## What
The ERBS of a stretch = the shortest path in the band's homotopy class among point obstacles
(Dayan 1997 PDF pp. 25-26 = printed pp. 14-15: "a minimal length member of the topological class",
terminals = RBS terminals + spoke ends; spokes PDF p. 32). It replaces the gate-order solve (free-meeting
order + Newton/Gauss-Seidel), which had no notion of side and zigzagged (eprom st2 1.21).

## How (engine/src/topo/erbs.rs, module doc steps 1-4)
1. Obstacle points with a side label: per cut item both spoke tips (a on p's side, b on q's side of the
   band's crossing, `p_left_of`); per wrap the arc at the stack radius sampled in the wedge (octagon
   vertices / circumscribed polygon); other-net features grown by w/2 + clearance; a frame box.
2. Rough path = band + thin loops round every point the band passes on the wrong side (a tip whose spoke
   the band crosses: Dayan p. 32 "goes round the spoke's end"; every arc sample; grown features on the
   other side). Loops leave from the nearest band point, ordered by (arclength, angle), farther-first on a ray.
3. Sleeve: walk the rough path through a Delaunay triangulation of the points; reduce back-crossings
   (Erickson, lecture notes "Shortest (Homotopic) Paths", section "Crossing Sequences",
   docs/literature/erickson-shortest-homotopic.html: homotopic iff equal reduced crossing sequences, adjacent
   equal crossings cancel; Efrat, Kobourov & Lubiw 2006 PDF p. 9: "follows the path through the
   triangulation", O(n + k). Hershberger & Snoeyink 1994 is in docs/01-bibliography.md, not in docs/literature).
4. Funnel (topo::funnel, branch funnel-fix) through the portals; Popov & Popov 2012 PDF p. 3 Fig. 3
   (funnel over obstacles classified left/right), p. 2 Fig. 1 (taut thread along arcs).

## Invariants
- Path homotopic to the rough path; every bend lies on an obstacle point and turns toward it.
- No point on the wrong side; length <= band (+ detours) ; sleeve failure = loud ERBS_FALLBACK, drawn sequential.
- O(n log n) per stretch; no rayon (sequential, deadline + cancel checked between stretches).

## Tests
Unit: sleeve_straight, pushes_round_a_tip, pushed_both_ways, crossing_portals, collinear_detours_far_first,
near_points_merge, touched_tip_is_no_detour, conflicts_counted, band_sides, grown_rectangle, erbs_u_turn_round_a_pad,
erbs_deadline_and_stop; judge patterns (4, both modes) at 1.000 / 0 violations; harness/erbs_accept.py marks
(a) eprom st2w2 <= 1.03 x band, 0 reversals, bends explained; (b) eprom+wiccon p50 <= 1.02, max <= 1.15, esp agg <= 1.10;
28+ source mutants (m3-runs/funnel/mutate.py); ladder seed 0 both modes with harness/area_check.py.

## Known limits
- Tips are per item: a later own visit's tip (same cut) bounds an earlier own wrap from outside (slot model,
  conservative, wiccon st46 +0.07 mm).
- Pad exits stay out of the sheet (M7).

## Round g (2026-11-16): length factor + three obstacle rules (sent to the lead before the board runs)

### What
1. **Length factor (draw.rs attempt -1).** A legal ERBS of length L over its band B is priced
   `L (1 + k max(0, L/B - 1)^2)` (`[drawing] erbs_length_weight` k = 1): the length up to the band, smooth,
   growing, no cutoff. Past `B (1 + g)` (`[drawing] erbs_margin` g = 0.05, about the octagon's 5.5 % on arcs) a
   sequential run is drawn too; it replaces the ERBS when `L_s (1 + g)` is below the ERBS's price (the margin =
   the room the ERBS keeps for the others). No legal sequential run: the ERBS stays. Finding ERBS_LONG lists them.
2. **Arc of a wrap whose feature the band runs through / wraps from inside**: no feature exit offset (the rays
   of the wedge run over the body; esp L0 st 6: samples at 1.09 mm round a 0.31 mm pad, 3.7 x band).
3. **Grown feature point = spoke end of the feature**: the rough path goes round it only when the band crosses
   the spoke (the segment from the feature vertex it was grown from to the point). Dayan 1997 PDF p. 32
   (printed p. 21): spokes "push wires away from fixed objects"; the rule that only a band separating a spoke tip
   from its root goes round the tip follows from spokes as artificial terminals and the routability iff
   (PDF pp. 25-26, Fig. 6). A point the band does not separate from its feature (past a band end, off a short band's
   corner, sided only by the extended pieces) keeps the band's side (qmtech: 1-1.4 mm detours, 16 x band).
4. **Tips at a squeezed feature**: a spoke tip at a feature of an over-full cut the stretch is on (`skip`, the
   cut's squeeze is its predicted violation) is no detour, as the feature itself is already left out (the
   over-full cut has not the room the tip asks for; Dayan PDF pp. 25-26: an ERBS satisfies the spacing iff the class is routable,
   so an over-full class cannot be made legal by spokes; spoke creation, PDF p. 32, only detects it). qmtech: 0.63 mm wire between pads 0.7 mm apart, long cuts' tips 0.505 mm from both
   pads, the ERBS wound round them (4.6 x).

4b. **Pad exit** (added after the lead's OK, sent before the board run): a feature whose clearance zone holds a
   terminal of the stretch (`poly_dist(terminal, feature) < w/2 + clearance`, or the terminal inside it) is in
   the stretch's `exit` set: it gets no spoke tips and no grown-point detour, like a `skip` feature. Its
   violations are predicted (they are the neck's own: the wire is wider than the room at the pad), counted in
   the ERBS line, and the drawing's neck handles them as today. qmtech st 47: 0.63 mm wire leaving a 0.3 mm pad
   at 0.5 mm pitch; both neighbours' long-cut tips lie 0.5 mm off the terminal on both sides, the ERBS wound
   round them. Dayan PDF pp. 25-26 (ERBS legal iff the class is routable; a neck-violating class is not); PDF p. 32: spoke creation checks, it does not repair.
   The violations are predicted at the pad exit only (within w/2 + clearance of the band's end), not along the stretch.
5. **Arcs round a feature wrapped from inside** (extends 2): no arc samples at all at a vertex whose feature
   the band crosses or wraps from inside (`through`); the wrap's vertex stays a point of the path.

### Invariants
- The class of the plan is never changed by a point the band is not separated from by a spoke (3, 4).
- Over-full and pad-exit violations are never silently passed: predicted, counted in the ERBS line, and the
  0.7 mm-pads test (`erbs_squeezed_pads_keep_the_band_and_the_finding`) asserts they stay findings.
- A legal ERBS is never dropped for its length; it competes on a smooth price (1).

### Tests
Unit `erbs_length_price_and_competition`; integration `wound_erbs_gives_way_to_the_sequential_run` (test point
"wind erbs"); unit fixtures for 2-4 (dip / no dip / squeezed tip); erbs_accept marks on eprom/wiccon/esp dumps;
qmtech dump ratio list; mutants per rule; deterministic ladder seed 0 rounds=1 both modes.

## Round h (2026-11-16): the rough path's class (draft, sent to the lead before commit)

### Finding (sleeve dumps of the round-g runs + cut data, `m3-runs/h/tools/{conflict,legs,wind,nwrong}.py`)
"Walk does not end" and long ERBS have three causes, none of them a repeated loop round one point
(points are merged at 20 nm, one loop per point):
- **A. Cap, not a cycle** (esp st 38, qmtech st 156/164/367): 1011 tips round a pad, every one a valid loop
  from the band; the unreduced walk is loops x depth, longer than the one cap `8 (faces + rough) + 64`.
- **B. Opposite pushes over-full across two cuts** (esp st 48/49/79/131, qmtech st 359): pushed tips of both
  labels whose spokes cross (esp st 79: 565 pairs; the tips of a vertex's westward cut and of a northern
  root's southward cut claim the same room). No single cut is over-full, the class is: the funnel zigzags
  between the two fans (1.5-3 x the band).
- **C. Unpushed tips on the wrong side** (qmtech st 108/119/177/271/335/364/416): tips left unpushed by rules
  4/4b (squeezed / pad-exit features: 647 + 158 of 805 in st 108) or between two crossings of one cut stay
  obstacles on the band's side; the loops of pushed tips beyond them make the path hook round each (st 108:
  21 x its band).
- Minor: the thin loop's legs (5 nm and 2.5 nm off the ray, proportional to the distance from the base)
  and the walk's 0.5 nm label tie disagree for a point on the leg at 10-20 % of it (2 such points in st 108);
  loops at one band vertex split by an arclength rounding cross (qmtech st 359).

### Rules (erbs.rs, rough path only)
1. **A loop is out, round, back over the same edges** (`Step::Loop`): the way back is the way out reversed,
   so a loop is the conjugate of one turn round its point and never encloses another point. Sources: the class
   is its reduced crossing sequence (Erickson, lecture notes "Shortest (Homotopic) Paths", section "Crossing
   Sequences", docs/literature/erickson-shortest-homotopic.html: "Two paths are homotopic if and only if they
   have identical reduced crossing sequences", adjacent equal crossings cancel); the funnel over the sleeve
   (Efrat, Kobourov & Lubiw 2006 PDF p. 9; it supports the funnel only, not the loop). **The conjugate argument
   is our own reasoning** (round i; no source in docs/literature): the loop's crossing sequence is w R w^-1
   (w the out leg, R one turn round the point's star), so every crossing of w is undone by w^-1 and the loop
   winds once round its own point and zero times round any other. A point on the leg counts as on the loop
   point's label side (round i test `loop_leg_point_of_the_other_label_takes_the_loop_side`: the side of w
   picks the conjugate, so it decides the class).
2. **Walk cap per straight piece** (faces + 8): a straight piece crosses a face at most once with a consistent
   side test, so more steps is a cycle; the total is the real work O(sum of loop depths).
3. **Loops at one base** (within 0.5 nm) share it and go in angle order.
4. **Opposite pushes whose spokes cross push nothing** (both tips `Never`): Dayan 1997 PDF p. 32 (printed 22):
   spoke creation "checks the routability of a sketch"; PDF pp. 25-26: the ERBS keeps the spacing iff the class
   is routable; crossing spokes of the two sides are a spacing the class cannot keep (over-full), the band
   stays there (its violation, as rule 4 of round g for one over-full cut).
5. **A labelled point that does not push and lies on the other side of the band than its label is no obstacle**
   (its label is not enforced; kept, it makes the class hook round it). Covers rules 4/4b's tips and rule 4 above.

### Lead conditions (approval 2026-11-16)
- (a) Rule 4's violations are **predicted**: `Erbs.settled` lists the settled gates; `check_layer` treats a settled
  cut like an over-full one for that stretch (its two features and the stretches on the cut), a settled arc
  or grown point its feature. Test `erbs_settled_pushes_predict_their_violations` (gaps before, predicted after)
  and `opposite_pushes_with_crossing_spokes_settle` (both gates reported). No single-wire board fixture: the
  crossing point X of the two spokes gives |V N| <= |V X| + |X N| <= the two spoke lengths, so for one wire the
  roots' segment is over-full and round g's rule 4 already squeezes it (a probe over 1250 geometries found no
  case); on boards it comes from stacks of several wires (esp st 79: 565 pairs).
- (b) Rule 5 changes obstacles only; `check_layer` checks every segment against every feature. `Erbs.unenforced`
  counts the dropped points; `erbs_unenforced_points_keep_their_features_checked` (squeezed pads: points dropped,
  violations against both pads reported).

### Invariants
- Every remaining obstacle point is on its label side of the band, or pushed (its loop puts it there): the
  class has no contradiction a funnel would turn into a zigzag.
- A loop never changes another point's side (exact reversal).
- "Walk does not end" only for a cycle of the side test, never for the path's size.

### Evidence (sleeve dumps, realise=erbs, job models; base = e1d7930 m3h-dbg1 / m3g4, exp = m3h-exp1)
| board | stretches | "does not end" before/after | len/band > 1.5 | > 2 | ERBS ms sum / max |
|---|---|---|---|---|---|
| eprom | 27 | 0 / 0 | 0 / 0 | 0 / 0 | 20 / 7 -> 14 / 4 |
| wiccon | 66 | 0 / 0 | 0 / 0 | 0 / 0 | 83 / 7 -> 73 / 5 |
| esp | 132 | 1 / 0 | 4 / 0 | 1 / 0 | 1118 / 211 -> 746 / 258 |
| qmtech | 421 | 3 / 0 | 8 / 1 | 7 / 0 | 20362 / 5782 -> 3200 / 338 |
qmtech st 108 20.9 -> 1.04, 119 2.24 -> 1.00, 177 2.34 -> 1.05, 359 2.95 -> 1.04; 164 (29k points) walks in 338 ms.
Left: qmtech st 335 1.70 (unchanged; 52 points claimed by both labels at the merge distance, an over-full
cut at zero width, not the rough path), esp st 38/40/57 1.31-1.38 (a ring of 1029 tips round a pad: the loops
are valid). The spoke grid passes each spoke's cells (Amanatides & Woo), not its box (`seg_cells`, tested
against brute force: `crossing_spokes_share_a_passed_cell`).

### Tests
Unit: `crossing_spokes_share_a_passed_cell`, `loop_legs_agree_on_a_point_near_the_base`, `many_deep_loops_walk_to_the_end` (3600 pushed points),
`same_base_loops_in_angle_order`, `opposite_pushes_with_crossing_spokes_settle`, `unpushed_wrong_side_point_is_no_obstacle`;
all earlier erbs/radii/m3-pattern tests; mutants per rule; sleeve dumps of eprom/wiccon/esp/qmtech before/after
(bad-stretch list, fallbacks, len/band, ERBS time); deterministic ladder seq + erbs.
