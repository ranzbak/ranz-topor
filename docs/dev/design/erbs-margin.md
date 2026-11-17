# erbs-margin design (rounds h, i)

Moved from DESIGN.md at the merge with main 385e043. Also changed there: the sleeve step's citation (Erickson, docs/literature/erickson-shortest-homotopic.html, crossing sequences; Efrat, Kobourov & Lubiw 2006 p. 9; Hershberger & Snoeyink 1994 is in docs/01-bibliography.md).

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

## Round i (2026-11-17): the length factor's sequential competitor keeps the undrawn ERBS (approved by the lead 2026-11-17)

### Finding (qmtech erbs ADV_DE, round g -> h: 28.89 -> 38.98 mm, detour 1.12 -> 1.51, new ADV_CLK/ADV_DE crossing + short)
Not the rough path. Dumps of the ladder's own run (hz-runs/m3i-dump-tip, -main; repro m3i-repro1 with the new
`TOPO_ERBS_DEBUG` line "ERBS draw wire ..."):
- Round g: both ERBS failed ("walk does not end", st 254 ADV_CLK, st 384 ADV_DE); both drawn sequentially in the
  given order, ADV_DE (0.25 mm) before ADV_CLK (0.127 mm): ADV_DE clean, ADV_CLK forced round U429 (8 DRC).
- Round h: both ERBS exist and are good (ADV_DE 27.59 / band 27.23, ADV_CLK 23.60 / 22.56, wrapping ADV_DE's
  terminal U429 as its band does). `precedes` puts ADV_CLK first (inner on a cut ADV_DE bends at).
  ADV_CLK's ERBS is legal but priced past `band (1 + erbs_margin)`, so the length factor draws a sequential
  competitor: attempt 0, **18.92 mm, straight along y = 87.91 into U431, crossing ADV_DE's planned path at
  (116.8, 87.9)** (ADV_DE not drawn yet, so the checker sees nothing). It wins (`replaced true`). ADV_DE then
  has no legal geometry (ERBS 11 nm short against the new copper, exact windows "no path" at the U4 end) and is
  drawn by force: the 38.98 mm detour and the crossing. Same pattern on wire 358 (net 55, L1: 17.05 vs band 20.97).
- The length test is no class test: 45 of 394 drawn ERBS are shorter than their band (the band is the sheet
  walk, not taut), so "shorter than the band" cannot tell a class jump.

### Rule (draw.rs, erbs mode only: `kept` is set only there, seq copper unchanged)
A sequential run that competes with a legal ERBS (`erbs_vs_sequential`) replaces it only if, for every other-net
wire not drawn yet, it crosses that wire's reference path on this layer as often mod 2 as the kept ERBS does
(`competitor_keeps_class`; refined from "no more often" after the ladder, approved by the lead 2026-11-17, see
"Parity" below). The reference path is the wire's ERBS run, or, on a layer where it has none (sleeve
failed, no band), its planned path: the chords of its passes through its turn points (`build_class_ref`), so no
undrawn wire is skipped. Crossings are counted topologically: proper segment crossings by orientation signs
(`seg_proper`), no distance threshold. Candidates come from a 1 mm grid of the reference segments, queried by
the boxes of the run's segments (no all-pairs scan); the grid is built once per drawing, at the first comparison. Otherwise the ERBS is drawn (it is in the plan's class; the competitor would take a path
the plan gave another wire: a planned short). Not a threshold on a judgement value: it is the precondition of
the comparison ("a sequential run of the same class", `erbs_vs_sequential`'s own doc), checked instead of
assumed. Sources: Dayan 1997 PDF pp. 25-26 (the ERBS is a minimal member of the topological class; the
realisation keeps the class), PDF p. 32 (spokes keep the room of the others); TopoR way: the topology is decided
in the topological phase, the drawing only realises it (Polubasov 2010, docs/literature/polubasov2010-topor-inside.html, section 1: TopoR is a topological router, "стадии трассировки и метризации разделены", routing and metrisation are separate stages).
Parity (the refinement): Erickson, docs/literature/erickson-shortest-homotopic.html (lecture notes, HTML, no
page numbers), section "Crossing Sequences": "Two paths are homotopic if and only if they have identical reduced
crossing sequences"; the reduction cancels a crossing with the next crossing of the same cut (the example
"...DDD..." loses a "DD"), always two at a time, so the number of crossings of each cut mod 2 is a homotopy
invariant. The other wire's reference path runs between its two terminals (punctures), so it is such a cut;
our use of it as one is our own reasoning. The rule is **sound, not complete**: a different parity proves a
different class (the competitor is refused); an equal parity does not prove the same class (it competes as
before round i). Raw counts are no invariant: two representatives of one class differ by bigons of 2 crossings.
The taut pull holds the same invariant against undrawn chords (`Drawer::pull_taut`); both read the same code
(`proper_crossings`, `Drawer::plan_chord`).
Ladder evidence for the refinement (hz-runs/m3i-lad1, "no more often"): qmtech wire 358 (HDMI5V, B.Cu): the
competitor crossed ADV_SCL's (wire 459) path 4 times, the ERBS 2 times; keeping the ERBS left ADV_SCL drawn into
HDMI5V's via (+6 DRC, 2 shorts, ADV_SCL connection DRC 2 -> 8). Same parity: one class, the competitor competes.

### Invariants / tests
- The competitor crosses every undrawn other-net reference path with the kept ERBS's parity
  (draw.rs `class_test_counts_proper_crossings_per_wire`, `class_test_bigon_is_one_class` (4 against 2: one
  class, the competitor competes; 1 against 0: refused); tests/topo_m3_class_keep.rs on the synthetic
  pattern `shortcut_across` (harness/radii_patterns.py, approved by the user 2026-11-17): A drawn first,
  its straight competitor forced by the test hook "straight sequential run" crosses B: A keeps its ERBS, no
  crossing; with "erbs class keep off" A is straight and B is drawn across it, the qmtech symptom. With B's
  sleeve failed ("fail erbs sleeve") B's planned path keeps A's ERBS the same way.)
- Mutant: predicate always true (competitor always eligible) must fail the drawing test.
- Seq mode untouched: `kept` is set only with realise = "erbs".
- Ladder: seq copper identical on the 5 boards; erbs per-connection table for qmtech/esp (ADV_CLK, ADV_DE, net 55).
