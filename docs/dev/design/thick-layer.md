# thick-layer: one priced search for wide traces, stacking and via windows (DESIGN, PROPOSED)

Branch thick-layer, 2026-11-23. Status: design only, nothing below is coded. Part 1 (F4, the plan of the drawn row in
`plan.json.gz`) is done on this branch and is the measuring tool for every stage here.

## Problem

qmtech VIN5V F3.2-F1.2 (1.0 mm, via 0.9/0.5) was planned with 4 vias: its crossers alternate F F F F | B | F F | B B B
along it. Via 4 had no real site, so the drawing put it against U404 and ran B.Cu over three SD signals. The plan
predicted 0 errors (reports/alt-route-probe-20261122-2225.md, R1-R3). The pattern `thick_dive_grouping` (PROPOSED)
reproduces this on main with 4 shorts and 0 predicted. The user (2026-11-22) gave these directions:

- Wide traces have higher priority: they go first, and moving them or changing their layer costs more.
- They are not locked. They may be shoved sideways or change layer when the whole board gains.
- The planner should not stack F and B traces in the same place unless they form a solid block. This always holds on
  2 signal layers.
- Bundles make the free space navigable.

The two outcomes the user named are both to come out of one priced search, not from a rule:

- (a) the thin crossers are grouped by layer and the wide trace weaves once per group;
- (b) the wide trace runs on one side and the signals on the other.

## Model (one cost, graded, no cutoff, no net/board case)

**M1. Priority weight ω(w) of a wire's own layer changes (F3).**

- Formula: ω(w) = 1 + a_w · max(0, w / w_def − 1) + a_t · (3 − tier). w_def is the board's Default netclass width.
  tier is the existing route-order tier (1..3: user priority, else 2 for wider than default, else 3;
  `decompose.rs:59`). ω ≥ 1 and grows smoothly.
- Where it applies: ω multiplies only the price of that wire's own vias. These places see it:
  - the P3 domain-graph via edges, `layers.rs:1407` (m = via_mm · ω(w) + site price; the edges are already per wire);
  - the N-layer transitions `Costs::trans_at`, through event → wire;
  - the refinement `wire_cost` (`refine.rs:2332`, the trans term × ω(w));
  - the TopoR loop's whole-board evaluation: the via term becomes Σ ω(w) · vias(w) at mm_per_via.
- What stays the same: the table's `vias` criterion stays a plain count (TopoR 6.3 manual p. 73: "Number of vias"),
  so the four-bests roles and the report do not change meaning. A crosser's via keeps ω = 1.
- Why the outcomes follow:
  - At every crossing, the global layering (Polubasov 2001; Polubasov 2010 «Оптимизация расслоения»: the layering is
    global and exact for 2 layers) now trades 2ω for one dive of the wide trace against 2 per thin crosser that dives
    instead.
  - With one crosser, the crosser dives, which gives (b).
  - With k ≥ ω same-layer crossers grouped, one wide dive is cheaper, which gives (a).
  - No layer is fixed.
- Moving a wide trace sideways is already dearer: the `length` factor is scaled by width + clearance (schema
  `[factors] weight` doc). No new term is needed. Its reroute, float-up and layer re-optimisation stay open, each
  priced.
- Literature: the sources found price all vias the same.
  - TopoR 6.3 manual p. 73 lists the four criteria. p. 57 picks a per-net via type: "not suitable for power nets".
  - Polubasov 2010 «Многовариантная оптимизация»: the via price is the judgement call, F = F1 sinφ + F2 cosφ.
  - Dayan & Dai 1993 p. 6 use one user via cost Kv.
  - A per-wire weight is our graded extension. It fits the structure, because the domain graph's edge weights are
    already per stretch (TW6 B site price).
  - Route order stays "wider first, among them shorter first" (Polubasov 2010 «Начальная трассировка»; memory
    route-order).

**M2. Moves that reach (a) and (b) (F2).** Single reroutes cannot reach these outcomes (Luzin 2015 pp. 9-10, fig. 21c:
"cannot be achieved by rerouting individual wires"; Luzin, Popov & Popov 2013 pp. 134-135, steps 1-8 and figs 2-3).

What exists:

- 5.8 `pair_move` (`core.rs` `obstructed_pairs` / `pair_move`) re-lays a via pair's obstructing crossers with the
  stretch held.
- TW7 C `via_trade` (fig. 21b → 21c).
- TW7 F `held_relay`.

What is added:

1. **Re-attachment candidate.** In `pair_move`, the obstructing tree branches are also offered re-attached together
   (`try_attach` for all `obs` at once). This is the equal-via alternative the probe found (DATA2/DATA3 to RN on
   B.Cu). It is LPP 2013 p. 134 fig. 2: "one wire at a pad is detached and joined to another wire ... creating a
   branch point". F2 joint re-attachment is this case.
2. **Group candidate.**
   - Applies to a wire with ≥ 2 layer changes.
   - For each run of crossers between two of its dives, the run is re-laid together so that it crosses inside the
     neighbouring same-layer group. This is pair_move with T on both sides, step 8.
   - Candidates are ordered by ω(w) · vias saved. This is an order, not a threshold.
3. **One-side candidate.** The wire's whole stretch is held to its end layer and all its crossers are re-laid. This is
   (b) as a move.

All three are kept by the board's evaluation: violations first, then the objective with M1's weighted via term. This
is the same keep test as every pattern move. Otherwise everything comes back exactly.

**M3. Via sites against the planned crossers (F1; keeps predicted = drawn).**

- What changes: the site test of a planned via counts the other nets' planned lanes in the triangle on every layer, as
  well as the static room (`Packer::site_room`). A through via needs every copper layer free. Lane = taut segment ±
  (w/2 + s).
- Where the test is used:
  - `Frame::rank` and `via_no_site` (refine.rs:881-904, 932);
  - `viaopt::siteless_intervals`, `SiteRooms`, and the layering's `site_amount`.
- Measure: one measure, reused from CM1's gap cuts (`viacut.rs`: a via point fits iff no gap cut through it is
  over-full, with the crossers' traversals as items). The `site_price = "graded"` deficit is kept, so no site is a
  predicted `via_site` error, priced and not refused.
- First step, test first: on `thick_dive_grouping`, read the plan report. Find why CM1's via cuts and the site test
  both gave 0 there. The likely cause is that the cuts are tested at the via's first static site, not at the place the
  drawing is forced to.

**M4. Stacking (user 2026-11-22, both messages).**

- Definition: stacking is F and B lanes that overlap in plan view. It blocks every through via there and, on 2 signal
  layers, every crossing.
- Measure, per triangulation edge e with passes on ≥ 2 signal layers:
  - The planner already holds one order across all layers (`State::edge_order[e]`). The layer-blind need is
    need_∪(e) = the edge's passes packed as one row (`State::edge_need`).
  - If need_∪(e) ≤ len(e), stacking is avoidable. Price 0, and the drawing is told to pack layer-blind there.
  - Otherwise the forced overlap is o(e) = clamp(need_∪(e) − len(e), 0, min over layers of the per-layer need).
- Price: P_stack(e) = k_stack · o(e) · ℓ(e) · share(e).
  - ℓ(e) is the stack's length along the wires (the depth of the two triangles at e).
  - share(e) = occupied signal layers at e / signal layers. It is 1 for 2 signal layers, including 4-layer boards with
    2 planes. With more signal layers it scales with the occupied share.
  - Planes are their own net's terminals (memory planes-are-terminals). A foreign via passes a plane through its
    antipad, so planes never count as occupied.
- Graded and costly-not-failed: a forced block (the solid-block case) is priced, never cut off.
- Where it lives: a per-edge cost slot of the `CapModel` arrays, like `via_squeeze` (`Arrays::sq`). P3 refinement and
  every P4 move see it incrementally and with the same measure. It is a new factor `stacking` (social; report level
  warning when drawn stacked where need_∪ fits).
- Interactions:
  - With M1: ω does not exempt a wide trace. Its overlap width is its own width, so it pays more.
  - With M3: stacked lanes leave no planned-room site, so a planned via there gets M3's deficit. Potential vias are
    handled by M5. Nothing is counted twice.
- Literature: nothing found on stacking in the TopoR manual or the 5.0 help. Manual p. ~95 covers display of wires
  "over one another" only. Polubasov 2010 fig. 4 prices crossing a stretch that carries a via, so a wire detours to
  leave another's via its room. That is the closest TopoR precedent.

**M5. Navigability: via windows (user 2026-11-22, bundles; memory bundles-tight-packed).**

- Measure: for every planned via interval, n_win = the triangles of the interval with an M3 planned-room site.
- Price: f(n_win) = k_win / (1 + n_win), falling smoothly. With n_win = 0 the via is M3's predicted error.
  - This covers via intervals, the vias the plan needs, not every triangle of the board. Wide corridors are covered by
    the existing per-edge / cut overflow: the wide trace is laid first and its rip-up sees thin-trace fragmentation as
    overflow.
- Bundles: the layer-blind packing of M4 keeps the F band and the B band side by side in edge order where they fit.
  The gap between the bands is the via window M5 rewards.
- What exists today:
  - ERBS packing at clearance (bands tight);
  - `via_squeeze` (slack around a via in a passage);
  - planned_room / reserve planned room (drawing keeps undrawn room);
  - `reserve_sites` / `site_taken` (another net's planned via);
  - TW6 A/B graded static site price;
  - TW6 D `pair_room` (opt-in, relocation only);
  - CM1 via cuts (planned via point vs crossers).
- What is missing:
  - the site choice against planned crossers (M3);
  - windows for the vias still to be placed (M5);
  - cross-layer stacking and layer-blind packing (M4).
- Literature: Polubasov 2010 names the factor «вклинивание между участками других проводников» (wedging between other
  wires' sections), a price that keeps bundles whole. Luzin 2015 p. 3 lists "make some room for wire routing" (via
  sizes). Nothing found on spreading vs bundling in TopoR. Duncan, Efrat, Kobourov & Wenk 2001 pp. 2, 4 spread for
  maximum separation, the opposite. We bundle, per the user (memory bundles-tight-packed).

## Fit, invariants, options

- **TopoR loop.** M1, M3, M4 and M5 are terms of the one evaluation (archive, P3, P4 keep tests). M2 adds candidates to
  the existing pattern-move slot.
- **Unchanged.** Route order, never-drop and draw-once.
- **Determinism.** Copper is bit-identical for any thread or lane count.
- **Option defaults.** Each item has an option. With all options off, copper = main (checked on the 4-board det s0
  run).
- **New keys.** Each is documented with meaning, unit, default and KiCad pointer:
  - `[vias] priority_weight` (a_w, a_t; default after the det s0 4-board A/B; proposed a_w 1, a_t 0.5);
  - `[vias] site_planned` (M3, default on after A/B);
  - `[optimize] pattern_moves` gains `reattach`, `group`, `one_side`;
  - `[factors] stacking`, `via_window` weights.

## Acceptance

| what | pass |
|---|---|
| pattern `thick_dive_grouping` (PROPOSED) | expected path: P 1 via at a real site, F3/F4 cross inside the dive; KiCad DRC 0 / 0 unconnected; predicted = drawn |
| new PROPOSED `wide_one_side` | P (1.0 mm) between PTH pads left→right; S1-S5 0.2 mm with PTH pads top→bottom (free layer); S6 with SMD F pads both ends. Expected: S1-S5 on B.Cu straight, P straight on F.Cu with 0 vias, S6 dives under P with its own smallest legal via pair. Total 2 vias either way; ω decides, so it tests M1 alone |
| new PROPOSED `bus_side_by_side` | a 4-wire F bus and a 4-wire B bus share a corridor wide enough for both side by side plus one via window; a signal from an F SMD pad to a B SMD pad must change layer inside it. Expected: the bands side by side, the via in the window between them; the stacked answer is the failing one (tests M4 + M5) |
| qmtech VIN5V det s0 | ≤ 2 vias, no shorts, no U404 / SW2.3 clearance item; plan report: predicted = drawn for conn 405 |
| ladder eprom, wiccon, esp-rust-board, qmtech det s0 | completion, DRC new (drc_digest per type), detour > 1.5, vias, runtime (ccx33 slot only when timing is claimed): no worse without a per-connection reason; numbers from ladder_diff --json only |

The patterns get PNGs (setup | expected | main) for the user, are PROPOSED and stay out of the suite, and show the
optimal path with every constraint visible (memory patterns-teach-optimal-path).

## S1 diagnosis (2026-11-23, plan report on the synthetic fixture `engine/tests/topo_thick_layer.rs`)

Default options, one run: P is planned with 3 vias (B | F | B | F), every via `site: true`, deficit 0, predicted 0,
while the drawing makes 2 shorts (B2, B3) and 4 clearance items. P's last via interval lies in traversal 7, between
its crossings with F4 (event s 0.44) and B2 (s 0.65). The site (`refine::end_site`) is chosen on the pass-point
polyline (`State::polyline`), where those two crossings are about 1 mm apart, at (11.27, 6.50). On the taut lines
the drawing realises, F4 is at x 10 and B2 at x 11: the point is past B2, and the 1.0 mm gap leaves 0.5 mm, too little
for the 0.9 mm via with clearances. CM1's gap cuts start at static features, so here they run parallel to the
vertical crossers and never count them. M3 therefore measures the gap between the two crossers that bound the via
event, at their taut positions, along the wire. The graded deficit is taken against the slack the two crossers have
on their far sides, so a crosser that can bend aside does not make the via a predicted error. Target test:
`thick_dive_grouping_predicts_what_it_draws` (ignored, fails on 7513b3db).

## Stages (each test-first, its own tiny repro named after the mechanism)

- S1: M3 (F1), on thick_dive_grouping.
- S2: M1 (F3) in P3 + evaluation, on wide_one_side.
- S3: M2 (F2 re-attach, group, one-side), on qmtech VIN5V.
- S4: M4 + M5, measured first (plan report columns) and priced after A/B, on bus_side_by_side.

The night budget (≤ 2 ccx43 h) covers S1-S3 with det s0 runs on the patterns and qmtech, and one ladder at the end.
S4 is measure-only tonight unless S1-S3 finish early.

## Addendum r3 (2026-11-23): M3 slack, one-side hold, P2 crossing price of a layer-free connection

**M3 crosser slack (done, `[vias] site_slack`).** A planned crosser is not a wall where it can
still shift inside its passage. The shift is measured per pass: the pass's span on its edge
(`State::pass_ports`), cut at its planned neighbours on that edge (they stay on their taut
tracks, so the same room is not counted twice; the via's own wire is no neighbour). The lane may
lie anywhere between its taut track and that shifted position. The room is that of the position
farthest from the via, and the window ends at the crossing farthest out. The drawing does not
yet realise the shift: the via relocation and ERBS still bound and measure against the taut
tracks (test `thick_dive_grouping_predicts_what_it_draws_with_slack`, ignored).

**One-side hold (done, `[vias] pair_hold`).** The one-side move's run stays on its layer through
the layering that follows the move. Each of its events off that layer costs `pair_hold` via
prices x ω in that layering: a domain edge to the fixed node in `assign_two`, a unary price in
`assign_n`. This is a graded release price, not a lock, and the board's evaluation decides as
before.

**P2 crossing price of a layer-free connection (`[search] free_crossing`).**

The problem: P2 prices every different-net crossing at a flat `crossing` price (20 mm,
`init.rs` `cross_mm`). That price stands for the via that a crossing generally costs one of the
two wires. A connection whose two terminals both allow every layer (PTH, `Obstacle::drilled`)
pays no via when it crosses: it takes the layer the other wire does not use. Today it still pays
20 mm per crossing, so S1/S2 of `wide_one_side` go round P's pad to save a crossing that costs
nothing. The result is 91.41 mm against the expected 86.00 mm.

The rule: the crossing price of the searching connection is multiplied by `free_crossing`
(0..1) when both of the following hold:
- the connection is layer-free (both terminal pads drilled);
- the crossing leaves it a free layer (`cross_pin` = `Free`: its pinned layers along the path so
  far still allow a layer the crossed stretch is not held to).

A crossing that pins it (`Via`, `Unlayerable`) keeps the full price plus the forced-layer price,
as today. So the price is graded by the path's own pin state, which the search already carries
(`pin` in `arc_chords`). There is no cutoff. It applies to the connection's crossings with any
width, not only with a wide trace: a layer-free connection's crossing costs no via in either
case.

Risk: a layer-free connection crossing many undecided wires (held to every layer) stays `Free`
at each crossing, but the layering may need a via for it later. That case is graded by the
existing forced-layer price only once the crossed stretches are held, which happens in the
TopoR loop's reroutes (`layers_now`).

The default comes from the det s0 A/B per connection. The target test is `wide_one_side`:
86.00 mm with no detour, P 0 vias, S6 2 vias.

Literature: Polubasov 2010 («Оптимизация разводки»): stretches are assigned layers before
rerouting, and a crossing is priced by the via it implies. Dayan 1997 p. 55: a crossing of two
nets that can take different layers needs no via.
