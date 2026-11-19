# tw4-r2: pad exit as a plan object, the via gap at its best point, a graded pour-cut term

Branch `tw4-r2` off main de9e3977 (TW4 round 2, ~/.claude/plans/topor-way-plan.md row TW4, lead note 2026-11-19
02:30; round 1 = docs/dev/design/tw4-eval.md). Diagnosis data: job t4r2d-fce1d85 (esp, qmtech, rounds=1, via cuts
off/on, plan dump + every planned via priced with via cuts, throw-away branch `tw4r2-dbg`), job t4r2t-65a9671 (every
over-full via pricing of IO6 during the esp run). No engine code on `tw4-r2` before the lead's reply.

## What the data says (main de9e3977, rounds=1, seed 0)
- **Both approved T1 patterns pass on main already** (local probe, rounds 1, 5.10 off and on): `exit_tip_from_plan`
  leaves U1.3 through the outer tip and goes round the chip clean (the plan's homotopy; the inner tip is a dead end);
  `neck_or_detour` takes E1 at full 0.8 mm (x 5.457), no neck (the search's `necking` price, 30 x 1.5 per stretch,
  beats 1.45 mm of length). They become regression tests, plus one new assertion each on the plan object below.
- **Unseen pad exits on main** (tw4_analyse, via cuts on): KiCad pad_exit items qmtech 33 (rounds 1) / 36 (r16 s0),
  esp 4-13; neck_loss qmtech 34 / 65. Of qmtech's drawn pad_exit (r16 s0: 12) **half are vias**: the plan puts the
  layer change at a 0.5 mm pitch pin, its via point lies 0.155 mm off the pin centre with a 0.8 mm via (ADV_DDCSCL
  U3.17, ADV_SCL U7.27, ADV_DVDD, U7-PVSS), the drawing falls back to a 0.56 mm via in the pad and KiCad flags the
  neighbour pin (0.055 mm < 0.15). The other half are tracks at a planned exit neck (`exit_tight` necks the first two
  passes to the floor; nothing checks that the floor fits; `pass_d` = 0 there).
- **IO6 U3.20-U4.11 (esp)**: both final plans price **0** over-full vias (off plan: IO6's via sits in its own pin's
  triangles, no crossers). The detour is decided earlier: during the on run every IO6 via candidate between the
  net-9 pad and the +3V3 pad south of the U3 row (triangles 4108/4109/4114/4121/4128) is over-full on its gap cut by
  **0.001-0.05 mm** (need 1.562-1.587 = clear + 0.8 via + clear + 0.127 IO10_SDA + clear + 0.152 IO5 + clear, len
  1.534-1.586), priced 1 + over/pitch = 1.00-1.08: a full violation for 1 um on one fixed line through one fixed point
  (the triangle's first site point, `cutflow::via_point`). The drawing does not put the via there (viaopt relocates
  along the taut path, `via_relocate` default on); the gap widens 0.05 mm within 0.4 mm south, where the cut fits.
  The sweep (`CapModel::optimise`) then moves the via to the next triangle with no over-full cut (under the chip,
  (150.85, 80.1)), and the B.Cu run from there is the 1.94 detour.
- **Pour islands (esp, rounds 1)**: 13 pieces cut off before stitching off, 18 on; GND C1.2/C2.2/C3.2 (2.15 mm2) and
  VBUS D3.2/D6.1 (5.98 mm2) have "no stitch site". The plan has no geometry of the pours (decompose unions the
  pour-served pads with their pour; nothing records that they need it whole). TopoR itself does not price it either
  (TopoR 6.3 manual p. 121: pouring "may split the copper area into multiple unconnected parts. You can select and
  delete the resulting islands"; TopoR 5.0 help, Plane Nets: the user adds wires to connect islands): our rule
  (planes are terminals; costly, not failed) goes beyond TopoR here.

## (2) Via gap at its best point (general CM1 fix, first: small, measurable)
The gap cuts of a planned via are measured at the **best point of its triangle**, not the first site point: of the
triangle's site candidates (`tri_site_points`, the 9 points `via_point` scans) with static room (`point_free >= r`),
the via point is the one whose gap cuts are least over (`viacut::over` graded, ties: today's order, so a via with no
over-full cut keeps today's point and today's copper). Maley 1987 pp. 32-33 (a sketch is routable iff no critical cut
is over-full) holds for the via at *some* position; a fixed point checks one position only (cm1-room.md claims "the
site the drawing tests first", which is not true with `via_relocate` on). Cost: per (triangle, net) up to 9 static
geometries (cached run-wide as today) and 9 pricings per repricing of that via; option `[capacity] via_best_point`
(default on after the A/B; off = today bit-identical). Graded: the price stays 1 + over/pitch of the best point (no
new threshold; a via over at every point of its triangle still costs a violation).
Not done: a smooth price below over = 0 (the per-edge model has the same step; changing it is its own stage).

## (2b) The via disc on an edge at the via's height (lead decision (B), 2026-11-19)
**Finding** (equal time, rev 3d0bb071; sweep trace, throw-away branch `tw4r2-dbg2`): `via_best_point` takes IO6's
CM1 price at the gap triangles 4108/4109 to 0, and the sweep still leaves the via under the chip. In the sweep key,
the price at the gap is about +1.4 k against about -8 under the chip. That price is the **per-edge packing of the
via disc** (`pack::wire_discs`). It puts the full disc on the edge the wire enters the via triangle by and on the
edge it leaves by, as if the via sat on each edge line. In a pad gap these edges are the narrow chords, so the
disc there is the same fixed-point error as the gap cut at one fixed point. `via_over_ramp` hid it on IO6 by
pricing the CM1 term low, and moved the error to ENABLE (1.07 -> 1.8). The pattern probe `via_gap_best_point`
(rounds 1) takes the far gap with every option.

**Model.** A via of wire w in traversal k lies at its point V (the point CM1 prices: the planned via point; with
via_best_point, the best point). It packs on the entry edge e_in (pass k-1) and the exit edge e_out (pass k), as
today, but at its height h_e = the distance from V to edge e's line, on the via's side, instead of on the line.
- Pair requirement (`pack::pair_req`), disc a against neighbour b (track width w_b, or disc d_b) at copper gap g.
  Today: R = d_a/2 + g + w_b/2 along the edge. New: x = sqrt(max(0, R^2 - h_e^2)), stretched by the neighbour's
  oblique factor as today. The neighbour's centre line must clear the disc, and the disc centre is h_e off the line.
  This is exact for a round via against a straight track crossing the edge. Two discs on one edge use their
  heights' difference.
- End margin (`Packer::margin_all`): the smallest offset along the edge where the disc centre, at the point
  h_e off the line, clears the end obstacle's polygon (`margin` already steps along the edge testing polygon
  distance; the test point moves by h_e along the edge normal). This is exact against the obstacle.
  - A straight pad side parallel to the via's offset keeps its full margin.
  - A round pad, or a pad corner, gives room as the via moves off the line.
- A disc-only item (the wire is on another layer at this edge) has no track term. Its requirement is the sqrt term
  alone, so it goes to 0 when the via sits R or more off the edge, and the via's room is then the CM1 gap cut.
- **Graded.** h_e = 0 gives today's numbers exactly. Every term is continuous in h_e: no threshold, no step.

Maley 1987 pp. 32-33, 40: a via is a point feature, and a cut's capacity is what lies on the cut. A via off the
edge line is on that edge's cut only by the part of its disc the line passes through. Popov & Popov 2012
pp. 117-118: the via is a round pad, and the room of a passage counts what passes it. D3 (wire_discs doc): the
disc counts on both edges; that stays.

**Code.** The per-pass disc of the capacity model gains the via's height per side, `Disc` = [d0, d1] + h (0 = today).
- `CapModel::apply` writes h from the via point it already computes (`vpts`). A changed height reprices the pass
  like a changed disc (`layers_changed`, the build memo key, the undo log).
- The drawing's packing passes h = 0 and is unchanged.
- Option `[capacity] via_disc_height` (default off until the A/B).

**Tests.**
- Unit: `pair_req` at h = 0 equals today's value. It falls continuously to the track-only term, and exactly at
  sqrt(R^2 - h^2). `margin_all` at an offset against a straight side (unchanged) and against a circle (smaller).
- Integration: stored = fresh prices (`viacheck`), and local judge focus exact on the qmtech crop.
- Off = main copper on 4 boards. TOPO_LOCALCHECK 0. Mutants per changed line.
- Probe: via_gap_best_point (not in the suite).
- **Acceptance** (lead): equal time for b / r / br + disc, 300 s, esp s0-4 + qmtech s0. A default flips only if
  all of these hold: IO6 is removed, no connection (ENABLE included) gets a new det > 1.5, and KiCad new does not
  rise.

## (1) T1 part 2: the pad exit as a plan object
Per wire end at a pad with another net's copper closer than w + 2s (the 5.10 fine-pitch test, `board::exit_stub`'s
gate, without its stub), the plan holds `Exit { wire, end, pad, side, at, dir, w_e, reach }`, read from the plan:
the leaving pass = the first pass whose next triangle is outside the pad (`tris[k+1]`, mirrored at end b), `at` its
pass point, `side` the pad side its edge lies on against the pad axis (second moments, as `exit_stub` / octi's
`Exit`: tip +/-, long side +/-), `w_e` the widest width in [floor, min(w, pad width)] whose stub keeps w_e/2 + s from
the neighbour copper (static, cached per (pad, side, w)), `reach` = octi's (pad extent + s + w_e/2). Never from the
straight line to the target (D1). Priced as one more CapModel entry per (wire, end) next to CM1's via entries (same
undo log, overlays, local-judge focus, reprice on the passes of its neighbourhood):
- a) static: the stub at `w_e` (at worst the floor) closer than w_e/2 + s to a neighbour: over = the deficit;
- b) crossers: other nets' chords between the stub and a neighbour pin (the stub as a segment feature with CM1's gap
  cuts: items = crossers + the stub at w_e, `demand::need`, own-pad wall);
- c) a planned via of the wire inside the exit zone (its disc closer than r + s to a neighbour pin): over = deficit
  (the qmtech via-at-pin half);
each over-full term 1 + over/pitch (counted in `violations()`, the measure, the archive). The exit neck gets
`pass_d` = d^2, d = (w - w_e)/(w - floor) (priced only with `[factors] neck_depth`, default stays 0; re-measured on
top as the round-1 decision asks). Compass: located `pad_exit` items (predicted) at the stub, `pad_exit_items` filled.
Option `[capacity] pad_exits` (default off until the equal-time A/B). 5.10 (`pad_exit_stubs`) unchanged.

## (3) Graded pour-cut term
`MeasureParts.pour = (count, graded)`: the plan's copper (per wire its layer stretches at their pass widths, planned
vias, plane stubs; `draw::track_item` / `via_item`) through `pourcheck::check_pour_reach` at a plan resolution
(cell budget a parameter, e.g. 1 M cells; static fill rasters cached per run). Each cut-off component holding a
pour-served pad (KiCad: one missing connection) = 1 count, graded 1 + its pads beyond the first x 0.25 (a ring round
three GND pads costs more than one pad, smoothly). Costly, not forbidden; weight = the violation weight x
`[factors] pour_cut` (default 1 after the A/B). Whole-board measure: every call. Local judge: the term again only when
the move's free wires have a stretch on a layer of another net's pour inside its fill box (else the old value), so a
move pays its own cut. Cost to be measured first (one full check today ~50-100 ms on esp; at 94 local judgements per
cycle that could double a cycle: if so, a per-window check in the local judge and the full one per cycle, said so).
Option `[capacity] pour_cuts` (default off until the A/B).

## (4) Doc lines on the test hooks
`refine::viacheck::FOCUS_ON` and `optimise::neckcheck::LAST`: process-global atomics; tests that set or read them run
in their own test binary (or serialised), never beside another test of the same binary that routes with via cuts.

## Patterns (PNG + text for approval, not in the suite before)
- `via_gap_best_point` (2): a SIG pin north of a gap between two other-net pads whose width grows from need - 0.02
  (at the triangle's first site point) to need + 0.05 within 0.4 mm; two crossers through the gap on F.Cu; SIG's
  target on B.Cu straight south. Expected: the via in the gap where it fits, the straight path, 0 predicted.
- `pour_ring` (3): GND pour on F.Cu with three GND pads (pour-served); a VCC tree of three pads that can join either
  round the GND pads (shorter by ~1 mm, encloses them) or past them on the open side. Expected: the open side (the ring
  cuts a pour terminal off), GND pads stay in the pour.

## Order, tests, acceptance
(2) -> (3) -> (1) (smallest first, (1) the largest). Each: off = main copper (hash, 4 boards); thread independence;
TOPO_LOCALCHECK 0 mismatches on fixtures + boards for (1) and (3); mutants per changed line; IO6 per connection;
equal time (wall 300 s, 4 threads, esp s0-s4 + qmtech s0) vs main de9e3977, judged per connection (ladder_diff
--json), no monotone gate; defaults only after the A/B wins. Budget <= EUR 4.
