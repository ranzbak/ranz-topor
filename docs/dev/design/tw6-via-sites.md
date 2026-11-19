# tw6-via-sites: "no via site" as a graded price, site and branch moves in the loop, room for crossers between via pairs

Branch `tw6-via-sites` off main 706cf43c. Plan: ~/.claude/plans/topor-way-plan.md, row TW6 (= VS-PLAN, P6 open part,
VS-CLASS-DETOUR) and the lead's line of 2026-11-18 on the dangling lead's via end. **Design note only. No engine code
before the lead's reply, and none before tw4-r2 merges**, because both change the same pricing code (list below).
Baseline job: tw6b-479cc5a (rev 479cc5a8 = main's engine + job script + pattern boards).

## Baseline today (no new long-board runs)
Measure: a connection is "via-site forced" when its first failed check is `via k of n: no site in its interval`.
That means the drawing had to place a via with no legal site. Counts come from result.json; the parser is
scratch/agent-wt/reports/tw6-via-sites-data/vsite.py.
- **qmtech s0, main de9e3977** (the default copper, unchanged by TW5), run tw5m3-de9e397/Mr1, rounds=1:
  - 6 via-site-forced connections: ADV_DDCSCL (18 violations), ADV_DVDD, ADV_SCL, MAX_SCLK, Net-(U7-PVSS), USB_VBUS.
  - 8 vias placed by force.
  - The plan hands the drawing 15 vias without a site (14 connections).
  - Compass via_site: predicted 15, drawn 16, KiCad 70.
  - The plan's original 5 (b90ec1a, force off) were ADV_SCL, UART1_CTS/RTS, MAX_SCLK, ADV_DDCSDA. Today CTS and RTS
    are no longer site-forced, but each still has 1 via where the hand has 0. CTS has 8 violations and a detour of 1.77;
    ADV_SCL has 1 via and a detour of 1.67 (hand: 0 vias, F.Cu).
  - 4 of the 6 put their via next to a 0.5 mm pitch pin (U7, ADV7511). This is the same quantity as tw4-r2 (1c).
- **bornhack s0, indicative only**:
  - Source: run nd2f-a127d51, 4 threads, default options. That rev is 148 commits behind main (before TW3-TW5).
    It was the last bornhack run at defaults.
  - 37 via-site-forced connections (the old count of 42 was with force off). 29 of them end at U6.
  - 118 vias placed by force, 7 outside the plan's class. The plan predicts 87 vias without a site (62 connections).
- **Patterns** (tw6b-479cc5a, rounds=1 and defaults give the same result):
  - `interleaved_boundary`: 0 DRC, 1 via. Green.
  - `planned_crossing_window`: 4 KiCad clearance errors, SDA against VCC's vias at (12.58, 7.25).
    - The plan predicts 0, and its VCC via sites are 1.67 mm apart.
    - `viaopt::relocate_vias` moves the two vias to 1.36 mm apart, but SDA between them needs 1.45.
    - The drawing finds both planned sites not free and places the vias with its own site search.
    - PNG: hz-runs/tw6b-479cc5a/D/planned_crossing_window/planned_crossing_window/s0/planned_crossing_window-compare.png.
  - These six patterns are green: via_force_relocate, via_pair_slide, via_pair_redundant, via_size_rule,
    branch_point_relocate, branch_point_into_pad.

**Why the loop does not remove them.** A site-less via already counts as one violation in the measure
(`CapModel::violations`: `nosite` adds 1, binary). But no move offers a way out:
- The layering prices every via the same (`layers::assign_two`: `g.add_edge(.., ±via_mm)`), so it cannot move a via to
  a stretch that has a site.
- The re-lay search does not see room (`init::arc_chords`: a flat `cross_mm`), so it lays the same path again. This is
  the same mechanism as tw4-r2's pour finding.
- `CapModel::optimise` moves a via only inside its own interval.
- The two site tests disagree:
  - `layers::wire_stretches` tests the centroid and pass points against `via_site_r`.
  - `Packer::via_site` tests the 9 points of `tri_site_points` at each net's own via and clearance.

## Literature (checked against the PDFs)
- TopoR 6.3 manual:
  - p. 179, Shift Vias (F7), three submodes: (1) vias move without rerouting; (2) wires can be rerouted as well;
    (3) components move too.
  - p. 176: "vias and T-points are moved to their calculated optimal positions".
  - p. 183: "Most violations are eliminated by procedures for automatic via shifting ... and wire path optimization".
  - In TopoR these are tools that run after routing. Only submode 2 changes the topology, so only that part becomes a
    loop move. Submode 1 stays our metric stage (`relocate_vias`, `relocate_junctions`).
- Dayan & Dai 1993:
  - p. 10: the best assignment of one two-terminal net.
  - p. 12: the search graph: vertical edges are vias at a point, horizontal edges are branches priced by wireContrib.
  - p. 13, Table 1: the complete graph (layer, point, direction). A "via cost" per point plus the cost of the no-via
    branch on one layer.
  - Pricing a via per candidate point, with the stay-on-the-layer path priced next to it, is exactly this graph.
- Polubasov 2001 p. 4: a stretch between two adjacent crossings needs a via if its ends lie on different layers. In
  the domain graph the edge weighs +1 or -1. A weighted edge (via price plus site price) is the same model with
  real weights, which our `DomainGraph` already supports.
- Polubasov 2010 «Перекладка проводников», fig. 4: "a penalty for crossing a stretch of a wire that carries a via".
  Wire 1 takes a longer path, so wire 2 passes without vias.
- Luzin, Popov & Popov 2013:
  - p. 133: S = Σ kᵢxᵢ.
  - p. 134, steps 1-11 and fig. 1: step 8.1 leads the wires crossing the v-u wire round u, which is our `pair_move`.
  - pp. 134-135, figs. 2-3: vias that are not adjacent are reduced to the adjacent case.
  - p. 135: "afterwards the via positions are corrected by a force algorithm (force size and direction depend on the
    wires near the vias)". The wires that cross between a pair are among those wires.
- None of these sources prices via room explicitly. Making the room graded is our rule (graded factors, costly not
  failed).
- History: VS-PLAN pin_sites (2026-11-14) added dog-bone vertices to the CDT, and qmtech's via_site went from 17/2/5
  to 26/31/78. TW6 adds no vertices.

## Model (A-E; each part has its own option; off = main bit-identical)
**A. One room measure, graded.**
- `room(t, net)` = the largest `cutflow::point_free` over the 9 points of `tri_site_points`. It is `Packer::via_site`'s
  existing free cache, now exposed as `Packer::site_room`.
- `Packer::via_site` becomes `site_room >= r`. tw4-r2's `via_cands` keeps the same points and the same cache.
- Deficit of a via interval: d = r − (best room over its allowed traversals, its end site, and its lead via end).
  - r is the net class via radius plus clearance (power via rules: the net class via by default).
  - The best room is read at the point `via_point` already returns when no point has room (its freest point).
  - Inside an obstacle the deficit is capped at the via diameter.
- Price of a via with d > 0: count 1 (the predicted violation, as today), graded amount 1 + d / (via_d + s), at the
  violation weight. This is the same form as CM1's 1 + over/pitch, and there is no new threshold.
  - The step at d = 0 is the violation itself, as in CM1.
  - The slack above 0 is already priced by `via_squeeze`, which is graded.
- **One measure for plan and drawing (lead 2026-11-19, from tw4-r2 (2b)).** tw4-r2's `via_disc_height` (the via disc
  on an edge at the via's height h) lost its A/B because the drawing still packs with h = 0. The plan's predicted errors
  fell 197 to 74-83, but drawn drawing_loss rose 198 to 229-255 and drawn pad_exit rose 25 to 41-52 (tw4-r2.md "(2b)
  result"). So `site_room` and the via disc at its height are one function, read by:
  - the plan: `Frame::apply` (vpts, discs at h), `CapModel::nosite` / the graded amount, and `layers::wire_stretches`;
  - the drawing: `Drawer::set_wire_layers` (draw.rs:1287, `wire_discs` at the drawn via point's height, not h = 0),
    `Drawer::relayer` (draw.rs:3032), and the site tests in `Drawer::place_vias` (draw.rs:1799, the planned site's
    room) and `Drawer::via_candidates` (draw.rs:1518, ranked by the same room). The height is read at the via point
    the drawing puts (planned site, or the searched one), so the packed lanes match the drawn via.
  - tw4-r2's (1c) reads it too (one count, see below).
- Test: plan vs drawing agree per via. For every drawn via, the room the plan read at its planned point equals the room
  the drawing read at that point (a `viacheck`-style stored = fresh check, unit test on planned_crossing_window and
  site_or_stay). Board check: drawing_loss and drawn pad_exit do not rise when `via_disc_height` and `site_price` are
  on together (the tw4-r2 (2b) nets: IO6, ENABLE, IO7, IO5, IO10_SDA on esp; ADV_DVDD, JS1_1, Net-(U7-PVSS) on qmtech
  are re-measured by name).

**A + C as implemented (round 2, on main 12699012 = tw4-r2 merged; `[vias] site_price`, default "off").**
- `Packer::site_room(t, net, s)` = the cached max free over `tri_site_points` (`via_site` = room >= r, unchanged);
  `Packer::site_deficit(room, d)` = min(d, max(0, d/2 - room)).
- Plan: `Frame::site_over` adds Σ deficit / (d + s) at each planned via point (`cutflow::point_free` there, the
  same measure) to the wire's CM1 entry (`Frame::wire_over`), so the undo log, lane overlays, `viacheck` and the
  local judge's focus cover it; the count stays `nosite`'s. `Frame::rank` orders site-less traversals by deficit (the
  optimise key has it inside its price already).
- Pad exits: term (c) is off when the site price is on (one count; the site price is the same room at the same point).
- Drawing: `Drawer::set_wire_layers` packs the via discs at the height of their via point (planned site before the
  vias are placed, drawn point after) whenever `[capacity] via_disc_height` is on; `Drawer::relayer` keeps a kept
  via's height. This is the plan/draw agreement of tw4-r2 (2b) and applies to `via_disc_height` alone too.
- C: `Packer::lead_ends` (set in `route_round` with the option) and `Frame::lead_via`: a layer change at a lead end
  (the drawing's `lead_via_site` rule) takes the lead's via end as its point and has a site.
- Not done in A: the drawing's `via_candidates` ranking stays the drawing's own (its checker is KiCad's rule).
- Tests: engine/tests/topo_site_price.rs (opt-in, graded and capped deficit, stored = fresh and focus exact with
  site_price + via_disc_height + pad_exits on the qmtech USB crop, off = default copper).

**B. The no-via alternative priced against it.**
- Layering: each domain-graph edge weighs `via_mm + viol_mm·amount(d_edge)`.
  - d_edge is static per stretch, cached in `StretchCache` and keyed by the option.
  - The exact max-cut then puts the via on a stretch with a site, hands it to a crosser that has room, or keeps both
    wires on one layer. This is Dayan-Dai's graph with a cost per point.
  - It applies to N layers too (`assign_n` `Costs`).
- Search: a crossing of the stretch (o, k) costs `cross_mm·f + site_mm·min(D_own(t), D_o(o,k))`.
  - D_own(t) is the graded room deficit of the net's own via near triangle t.
  - D_o is the deficit of o's present via interval through k (the Polubasov fig. 4 factor), stored per wire and pass
    like `cross_k` and refreshed in `set_lr`.
  - The path round (0 vias, longer) then competes with the crossing in S itself.
  - New factor `via_site_social` (weight `site_mm`). It oscillates per wire, like all factors.

**C. Lead via end (exist-term r4).**
- An interval that begins or ends at a lead terminal also offers `KeptEndOut::via_end`. Its room is exact
  (`lead_len`).
- If there is no via end, the interval's static room is used as in A.
- The point becomes CM1's via point (`vpts`) for that via, the same point the drawing's `lead_via_site` takes.

**D. Room for planned crossers between a via pair (P6 open part).**
- `viaopt::gap_at` gets one more maximin term for each via whose wire has an adjacent via with crossers between the
  two: dist(v1, v2) − need(radii, clearances, crossers' widths). It uses the same items as CM1's via-to-via cut
  (`viacut::pair_over`).
- So relocation never moves the pair closer than the crossers need (planned_crossing_window: 1.67 → 1.36, not
  allowed below 1.45).
- First step of round 1 (diagnosis, pattern only): `TOPO_VIAREL` plus a drawing dump, to show why the drawing finds
  the planned sites "not free". The two candidate causes are the relocated point itself, or SDA's planned room.
  Only the cause the dump shows gets fixed.
- **Dump result (round 1, tag tw6r1-c1d6afd, K and D identical).** The diver is VCC (POWER, 0.8 mm via). SDA's taut
  crossing on VCC is at arc 10.56 mm (x ≈ 11.56), 0.24 mm left of the B.Cu window (x 11.8 .. 14.2).
  - Via 0: interval [3.375, 10.56] lies wholly left of the window. Relocation finds no legal point (gap −0.600 before
    and after) and still moves it 7.967 → 6.403. The drawing refuses it (the planned point is in the rule area, the site
    is marked tight). The search then forces it out of the plan's class at (12.28, 7.90), with room −0.308 mm.
  - Via 1: relocated to (13.375, 7.0), gap 0.225, legal alone. The drawing refuses it on `via_pair_price` = 0.175 mm
    against the forced via 0 (SDA needs room between the pair). It is drawn at (13.30, 7.0).
  - The 4 KiCad clearance errors are SDA × VCC's forced via at (12.58, 7.25) (drc_digest).
  - Cause: the plan hands over a via interval with no site (A/B), not the relocation of a legal pair. Part D's
    quantity (`via_pair_price`) is only the second refusal, caused by the first via being forced. Fix order unchanged:
    A (graded deficit of via 0's interval) + B (layering: the deficit-weighted edge lets SDA dive, 0.6 via, or moves
    the crossing into the window). D keeps the pair's room once both vias have sites. Side finding: relocation moves a
    via with zero gain (gap −0.6 → −0.6); D keeps a tight via at its plan point (no move without gain).

**D as implemented (round 2; `[vias] pair_room`, default false).** `ViaCtx::pair_room`; `relocate_vias` collects
per wire the crossers' arcs on the track (`track_crossing`) and, per consecutive via pair, the crossers between the
first via's interval end and the second's start (Σ width + max clearance); `gap_at` then needs `rv + rv' + c + Σ`
between the pair. A via whose site predicts a violation (rule gap < 0) moves only for a larger force gap (the side
finding of the round-1 dump: 7.967 → 6.403 at −0.6 → −0.6).

**B layering as implemented (round 2; under `[vias] site_price = "graded"`, `LayerParams::site_price`).**
`layers::site_amount` (best `Packer::site_room` over the traversals strictly between the two events, else the end
traversals; amount 1 + deficit / (d + s), 0 with a site). `assign_two`: each domain-graph edge weighs
`via_mm + cap_mm x amount`. `assign_n`: `Costs::site` per event and `Costs::trans_at` in the greedy colouring,
`n_cost` and the expansion edges. The refinement's own trans stays (its CM1 entry carries the graded site price).
The search term (`via_site_social`) and the `wire_stretches` count are not done yet (HANDOFF.md).

**B search as implemented (round 3; `[factors] via_site_social`, default 0).** New factor
`factors/via_site_social.rs` (`Event::ViaSite`, registered last so the earlier factors' draws stay), weight
`InitWeights::site_mm`. `init::arc_chords`: each different-net crossing adds `site_mm x min(A_own(t), A_o(o, k))`,
A = `Packer::site_amount(room, d, s)` = 1 + deficit / (d + s) without a site, 0 with one (the same function the
layering's `site_amount` now calls). `A_own(t)`: the connection's own room in the crossing's triangle
(`State::site_own`, filled with the triangle's chords). `A_o(o, k)`: `State::site_k[o][k]`, from the loop's layering
(`State::mark_site_k` in `set_lr` and at the loop start; cleared after the loop): inside a via interval
(`refine::via_ranges`, strict with between_crossings) the best room over the interval, elsewhere the traversal's own
room (a crossing there would put the via there; the note's "present via interval" generalised to wires without one).
The state holds `pack::SiteRooms` (the packer's room cache shared by `Arc`, its via sizes), set only when the weight
is > 0, so P2 and the default are untouched (weight 0 adds 0.0: bit-equal). Tests (topo_site_social.rs): a crossing
in a no-via keepout moves to where a via fits (still one crossing, the path round costs more); 0 where the crossed
wire's via has a site (the min); the interval amount; opt-in.

**E as implemented (round 3; `[optimize] site_move`, `branch_move`, default false).** After `pair_move` in every
cycle, per wire present at the cycle's start, its site-less via interval with the largest amount
(`viaopt::siteless_intervals`, the packer's static room over the interval, part A's measure; the plan's taken sites
and end sites are left to the evaluation): (i) `site_move`: the wire's single reroute (its own judge, B's search term
when on); not kept: (ii) the wires whose crossings are the interval's two events (`viaopt::bounding_wires`, the
layering's partners) laid again together (`Sp::relay_kept`, the body of `pair_move` without the held stretch: the
crossers see the interval's two layers already); `branch_move`: the branch arms of the junctions inside the
interval (`viaopt::tee_arms`: a branch on a turn of the wire there, or the wire itself at its junction end) laid again
one at a time (the tree search picks a new junction). Every relay is kept when the board's evaluation drops (its
predicted violations count a via without a site; with site_price its graded deficit too, so no extra price is
added: one count). Counted inside the cycle's `max_ripped`. Report: `loop.site_move` / `branch_move` [kept, tried]
and one warning line (`core::site_move_line`). `pair_move` is left as it was (no refactor, bit-equal by
construction).

**E round 4 (lead decisions 2026-11-19).**
- *Graded detour price in the keep judgement* (`[optimize] site_move_detour`, excess-ratio scale, default 0.5, 0 =
  round 3's evaluation alone). Per relaid connection `c`: `D(c) = viol x ((len_c / straight_c - 1) / scale)^2`,
  `len_c` the length of its alive wires, `straight_c` pad to pad (the run report's detour ratio;
  `escape::detour_excess` is the badness's detour term, `escape::detour_price` has the shape of the via-pair stretch
  price `viaopt::pair_len_price`: a scale, not a cutoff). `viol` = the judge's violation weight. Kept when
  `e1 + ΣD_after < e0 + ΣD_before` (`core::site_keep`; (ii) and the branch moves), and in (i) the reroute's own
  judge through `Rerouter::judge_extra` on both sides (`Sp::reroute_d`, every other reroute passes 0: bit-equal).
  Symmetric by design: a move that shortens a detour gains by as much (the crop test: at scale 0.001 the site moves
  kept rise 0 -> 3 of 3-7). Moves the evaluation alone would have kept and the price refused are counted
  (`loop.site_move_detour_refused`, the report line). Why per connection: the det > 1.5 metric is per connection,
  and a branch's straight to its junction is no detour reference (Luzin, Popov & Popov 2013 p. 134 step 8.1 keep a
  relay by the board's quality; the length term there is the plain length, the graded ratio is ours: the user's
  "no detours where a direct path exists").
- *Branch moves bounded by the tee event.* The layering's tee rule (Polubasov 2001 §5 p. 9) puts an event at every
  junction (trunk traversal k, fraction 0.5), so with strict intervals (between_crossings) a junction never lies
  inside the via interval its tee via sits in: round 3's `tee_arms(lo..=hi)` could never match a trunk junction (0 of
  0 branch moves on esp and qmtech). `tee_arms` now takes the interval's two events and matches a junction whose tee
  event lies from the first to the last, inclusive (test `a_tee_event_bounds_the_strict_interval_but_lies_outside_it`).
- *M13* (`set_lr` without the B term's `mark_site_k`): no fixture showed a copper change (the qmtech USB crop x 6
  option sets, the keepout board x 8, live vs loop-start site_k). The refresh is made structural instead:
  `State::apply_layering` sets layers_now, the free wires and site_k from the same events, called by `set_lr`.
- `[vias] site_price = "graded"` implies `[capacity] via_disc_height` (its new "auto", unless the user sets it), so the
  plan prices the via's room at its real point and the drawing packs the discs at the same height (r3: s without h
  doubled qmtech's unconnected).
- `planned_crossing_window` goes to CROSS-DRAW (F2/F3); the B search term stays parked (opt-in, weight 0).

**E. Loop moves (submode 2).**
- `site_move`: per via with d > 0, after `pair_move` and at the end-of-cycle via weight:
  - (i) re-lay its own wire, with B's search terms;
  - (ii) re-lay the crossers that bound its interval, with the stretch held (as `pair_move` does, Luzin-Popov step
    8.1), so that the interval grows to a site.
  - A move is kept by its price (the evaluation plus the graded amount), like `pair_move`. The table protects the
    bests. There is no worse-gate on the board.
- `branch_move`: re-lay the branch arm of a junction whose tee via has d > 0 (`tree::dependents`, `search_tree`). The
  junction's topology then moves.
- The metric `relocate_junctions` and `relocate_vias` stay after the loop (submode 1).

## Functions touched, and the overlap with tw4-r2
| file | TW6 | tw4-r2 touches the same |
|---|---|---|
| pack.rs | `Packer::site_room` (new, from the `via_site` cache), `via_site` | `via_cands`, `via_point_cached` (same 9 points, same cache key): **shared** |
| refine.rs | `Frame::rank`, `Frame::apply` (nosite becomes a graded amount; lead via end in `vpts`), `Frame::end_site_raw`, `default_ks`, `CapModel::{violations, overfull, nosite, nosite_wires, nosite_debug}`, `Arrays` / `MemRead` / `Mem` (`nosite`, `set_nosite` get f64) | `Frame::apply` (vpts, `wire_heights_into`), `Changed`, `reprice_vias`, viacheck: **same functions; rebase after merge** |
| refine_par.rs | overlay `wns` gets the graded amount | one line |
| viacut.rs | read only (`pair_over` for D) | `pair_over`, `over` signatures: **after merge** |
| layers.rs | `wire_stretches` (site through `site_room`), `assign_two` edge weights, `assign_n` `Costs`, `StretchKey`, `LayerParams` (+ site flag, viol_mm) | none |
| init.rs, state.rs | `InitWeights.site_mm`, `fill_chords` / `arc_chords`; `State.site_k` (like `cross_k`) | none |
| factors/ | `via_site_social.rs` (new), `Event::ViaSite` | none |
| viaopt.rs | `relocate_vias`, `gap_at` (D) | none |
| core.rs | `Sp::{site_move, branch_move, set_lr}`, candidate list beside `obstructed_pairs`, `route_round` option plumbing, warning lines | `route_round` plumbing (text merge only) |
| draw.rs | `Drawer::set_wire_layers`, `Drawer::relayer` (via discs at the drawn via point's height), `Drawer::place_vias` / `via_candidates` (read `site_room`), `lead_via_site` (C) | tw4-r2 (1) pad exit plan object and `Disc` = [d0, d1, h]: **after merge** |
| compass.rs | via_site items carry the amount | none |
| model.rs, schema.py, configuration.md | options | own options (text merge) |

**Decision for the lead:** tw4-r2 (1c) ("a planned via inside the exit zone, its disc closer than r + s to a neighbour
pin") prices the same quantity as A's deficit against pins. It should be counted once. Proposal: (1c) is dropped from
tw4-r2, or reads `site_room`. A via short of room then pays the site price and not also pad_exit (c).
**Lead decision (2026-11-19):** tw4-r2 lands first and keeps (1c); TW6 part A builds `site_room` so that (1c) reads it
(one function, one count). TW6 does not edit tw4-r2's branch. Part D also waits for the merge: `git diff
main...tw4-r2` touches core.rs, model.rs, schema.py, configuration.md and viacut.rs, which D needs (viaopt.rs itself is
untouched). Order after the merge: A+C, D, B (search term), E; all opt-in until the equal-time A/B.

## Options (judgement calls; defaults are set only after the A/B)
| option | part | default now | meaning |
|---|---|---|---|
| `[vias] site_price` = "off" / "graded" | A, B layering, C | off | one room measure, graded amount, weighted domain graph, lead via end |
| `[factors] via_site_social` (mm) | B search | 0 | price of a crossing in a room-less place, times min(own, crosser's) deficit |
| `[vias] pair_room` | D | off | relocation keeps the crossers' need between a via pair |
| `[optimize] site_move`, `branch_move` | E | off | loop moves (prof-counted, inside the work-unit throttle like `loop_p4`) |
| `[optimize] site_move_detour` (excess ratio) | E r4 | 0.5 | graded detour price in E's keep judgement; 0 = none |
| `[capacity] via_disc_height` = auto/on/off | A r4 | auto | auto = on with site_price graded |

## Tests
- Unit tests:
  - `site_room >= r` ⇔ `via_site`.
  - The amount is continuous in d above 0.
  - Layering with the option off: events bit-equal.
  - The weighted domain graph against `layers::brute_force` (fuzz with random weights).
  - D's term against a brute-force maximin on the pattern geometry.
  - The search term is 0 when both sides have room.
- Exactness: viacheck stored = fresh for the graded nosite amount; `TOPO_LOCALCHECK` 0 mismatches on fixtures and
  boards; copper independent of the thread count.
- Off = main copper hash on eprom, wiccon, esp and qmtech. Mutants for every changed line.
- Patterns: the 8 above. Proposed new patterns (PNG and text before the suite, as round 1's first item):
  - `site_or_stay`: a 0-via path round a fine-pitch pin row against a crossing whose via has no room. Expected:
    0 vias. The geometry comes from ADV_SCL.
  - `lead_via_end`: a dangling lead with no via room at its end. Expected: the run stays on the lead's layer, or the
    via goes to the lead's via end.
  - Round 1 (harness/tw6_patterns.py, PROPOSED, tag tw6r1-c1d6afd): both reference boards have KiCad DRC 0. **main
    already solves both under K and D** (site_or_stay: 0 vias, DRC 0; lead_via_end: 1 via at the via end (8.982, 6),
    DRC 0). They are acceptance guards (no regression), not failure reproducers. A reproducer of the qmtech class needs
    the crossing to be cheaper than the detour in today's S (a longer detour, or congestion round the row); the user
    decides whether to keep them, or to grow site_or_stay until main fails.
- Boards:
  - qmtech rounds=1, judged per connection (the 6 plus CTS/RTS).
  - Equal time as in tw4-r2: wall 300 s, 4 threads, esp s0-4 and qmtech s0.
  - bornhack once, at the end of the stage, with 4 threads; U6 reported separately.

## Acceptance (plan row TW6)
- qmtech via-site forced 6 → 0. ADV_SCL and UART1_CTS/RTS are drawn with 0 vias, like the hand.
- bornhack via-site-forced connections 37 (old count 42) → ≤ 10.
- `planned_crossing_window` 4 → 0 DRC. `interleaved_boundary` stays 0 DRC with 1 via.
- The other 6 via patterns stay green.
- Every changed connection is judged in context, with no monotone gate. A default flips only if there is no new
  det > 1.5 and KiCad new does not rise in the equal-time A/B.

Order after tw4-r2 merges: A (with C) → D → B search → E. That is smallest first, and E needs B's search terms.
Budget: ≤ €3. This round used about €0.03.
