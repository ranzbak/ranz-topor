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
Dayan & Dai 1991 (UCSC-CRL-91-40) PDF p. 1, abstract: vias (and Steiner points) are positioned *homotopically*, i.e.
moved only within the sketch's topological class; Dayan 1997 PDF p. 29 (printed p. 18): the end-points of the rubber
bands are terminals, vias or junction points, so a via is an end of the bands on its layers. A via placed on the far
side of a wire the plan does not cross there changes the class (the run must cross it), so the site is not in the
plan's class; the drawing only realises the plan, so such a site is no candidate while an in-class one exists.
(Round 1 cited "Dayan 1997 PDF p. 32: vias are fixed objects"; p. 32 is about layer assignment and spokes, verifier F6.)

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

## radii-m3-clear round 2 (2026-11-17, user decisions on the verifier's findings)

#### What
1. **Class per traversal and layer** (F1). `crossed` was the set of nets the plan crosses wire w with anywhere, over
   all layers, so a crossing with VIN5V elsewhere on JS_INTB_5V made its site across VIN5V's F.Cu track in the via
   traversal look in class. Now `Drawer::class_crossings(w)` = `{(k, net, layer)}`: the traversal k of w, the other
   net, and the other wire's layer in its traversal there (`prov_layer` of its two passes; -1 = open: a via interval of
   its own, or no pass layers). `via_reach` in traversal k: a reach on layer l over a drawn track of net n is in class
   only if `(k, n, l)` or `(k, n, -1)` is in the set. The reach segment runs from the taut chord of k to the site, both
   in k's triangle, so the traversal is the place where the plan decides on which side of n the via stands.
2. **Reach price opt-in** (F2): `[vias] reach_price` (Options `via_reach_price`, default false). Off: the reach's
   violations add nothing to the searched or the forced site's price; the class verdict applies either way. On:
   round 1's price (`octi.room x dv x reach` searched, `+ reach` forced). Off by default until VS-PLAN (the planner's
   via-site room check) is fixed: the price re-picks in-class sites board-wide (qmtech MAX_SCLK +2, esp USB_D+).
3. **Forced over every site counted** (F5): a via forced with `force_via(.., None)` whose site fails the class test
   is counted in `via_out_of_class` (the summary line understated it).

#### Invariants
- With the price off, copper changes only where a site that won before is out of class now.
- No via dropped (unchanged order: in class legal > in class forced > out of class legal > any site forced).

#### Tests
- topo_d2 `a_via_site_across_an_uncrossed_drawn_wire_is_out_of_class`: crossed in this traversal on the track's layer
  = in class; on the other layer, or in another traversal = out of class; open layer = in class.
- topo_d2 forced path: in force mode, no legal in-class site, a legal out-of-class one: the via is forced in class.

## radii-m3-clear round 2: C27.1 pour island (design note; lead OK 2026-11-17 with conditions: runtime per board, coarse stubs bit-identical, fixture + mutants, parked prevention)

#### Data (qmtech s0 rounds=1, tip fc98488, reach price OFF; TOPO_STITCHDBG, job m3c2-1)
- The class filter alone moves JS_INTB_5V's via to (133.020,121.269) in BOTH modes (VIACAND: the old site
  (132.887,118.904) is now `in class false`). seq: 401 -> 400, only JS_INTB_5V changes (DRC 8 -> 7). erbs: 407 -> 396
  (JS_INTB_5V 17 -> 7, JS2_6 10 -> 12) but unconnected 24 -> 25: the C27.1 island. So the loss is the filter's, not
  only the price's.
- C27.1 (GND, F.Cu) is boxed in on F.Cu by JS_INTB_5V's F.Cu run (west, x 133.02) and VIN5V's 0.5 mm track
  (east / north diagonal); the stitcher needs a pad stub to a via standing in In1.Cu's main GND fill. Its lattice
  (rings 0.1 mm, 32 directions) has: k8 (134.152,121.850) -> In1 fill depth < via r + ring (JS_INTB_5V's via antipad
  1.27 mm away); k7 (134.328,121.867) -> 24 um short of clearance to VIN5V's F.Cu diagonal (erbs geometry; in seq
  the same k7 is legal, which is why seq keeps its stub). Every other lattice point fails (own pad, U6 pins,
  JS_INTB_5V / VIN5V copper).
- By hand: a legal site exists between k7 and k8 (about x 134.21..134.30 at y 121.87: In1 depth >= 0.65 mm,
  >= 0.8 mm off VIN5V's diagonal, clear of the pad). The coarse lattice steps over it (0.18 mm arc spacing).

#### Proposed rule (stitch.rs pad stub, generic, no board case)
Second, finer lattice only when the coarse one finds no stub for a pad: rings every 0.025 mm, 128 directions,
out to 1.5 mm (`stitch::STUB_LATTICES`; pretests first = raster lookups; checker queries capped as now, 4 x MAX_TRIES).
Nearest first. Work cap over one stitching run: `FINE_POINTS_MAX` = 100 000 candidate points (about 15 pads' full
fine search); report line "fine stub lattice: N cut piece(s) searched, M stub(s) found, P points (T ms)".
Stubs that the coarse lattice finds are unchanged (no copper change elsewhere). This is the user's own hand technique
(qmtech hand-fix lessons: GND pad vias, nudges <= 0.3 mm); TopoR leaves islands to the user (TopoR 6.3 manual p. 121).
Not the root cause: the plan has no price for the pour access its copper takes. That prevention (the plan pricing
the pour access of its via sites and runs, graded) STAYS PARKED (lead: on the parking list); it is a planner change.

#### Test
topo_d4 fixture: a GND SMD pad boxed in by two foreign tracks so that the only legal stub via site lies between two
coarse lattice points: before the fix POUR island stays (after = 1), after the fix one pad stub (after = 0). Mutant:
fine pass off; fine radius 0.

#### Measured (job m3c2-2 at 982bb11, in-run A/B hook TOPO_STITCHAB: the same stitching coarse-only on the same items)
- qmtech erbs: C27.1 fixed (pad stub (134.284,121.860), 0.900 mm, via); unconnected 24 = main; coarse-only items all
  kept (only-coarse 0, only-fine 2 = this stub). Fine lattice 24 pieces, 1 stub, cap reached, 650-775 ms per call.
- qmtech seq: NOT bit-identical. The fine lattice fixed a GND pad at (160.773,-73.904) in pass 0 with a new via
  (0.98 mm); coarse-only, the same pad gets a track-only stub (1.3 mm, no via) in a later pass, after another piece's
  fix made that F.Cu fill main. So in pass 0 the fine lattice pre-empted a coarse stub of a later pass (+1 via).
- Fix (this round): the fine lattice gets its own passes after the coarse passes are over (`FINE_PASSES` = 2);
  everything the coarse passes place is the old copper; in a fine pass the coarse lattice is still tried first
  per piece; a piece the fine lattice failed on is not searched again; a pass after one that placed nothing reuses
  its reach and skips the (identical) island-via and coarse searches. Test: `the_fine_lattice_waits_for_the_coarse_passes`
  (a second, roomy boxed GND pad is fixed before the fine stub in both piece orders; mutant `fine_on` from pass 0
  fails it).
