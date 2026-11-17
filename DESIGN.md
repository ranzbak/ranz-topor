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

## edge-clear (2026-11-17): track ends off the pad centre, copper-to-edge as KiCad measures it, rule C

### Cause (fx pattern `edge_pads`, main 0a53849, seq and erbs: x = 0.600, 1 copper_edge_clearance each)
0.2 x 0.5 mm pads at x = 0.6, a 0.25 mm track, copper-to-edge 0.5. The plan is right; every run starts and ends at
the pad CENTRE (`draw_wire`, `cutflow::wire_stretches`), where the track's end disc (wider than the pad) is 0.025 mm
into the edge clearance: no legal start node, never-drop forces the straight line at 0.600. Neck cannot help (pad
0.2 < the board's min track width 0.25). Second: `edge_clear()` added half the Edge.Cuts stroke (39e7297).

### Rules
- **A. Pad-end anchor** (`topo::padend`): a run end on a pad ends at the centre unless the end disc there breaks a
  STATIC clearance (board edge, foreign pads/holes/shapes/keepouts; the drawing's own `need_of` / `seg_clear`); then at
  the nearest point inside the pad (inset 1 um, KiCad connectivity: the end in the pad) that keeps them (push from the
  worst violated object; 16-ray bisection fallback). None legal: the centre (never drop prices it, counted). Once after
  the vias, before ERBS and drawing (draw-order independent); used by the drawing's run ends, the octilinear pad-exit
  leg and the ERBS stretch curves (`cutflow::Plan.ends`). Log line "pad ends: ...".
- **B. Copper-to-edge from the Edge.Cuts centre line**: `Rules::edge_clear() = edge_clearance`. KiCad 10 sets the
  stroke width 0 for edge shapes (pcbnew/drc/drc_test_provider_edge_clearance.cpp, tag 10.0.8 l. 295-298:
  `if( item->IsOnLayer( Edge_Cuts ) ) stroke.SetWidth( 0 );`); the 10.0.8 DRC of edge_pads reports "actual 0.4750 mm"
  for the track at 0.600 against a 0.05 mm-stroke edge (no half stroke). Every `edge_clear()` user (checker, obstacles,
  planner corridor `proj.edge_clear`, drawing ents, tidy, ERBS via the packer) and the raw `edge_clearance` users
  (exit stub, ignored-rule factor) agree.
- **C. `[rules] edge_clearance_zero`** (user decision 2026-11-17; `toporlib/edge_zero.py`, applied by
  `plugin/topor/run.py` to the engine's `rules.edge_clearance`), consulted only when the resolved copper-to-edge is 0
  (a non-zero Board Setup / custom value always wins); one value for all layers: `"zone_clearance"` (default) = the
  biggest clearance of the board's copper pours; `"board"` = 0 as KiCad; `"pours"` = the biggest per-layer least
  distance of the fills near the edge. KiCad's filler does not use a zone's clearance for the edge: Edge.Cuts is knocked
  out at `max(PHYSICAL_CLEARANCE, EDGE_CLEARANCE)` with line widths ignored (pcbnew/zone_filler.cpp tag 10.0.8,
  `ZONE_FILLER::buildCopperItemClearances`, lambda `knockoutGraphicClearance`, l. ~2409-2450); the zone local clearance
  enters only `CLEARANCE_CONSTRAINT` (pcbnew/drc/drc_engine.cpp `DRC_ENGINE::EvalRules` l. ~1983), the implicit
  edge constraint is `bds.m_CopperEdgeClearance` (l. 231-232). Measured on refilled wiccon / qmtech: the fills reach
  the edge line (0.00002 / 0.0005 mm), pour clearances 0.508 / 0.254. Every mode keeps the EDGE_CLEARANCE_ZERO warning
  (Board Setup -> Design Rules -> Constraints -> Copper to edge clearance) with the value used; report.md Settings.

### Invariants
- Centre legal => anchor = centre bit-for-bit. The anchor lies inside the pad, its disc legal against static features
  whenever such a point exists; it is only taken when the centre is illegal (never a new violation).
- C is stricter than KiCad or equal, never looser; KiCad's DRC is unchanged by it.

### Tests
Engine `topo_pad_end` (edge_pads x 0.625..0.6251 both modes, corner pad, no legal point, legal centres stay, inset),
`check_clearance::edge_clearance_ignores_the_outline_stroke`; pytest `tests/test_fx_edge_pads.py` (mechfix, KiCad DRC
0 and x optimal, seq + erbs); `tests/plugin/test_edge_zero.py`; run flow `test_edge_zero_rule_c_*`,
`test_edge_non_zero_board_value_wins_in_the_run`; mutants `tools/edge_mutate.py`.

### Follow-up
Neck ladder before the centre fallback when no legal anchor exists (neck the end to the widest width that fits, floor
= min track width, warning): not a small change (the neck is decided by the planner / drawing per stretch, the anchor
per end); counts of "no legal point" ends per board in the edge-clear report.
