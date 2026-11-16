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
   (Hershberger & Snoeyink 1994, reduced sleeve; Efrat, Kobourov & Lubiw 2006 PDF p. 9: "follows the path
   through the triangulation", O(n + k)).
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

## radii-m3-clear (2026-11-16): a band in another stretch's terminal feature is one predicted event

#### What
`erbs::check_layer` sorts every ERBS violation as predicted (a over-full cut, a planned crossing, a feature the band
runs through, a pad-exit feature) or as an `erbs_gap`. Suppose stretch A's predicted violation is at feature f
(A runs through it, or f is A's pad-exit neighbour). Suppose f is a terminal feature of stretch B: a pad, via or
junction of B's own net that holds an end of B. Then A also violates B's ERBS where B leaves f, and that is the same
event. New rule: a stretch-stretch violation A-B is predicted when its place (the midpoint of the two segments'
closest points) lies within `w_A/2 + clearance(A, f)` of such a feature f, and A's predicted class at f covers that
segment (through: the whole stretch; exit: only `exit_run`, as for the feature itself). Symmetric in A and B.

#### Why
On the m3g4 dumps (qmtech L0/L1, esp L0/L1), 24 of the 31 unpredicted stretch-stretch violations that are not near a
winding are of this kind: B's end segment sits inside its own via or pad, and A runs through that via or pad
(qmtech st368 through the via of st324, st296 through the via of st341, st359 through the via of st91; esp 11/41,
44/50, 58/60, ...). They look like tight bundles in the `erbs_gap` count, but they are the plan's own feature
violation seen a second time. With them counted, `erbs_gap` cannot show the real bundle-spacing faults.

#### Literature
Dayan 1997 PDF pp. 25-26 (printed 14-15): the ERBS meets the spacing iff the class is routable. A class whose band
runs through a feature is not routable there, and every violation that comes from that is the plan's (already the
rule for the feature itself, DESIGN round g rules 2, 4b, 5). B's copper inside its own terminal is part of that
terminal (Maley 1987 p. 29: features enclose the element; the wire's end is attached to the feature).

#### Invariants
- Copper is unchanged. The `predicted` flag decides nothing in the drawing; `RunBand.gap` is read by no one.
- A violation that is not at such a terminal feature keeps its class.
- An exit feature predicts only along its `exit_run`, as before.

#### Tests
- Unit `erbs_band_through_a_terminal_via_is_one_predicted_event`: a band through a via of net 2, and the stretch of
  net 2 that ends in it. The stretch-stretch violation is predicted.
- A control in the same fixture: a third stretch that only grazes the end segment of the net-2 stretch away from
  the via stays an `erbs_gap`.
- Mutants: the rule off (`false`); the place test dropped (any terminal feature counts); exit_run ignored.

## radii-m3-clear (2026-11-16): a via site keeps the plan's class (lead's changes to the first note applied)

#### What
`Drawer::place_vias` (draw.rs, shared by both realise modes) searches a site when the plan's site is not free. Until
now it priced each legal candidate (planned room taken + squeeze + distance to the missed site) and never asked whether
the runs on the via's layers can reach the site from the plan's path. New `Drawer::via_reach`: per candidate p in
traversal k, a straight reach segment at the wire's width per via layer from the point of `taut_chord(w, k)` nearest p
to p, checked against the copper drawn so far. A reach that overlaps (gap <= 0) a drawn track of another net that the
plan does not cross w with puts p on the far side of that wire: out of the plan's class, so not a candidate.
- In-class legal sites: price as before + the reach's violation amounts x (octi.room x via diameter), graded.
- No in-class legal site (force mode): `force_via` restricted to in-class sites, reach amounts added to its price
  (costly, not failed).
- No in-class site at all: the cheapest legal out-of-class site as before, counted (`via_out_of_class`, plan-vs-drawn
  summary line); then force_via over every site as before.
The planned site itself (on the plan's path) is not reach-tested.

#### Why (data, qmtech s0 rounds=1, both modes identical; TOPO_VIADBG on the VM, m3c-3)
JS_INTB_5V (wire 452): plan site (134.927,124.399) is tight (25 um from R18.2). Final draw candidates in k 55:
(133.020,121.269) room 12 nm price 8.0e6, legal and on the plan's side of VIN5V; (132.887,118.904) room 538 um
price 1.7e6, on the far side of VIN5V's drawn 0.5 mm track (y 120.469). The second wins. Every L0 candidate for the run
then shorts VIN5V (FORCEDBG: attempts, funnel all short items 1495/1496); force mode takes the funnel with one short,
which grazes U6 pins by 5-74 um. Connection forced in both modes by this site: seq 8, erbs 17 violations (the
"JS_INTB_5V 8->17" of the m3 report is the two modes' force picks after the same bad site). With the
via at 133.020 the plan's ERBS is legal (0 violations, U6 row at 125.724 >= 125.72). So a legal geometry existed.
Of the 8 searched vias in the final qmtech draw, this is the only one cut off from every legal alternative by copper.

#### Literature
Dayan & Dai 1991 / Dayan 1997 PDF p. 32: vias are fixed objects the topology is built round; a via placed on the far
side of a wire the plan does not cross changes the class (the run must cross it), so the site is not in the plan's
class; the drawing only realises the plan, so such a site is no candidate while an in-class one exists.

#### Invariants
- Seq output changes where an out-of-class site won before, or where the graded reach price reorders in-class sites.
  Judged per connection against main (lead: changed seq is fine if better).
- No via is dropped: in class legal > in class forced > out of class legal (counted) > any site forced.
- Root cause upstream (not fixed here): the plan's own site (134.927,124.399) is 25 um from R18.2 (planner via-site room
  check; CM1 / parked VS-PLAN).
- O(candidates x objects nearby) extra checks per searched via.

#### Tests
Synthetic fixture (harness/fixtures.py; PNG to the lead first, patterns need the user's approval): a via whose
planned site is blocked by a pad, a drawn wide track between the near legal site and a roomier far site; expected
the near site, no short. Mutants: class test off; reach on one layer only; force_via without the class; crossed-net
exemption off.
