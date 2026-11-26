# draw-absorb: a drawing that absorbs small plan faults

Design note only (branch `draw-absorb-note` off main 6fbe3550). No engine code. User request 2026-11-26: "Drawing that
absorbs faults: investigate into that, build plan." TopoR's metric model recomputes the shapes of the neighbouring wires
on every change, so small plan errors disappear. Our drawing commits octilinear copper wire by wire. A wire that no longer
fits is then forced last, against neighbours that are already final.

## 1. Evidence: where the DRC items of main come from

**Data.** Existing runs only, so EUR 0 was spent. The runs are `hz-runs/sh10b/M-{qm,esp,wic}` (main 6fbe3550, det s0,
`optimize.rounds=1 runs=1`, main defaults, score-honest r7 base). They are the same numbers as the cached det runs of
142414c and e14c710 (qmtech 225, esp 17).

**Method.** `tools/draw_absorb_classify.py` (on this branch) is read-only.
- It joins every new routed-copper KiCad item to the nearest drawn engine violation. The join uses the same rule as
  `toporlib/compass.py:build`: a named net and layer, within 0.5 mm.
- That violation gives three things:
  - the **mechanism**: its engine cause;
  - the **compass class**: the plan's verdict at that spot;
  - the **hit**: what the forced copper ran into. "Movable" means another wire's router track or via. "Static" means a
    pad, a board item, a dp bridge or a keepout.
- Every engine violation belongs to a never-drop wire, because only the forced paths write copper with a violation
  (`core.rs:10746` `viol_events`).
- These forced wires are drawn **last**: the never-drop pass at `core.rs:8752` runs after every other wire has committed.

**Board totals.** wiccon has 0 items and no forced runs. Its ERBS kept 69 of 69 bands.

| board (det s0) | KiCad new | on never-drop wires | unconnected | forced runs (relaxed / funnel / attempt) | forced vias | free-angle forced stretches |
|---|---|---|---|---|---|---|
| qmtech | 225 | 223 (25 conns, 98 engine viol.) | 17 | 22 (11 / 6 / 5) | 8 wires | 7 (9.0 mm) |
| esp | 17 | 17 (6 conns, 10 engine viol.) | 8 | 7 (0 / 5 / 2) | 0 | 2 (6.3 mm) |

**Fault classes.** Each row is a mechanism / compass class / hit. The counts are KiCad items from the script's
`--json` output.

| # | fault class | qm | esp | conns (worst) | absorbable by a re-taut drawing? |
|---|---|---|---|---|---|
| F1 | forced via beside pins: no disc in its interval; hits a foreign pad (pad_exit 49, overfull_nearby 22, model_gap 6) | 77 | 0 | 7 (ADV_DDCSCL @ADV7511.54: 24; FPGA_UART1_CTS/RTS @U9: 21; MAX_I2S0/ADV_SCL/MAX_SCLK @U7: 22) | no: the plan's via interval sits where no disc fits. Owner: via-plan (`terminal_star_sites`, plan moves) |
| F2 | forced via against a neighbour's copper (via_site / movable) | 28 | 0 | 2 (ADV_DDCSCL / ADV_DDCSDA via pair @125,73) | partly: the neighbour could move round a fixed via. The via pair itself is via-plan's `via_pair_joint` |
| F3 | forced run at a planned neck, hitting a neighbour's track (neck_loss / movable) | 63 | 0 | 2 pairs + 1 (VIN5V 0.5 mm conn 398 vs JS_INTB_5V: 37; JS_CS_5V vs VIN5V: 25; JS1_9: 1) | **yes**. The plan fits only with the wide wire necked (TopoR "shrinkage"). The drawn VIN5V has `neck_nm` 0 |
| F4 | forced run (or wrong side of a via) where the plan's cuts fit, hitting a neighbour (drawing_loss / overfull_nearby, movable) | 28 | 6 | 10 (qm JS1_9 vs JS1_5, ADV_DDCSCL_C vs a HDMI5V via, /rtc-spi/vbat vs VIN5V, SDRAM_A7, SDRAM_A4, JS1_6, ADV_BGVDD; esp USB_D-, IO7, IO9_BOOT) | **yes**. Room exists in the plan, and the sequential commit gave it away |
| F5 | drawing_loss, static hit | 2 | 8 | 3 (USB_D- @D8.1, +3V3 @R11.2) | partly: the wire's own pad exit, not a neighbour |
| F6 | planner over-full, movable hit | 15 | 0 | 2 (ADV_BGVDD vs MAX_LRCLK, ADV_AVDD wrong side) | no: the plan has no room; a drawing can only spread it |
| F7 | pad exit, static (forced run / wrong side) | 10 | 3 | 4 | no: pad-exit plan step (shopping list 5.10) |
| - | KiCad only (engine silent) | 2 | 0 | - | no: checker gap |

**What a fault-absorbing drawing could remove.**
- F3 + F4 are **97 of 242** items on the two boards (40 %), on 12 connections. This is the target.
- F2 (28) is shared with the via-plan design.
- F1, F6 and F7 (117) are plan faults and need plan steps.

**Upstream drawing counters on qmtech** (result.json `route.erbs`, draw stats).
- **Joint band commit.** ERBS gives 416 bands and keeps 383. It rejects 33: 20 static, 5 predicted, 8 gap.
  - 37 are rescued as octi form.
  - 24 are gap-kept, each recorded as a violation by the later run (`draw.rs:3797`).
  - 26 runs are drawn sequentially (`cut_back`), of which 15 have no band ("ends moved" 12).
  - 10 runs stay any-angle because their octi form hit another band.
- **Vias.** 56 planned via sites were not free when phase 1 placed the vias, and the drawing's search took them.
- **Pad ends.** 32 pad ends are off centre (esp 17).
- **Not attributed.** These counters are not attributed per KiCad item today. Step A0 does that.

## 2. Literature: how fault absorption is done

| Claim | Source (page) |
|---|---|
| Everything is routed, "even where this might entail violations"; the violations are removed later, automatically. | TopoR 6.3 manual p. 65 |
| Even crossings are "not an error ... eliminated when the wire shape is calculated (F5)". | 6.3 manual p. 105 |
| FreeStyle: "wire shapes are recalculated, but their topological paths remain the same". Wires cannot be edited by hand "because they are rebuilt automatically"; this "helps maintain clearance". | 6.3 manual p. 239; p. 175 ("optimal wire geometry is immediately calculated") |
| Overflow is counted, not hidden. The variant table shows Violation and **Shrinkages** ("width reductions in nominally-wide wires"), and variants are sorted by violations first. | 6.3 manual pp. 72-73 |
| The minimum width is used "in bottlenecks unless a different path is available". | 6.3 manual p. 56 |
| The rubber-band sketch has zero width and spacing. "Because rubber-bands are flexible, a later branch can easily be inserted between two previous branches without the need for explicit shoving or rerouting". Spoke creation then pushes wires apart. | Dayan 1997, PDF p. 32 (printed 21) |
| "Allow localized overflow regions ... used to direct the refinement". | Dayan 1997, PDF p. 28 (printed 17) |
| RBU, rubber-band updating, is "a continuous operation" that moves objects while keeping connectivity. Neighbours flex or snap (fig. 2). It is the only operation that can "incrementally move a via and propagate the results to the surrounding geometry". | Su & Dai 1997, PDF pp. 2-3 |
| Spoke creation per terminal checks cuts. "An unroutable cut indicates ... a design rule violation". The ERBS "guarantees a legal geometric transformation". | Su & Dai 1997, PDF p. 4 |
| A sketch is routable iff no cut is over-full. If it is not, spoke creation "will locate all of the endpoints that participate in overflowing cuts". | Staepelaere 1992, PDF pp. 12-13 |
| A layer is realised jointly by plane sweeps: OrderSegments, TransformLayer, StraightenLayer. The octilinear form uses two slope cases. | Staepelaere 1992, PDF pp. 15-20 |
| "The entire geometric wiring would have to be recomputed for even a small change". A local version is future work. | Staepelaere 1992, PDF p. 63 (printed 55) |
| Given the cut contents, each trace's ideal route goes through its doorways independently, with no order. With an over-full cut the output "need not even be a sketch". | Maley 1987, pp. 40, 44-47 (scan) |
| TopoR's metric path: per pad, a circle radius = pad + the widths of the wires between + clearances, then a funnel. Each wire is independent given the topology. "Omitting any factor" gives violations. | Popov & Popov 2012, PDF p. 3 (journal p. 119) |
| Delay lines: elements "are automatically moved apart to eliminate possible clearance violations". | Luzin 2015, PDF p. 4 |

**Silent.**
- No source says how TopoR updates neighbours internally, whether globally or locally (manual; Polubasov 2010: the
  metric stage is "outside the scope").
- No source gives an octilinear absorption step. Dayan, Staepelaere and Su & Dai convert the any-angle ERBS by sweeps;
  TopoR outputs any-angle or arcs only.
- No source says what to draw when spoke creation fails. Staepelaere reports the endpoints and stops.

**Reading.** TopoR absorbs faults in two ways.
- **(i) Recompute.** Every wire is recomputed from its topology, so no neighbour is ever "committed" (p. 239).
- **(ii) Shrink, then count.** Wide wires shrink in bottlenecks, and what still does not fit is counted as a violation
  (pp. 56, 72-73).

Our ERBS already does (i) for the first pass (`erbs.rs:2466` `realise`, all stretches from the plan). We lose it at the
first commit.

## 3. Our drawing today: where it commits too early

1. **Vias before tracks** (`core.rs:8291-8352`). All vias are committed first, in draw order. A via without a site is
   forced after the legal ones (`draw.rs:2412` `force_via`). This produces F1/F2. The wrong-side cases (qm 7, esp 7)
   are legal attempts that pass a committed via on the side the topology does not allow (`draw.rs:3565`).
2. **Joint band commit** (`joint.rs:157` `reserve_bands`).
   - All ERBS bands are committed at once, which is good.
   - A conflict edge goes to a maximum independent set (`erbs.rs:2162` `keep_bands`). Each **loser** gets one try in
     octi form against the kept bands (`joint.rs:354-376`), and is then drawn sequentially against them.
   - No band is re-realised with the conflict known. The stack radii (`radii::from_measure`) stay as measured.
3. **Per-wire loop** (`draw.rs:3406` `draw_wire_runs`, ladder `draw.rs:3498`: ERBS, exact, plan neck, funnel).
   - Each run is committed (`draw.rs:3773`) and is final for every later wire.
   - The octilinear form is a per-wire DP against fixed neighbours (`octi.rs:643`).
4. **Never drop** (`core.rs:8752-8808`).
   - A wire with no legal attempt is drawn again at the end, against all copper.
   - The candidates are a relaxed exact path, funnels and the ladder's violating attempts, chosen by `pick_forced`
     (`draw.rs:3024`), then a forced octilinear DP.
   - Only clearance is relaxed. Width is not: the plan neck appears only through the attempt-1 candidate. A neck to the
     width that clears the hit neighbour is never a candidate, which is F3.
5. **Post passes** (straighten `octi.rs:3112`, via reduction, fuse, polish). These refit one wire against fixed
   neighbours.
6. **Feedback** (`core.rs:8606-8750`) is the only neighbour redraw. It rips the blockers and redraws them **through the
   sequential ladder**, so their paths may change. The user turned it off for the TopoR loop (M6 2026-11-18,
   `core.rs:8607-8614`, docs/user/operate.md:617), so it does not run on main.
7. **Parallel drawing** (`par.rs`). Only order-free units run in parallel: stretches, band checks, octi DP nodes.
   Per-wire drawing and never drop are sequential.

**Where a re-taut fits.** At the forced wire (step 4), the code already knows which drawn wires it hits:
- `viols_of` (`draw.rs:2801`) records `against` per violation;
- `exact.rs:906` `blockers` gives the wires in the sleeve.

The undo primitives exist: `uncommit` (`draw.rs:3308`) and `restore_wire` (`draw.rs:4271`). What is missing:
- a redraw of a neighbour **inside its own sleeve and crossing order**, which is a rubber-band update, not a reroute;
- a band re-realise of a conflict component, by spoke push and re-funnel, before the first commit.

## 4. Plan (staged, smallest first; every step is an option, off = main copper hash)

**Rules for every step.**
- The topology (sleeve, crossing order, layers, vias) never changes.
- No run end moves. The pad-centre rule: `padend.rs:174` `run_end` stays the end of every run, and the count of pad
  ends off centre (`tidy.rs:291`) does not rise against main.
- Prices are graded, with the existing `violation` / `neck_*` / length factors. A step picks the cheaper of the
  candidates, which is the method, not a board-level gate.
- Copper is bit-identical for any thread or lane count (BTreeMap order; parallel only where the work is order-free).
- Each step has 2-4 candidate patterns. Before any of them reaches the user, it runs on main and on the step with
  `--keep-copper`. Main must fail visibly, and the expected answer must be the user-optimal path.
- The evidence ladder is det s0 esp + qmtech; wiccon and eprom must keep copper = main.

### A0. Provenance meter (no copper change)
- **What.** Per engine violation and per KiCad item, record:
  - the run's source: ERBS kept, `Cut(class)`, or forced relaxed / funnel / attempt k;
  - the hit wire's id, and its packed slack on the far side (lane intervals `iv.lo/hi`);
  - whether the plan necked the wire there;
  - whether its band was rejected in `reserve_bands`.
- **How.** A result.json block `draw_absorb`, read by `tools/draw_absorb_classify.py`.
- **Option.** None (a report only). Copper hash = main.
- **Expected.** It turns the "absorbable" hypotheses of section 1 into measured slack. It also decides the A2-vs-A3
  order: are forced runs mostly runs with a rejected band, or runs with a kept band?
- **Cost.** 1 det s0 unit (esp + qmtech).
- **Patterns.** None; this is a meter. Unit tests: every forced candidate source is reported.

**A0 as built (branch draw-absorb, 2026-11-26).**
- The drawing tags each violation when it writes the copper (`draw::ViolSrc`, not part of `Viol`'s equality, read
  by nothing in the drawing):
  - the writer: `forced_run`, `forced_via`, `gap_kept` (a band kept across a model gap, drawn second),
    `junction_join` or `diff_pair`;
  - the forced candidate (`relaxed`, `funnel`, `erbs`, `erbs_kept`, `attempt0..5`), the octilinear form, and the
    run's band status (`CutClass` name, `kept` for a gap-kept band);
  - the width, its floor (`State::w_min`) and the narrowest drawn track;
  - the hit: the router wire that owns the indexed item (`item:N`, live handle -> `ent_wire`), whether it was a
    reserved band, and that wire's packed room away from the violation (`iv`, at its nearest pass).
- result.json `violations[].prov` carries all of this. `compass::absorb_meter` adds the drawn compass class (a port
  of `toporlib/compass.py` `classify_violation`) and the fault class (`compass::fault_class`).
- `route.draw_absorb` counts them: per fault class and connection, writer, candidate, band status, forced runs per
  band status, re-taut room (hit slack >= depth) and shrink room (drawn width - floor >= 2 x depth).
- compass.py joins the KiCad items per fault class and checks the engine's class against its own
  (`absorb.class_mismatch`, must be 0). metrics.json `draw_absorb`; `tools/draw_absorb_classify.py` shows both.
- Rule: F1/F2 = a forced via (static/movable hit); else by compass class: F3 `neck_loss`; F4/F5 `drawing_loss`,
  `overfull_nearby`, `edge_overpredict` (movable/static); F7 `pad_exit`; F6 every other plan class.

### A1. Shrink before violate (TopoR shrinkage in the forced path)
- **What.** In `pick_forced` (`draw.rs:3024`), add one candidate: the best forced path, necked over the hit stretch to
  the width that clears the hit copper.
  - The floor is `st.w_min` (net class / board minimum; board rules are the authority).
  - The neck zone obeys the neck-from-pad rule: it may extend back towards the footprint and start at the pad edge
    (neck floor known), and never moves the run end.
  - Price: the `neck_depth` / `neck_length` factors against the `violation` price. It is counted as a drawn shrinkage
    (compass), not as a violation, and warned per wire.
- **Option.** `[drawing] forced_shrink = false` (default off; flip only after a verifier).
- **Expected.** F3, qm 63 items on 2 connection pairs, plus any part of F4/F6 where the wire is wider than its floor
  (from A0).
- **Via-plan.** Independent: vias are fixed points.
- **Patterns.**
  - `wide_bottleneck_neck`: a 0.5 mm power trace and a 0.2 mm signal through a gap that fits only with the power trace
    at 0.3 mm over 1 mm. Main: a forced clearance violation. Expected: necked over the gap only, DRC 0, one shrinkage
    warning.
  - `neck_not_at_pad`: as above, with the gap at the pad. Expected: the neck starts at the pad edge, the end at the pad
    centre.

**A1 as built (branch draw-absorb, 2026-11-26; lead GO widened to the gap-kept band).** `[drawing] forced_shrink`, default
false (since 0.5.0, unreleased), off = main copper (hash on 4 boards).

- **Joint commit** (`joint.rs` `reserve_bands`, before the gap pairs are kept):
  - Each model-gap pair (overlap <= `erbs_gap_keep_max_mm`) is tried with the yielding band first, then the other.
  - The band is uncommitted and re-realised by `neck_band` (joint.rs). Its zones are the plan's (`plan_neck`) plus one
    zone per track that breaks the rule against a gap partner (`Drawer::hit_zones`).
  - It takes the widest width in `[floor, nominal]` at which it hits no gap partner and nothing its full width did
    not hit; then `fit_neck` trims the zones (`neck_trim`).
  - Its pairs are measured again from both sides. A band that does not clear keeps its copper.
  - A0 found that the plan's own zones do not reach most model gaps: no over-full cut predicts them, so the zones sit
    round the pad ends. On qmtech, 3 of 46 tries cleared with the plan's zones alone.
  - The reservation keeps its zones (`Reserved::zones`), so the band keeps its neck:
    - the warnings name them;
    - its octilinear form is drawn at the neck's zones and width, never wider;
    - `tip_anchor_reserved` does not re-anchor it at full width.
  - Counted: `route.erbs.gap_shrunk`.
- **Forced run** (`draw.rs`, never-drop branch, after the octilinear form, pull and plan necks; `shrink_forced`):
  - Only a run that is still at full width everywhere. A run the plan or a pad end already necks keeps those necks.
  - Zones go over the violating tracks (`hit_zones`), at the widest width whose severity key ([`viol_key`]) is the
    floor's, trimmed (`trim_neck`), never ending inside its own end pad (`neck_out_of_pads`).
  - Taken only when the key gets strictly better (graded: a neck that only makes a violation shallower is taken too).
  - Counted: `stats.forced_shrunk`; each neck is a NECKED warning.
- **The floor** is `State::w_min`: the plan's neck floor, i.e. the KiCad minimum track width, the net class minimum,
  and 2 x the minimum connection width.
- **Where it touches the neck-regrow branch:** both paths build their copper with `Drawer::neck_items` (necked inside
  the zones, nominal outside). A change there, such as never widening after a neck inside a run, applies to both. The
  new code is in joint.rs (`neck_band`, the gap loop) and draw.rs (`hit_zones`, `shrink_forced`, the call in the
  forced branch, `a1z` in the adopted-band path). `neck_items` and `fit_neck` became `pub(super)`; their bodies are
  unchanged.
- **Tests:** `engine/tests/topo_draw_absorb.rs` holds the patterns `wide_bottleneck_neck`
  (`a_forced_run_shrinks_over_the_gap_instead_of_violating`) and `neck_not_at_pad`
  (`a_shrink_next_to_a_pad_has_no_width_step_in_the_pad`), plus the joint commit, the graded shallow case and the
  no-floor case.

**A1b as built (2026-11-27): `[drawing] reserved_keep_neck`**, default false (since 0.5.0, unreleased); off = main
copper (hash on 4 boards).

- **RCA (does main draw wider than the plan?).** Yes, on two paths. Both are legal, and on the measured boards none
  takes planned room:
  - **Re-anchor** (`padend.rs` `tip_anchor_reserved`). It fires only when the full width passes the checker over the
    whole band against statics, vias and every reserved band. The neck it drops belongs to the plan's hook into the
    pad side (draw-via-neck Q3), not to the tip path it draws. The user-approved Q3 expectation is "the bar ends
    straight on the pad's axis leg below the tip at 0.28", near full width.
  - **Adopted band's octilinear form** (`draw.rs`, the `real` closure). Main's `realise` necks the octi points at the
    widest width that passes. That width can be above the reserved neck width.
  - The meter is `route.erbs.unnecked_list` (`at` = `tip` / `octi` / `octi_wider`, `room_mm` = the length where the
    wider copper raises `via_plan_price` over the reserved width).

  | board (det s0 C1) | tip | octi_wider | room taken |
  |---|---|---|---|
  | qmtech | 1 (1.07 mm) | 5 (11.06 mm) | 0 mm |
  | esp | 0 | 1 (1.01 mm) | 0 mm |
- **Key on:**
  - The re-anchor draws the band's necked items (`kept_neck_items`: the plan zones at the reserved width, trimmed and
    grown by `fit_neck_with`).
  - The octilinear form keeps main's form when it is no wider than reserved. Otherwise it takes the kept-neck items
    if they are legal.
  - Counted: `kept_neck`.
- **Measured (det s0 C1, r2 4c440a4e, vs off):**
  - unnecked 6 -> 0 on qmtech and 1 -> 0 on esp.
  - esp: DRC 17 -> 14 (F5 8 -> 4, F7 3 -> 4).
  - qmtech: DRC 223 -> 226 and F4 26 -> 31. A new 6-item clearance cluster on ADV_TXC+/- / ADV_TX0- at
    (134.91, 68.84) appears and a 3-item one at ADV_TX2 goes. The cause is ADV_AVDD's kept neck: the HDMI pairs drawn
    after it take other forms, and USB1 TX0-/TX1-/TXC- detour goes 1.06 -> 1.36.
  - The key stays off. It contradicts Q3 at the tip, and the planned room main's wider copper would take is 0 on
    these boards.

### A2. Re-taut neighbours of a forced wire (local rubber-band update, RBU)
- **What.** Before a wire W is forced:
  1. Take N = the drawn wires that W's best candidate hits, the movable hits from `viols_of`. They are ordered by
     route order and capped by `absorb_max_neighbours` (default 3, a work cap).
  2. Uncommit N, then draw W at its best attempt against the rest.
  3. Redraw each n in N as the taut path **in its own sleeve with the same crossing order and layers**: the ERBS
     funnel over n's plan sleeve with W's copper as a sided obstacle, then its octi form. This is not the feedback
     ladder.
  4. Keep the joint copper if its price (violations + length of W and N at their tier weights) is below forcing W
     alone. Else restore it (`restore_wire`).
- **How it differs from feedback.** M6 removed feedback: a reroute that could change the path or topology. A2 is TopoR's
  "shapes recalculated, topology the same" (manual p. 239; Su & Dai RBU p. 2).
- **Option.** `[drawing] absorb = "off" | "retaut"` (default off), plus `absorb_max_neighbours`.
- **Expected.**
  - F4: qm 28 + esp 6 items, 10 connections.
  - The esp/qm wrong-side cases: a sleeve funnel passes each via on its class side by construction.
  - F2's 28 where the neighbour has slack (A0).
  - F3 where shrinking costs more than moving the neighbour.
- **Via-plan.** This is the track-side half of via-plan's drawing contract: vias stay at the plan's point and the
  tracks re-taut round them. A conflict that A2 cannot absorb stays a `model_gap` item for the plan.
- **Patterns.**
  - `retaut_neighbour`: B, drawn first, hugs the side of a gap where A's taut path needs to pass. The cut fits A + B.
    Main: A forced. Expected: B one pitch over, A straight, no detour.
  - `retaut_round_via`: the same with a fixed via of net C in the gap. Expected: both pass the via on their class side.

### A3. Iterated band realise (absorb before the first commit)
- **What.** In `reserve_bands`, for a conflict edge between two statically legal bands (predicted or gap, not static):
  1. Raise the spoke radius of the inner stack at the conflict portal by the overlap.
  2. Re-realise only the component's stretches (`erbs.rs:1465` `erbs_layer_with` with its `mine` subset).
  3. Repeat until no conflict remains, or `band_passes`. Su & Dai repeat passes until the cost converges; spoke
     creation (Dayan p. 32; Staepelaere pp. 12-13).
  4. Only the conflicts left go to `keep_bands`.
- **Method fit.** This is the abstract-first form of A2. It runs before any copper and is order-free, so it can run in
  parallel on the conflict components.
- **Option.** `[drawing] band_passes = 0` (0 = main).
- **Expected.** The 13 predicted + gap rejections and the 24 gap-kept bands on qmtech (esp 3 + 1), with their downstream
  cut-back and forced runs (A0 names them). It may make A2 rarer, so A0 decides whether A3 goes before A2.
- **Via-plan.** Vias are sleeve features with fixed points. Better planned points (plan moves) mean fewer conflicts.
- **Patterns.** `band_conflict_pair`: two wires through two consecutive gaps where the per-portal radii disagree.
  Main: one band rejected, then drawn with a detour or forced. Expected: both taut, side by side.

**Why no over-full cut predicted the qmtech gap pairs (Goal 3 RCA, 2026-11-27; no code).**
- Method: `TOPO_ERBS_DUMP` of off qmtech det s0 C1 (the dump now carries every cut with its slack), then
  `tools/draw_absorb_gap_rca.py`.
- The cut test was right: there was room.
  - 24 ERBS model gaps: 22 `common_cut_fits`, 2 `at_terminal`.
  - For each `common_cut_fits` gap, a cut carrying both bands lies within 0.041 mm of the gap. Its slack is
    0.345-33.7 mm (median 5.17 mm), and none is below one pitch.
  - The gaps are shallow: median 0.103 mm, max 0.467 mm.
- The fault is the realisation between cuts:
  - Each stretch's ERBS is computed on its own through its own spoke tips. Portals overlap on slack cuts.
  - The spacing holds along each cut, but not perpendicular to an oblique segment near a neighbour's bend.
  - Dayan's spacing claim (p. 26) assumes per-portal consistent radii, and that is not proven for our inflated
    sketch (§2.5).
- How the joint commit's gap pairs relate to the ERBS gaps:
  - 15 of the 24 pairs are ERBS gaps.
  - 9 are copper-only, from octi / realise shapes.
  - Of A1's 9 remaining pairs, 7 are `common_cut_fits` and 2 are copper-only.
- Plan-side item for later: a per-pair spacing check in the ERBS with order-aware portals, i.e. a joint sweep over a
  band component (A4 / Staepelaere's joint layer pass). A cut-count test cannot see this fault.

### A4. Joint octilinear form per band component (only if A0 shows it is needed)
- **What.** Replace the per-wire octi DP for a component of bands that share passes with Staepelaere's joint
  TransformLayer / StraightenLayer sweep (pp. 15-20), with the precedes order from `erbs.rs:2076`.
- **Option.** `[drawing] octi_joint = false`.
- **Expected.** The `octi_vs_band` 10, `octi_asis` 4 and forced free-angle 7 stretches on qmtech. Their KiCad items are
  unknown until A0. The literature has no incremental octilinear step (Staepelaere p. 63), so it would be our own design
  and the largest step.

**Not in this plan** (plan faults, other owners):
- F1 (77): via-plan design, `terminal_star_sites` / plan moves;
- F2's via pair: via-plan's `via_pair_joint`;
- F6 (15): the plan;
- F7 (13): pad-exit plan step 5.10;
- the 2 KiCad-only items: the checker.

## 5. Invariants and tests
- Every option off gives a main copper hash on eprom / wiccon / esp / qmtech.
- The topology is unchanged per redrawn neighbour. A fuzz test checks the crossing order per shared edge, before and
  after, against an exact side test.
- No run end moves. Pad ends off centre are no higher than main on every board.
- Each candidate source has a unit test that a call-site mutant kills (vfy-dvn5 lesson).
- Boards: per-connection DRC set vs main (rule 4), det s0 first. Real settings only for the final candidate, in ccx33
  slots.

## Decisions for the user
1. A2 redraws already drawn neighbours. It keeps their topology, so it is TopoR's recompute, not the M6 reroute. Is that
   compatible with "draw once"?
2. A1 necks a wide trace in a bottleneck instead of writing a clearance violation (TopoR shrinkage). Should that be
   allowed down to the net class minimum, outside the pad exit?
3. Order after A0: A1, then A2 or A3, by what A0 measures.
