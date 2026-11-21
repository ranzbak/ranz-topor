# first-draw-45: the first draw's octilinear form where 45° fits (A5 first-draw part)

**Status:** P1-P5 implemented and approved by the lead (P1-P3 2026-11-21 r2, P4 user-approved r6, sweep default r9); r10 adds the pad-centre, tie-floor, P2-wall, sweep-weight and loser-corridor rules below (losers-first draw order opt-in). Reports: scratch/agent-wt/reports/first-draw-45-r*-*.md. Base bbd6e0f1.

## What the diagnosis found

The writer of every first-draw free-angle stretch is the octilinear DP of the ladder (`octilinear`, draw.rs:3527 at
attempt -1 on the adopted ERBS band; ERBS_KEPT re-commits that result, draw.rs:3600). The DP keeps a taut piece
(octi.rs:849/958) when no connector chain is accepted. Read-only probes (`TOPO_FD45WHY`, `TOPO_OCTIPROBE`,
`TOPO_FD45_ORDER`) name the cause per stretch. They are copper-neutral: the qmtech copper hash is the same with and without them.

| family | what blocks every 45° connector | qmtech det s0 | esp det s0 | patterns |
|---|---|---|---|---|
| F1 no chain | an *octilinear* taut piece is a connector, so it must pass the leg test (octi.rs:1035-1046). The taut run fails it, so the DP has no chain at all and the whole run stays any-angle (`octi none`, `FD45NONE`) | 8 runs, 115.5 mm | 9.3 mm | - |
| F2 wall | the run's own taut path crosses a strip wall: an edge an undrawn wire crosses that is not one of this run's portals (octi.rs:1253 `blocked`). Every connector near it crosses the wall too | 17 stretches, 42 mm (+28 mm strip) | about 54 mm (walls; the taut flag was not logged on esp) | - |
| F3 order | reserved bands of undrawn lane neighbours, nothing else | 8 stretches 56 mm, plus about 103 mm mixed with walls/drawn | 3.2 mm | **K5: all of it** |
| F4 static | pads, holes, kept copper | 33 mm | 7 mm | U6 edge wire |
| F5/F6 | drawn copper only (38 mm); legal, but `planned_room` priced above `any_angle` (29 mm) | | | |
| shape | every form that fits crosses its own chord, so the whole bundle has to move | | | **U6** |

K5: the draw order is D1, D2, D3, SDA, D4, SCL (core.rs:8014). The bands are straight chords, so `precedes`
(erbs.rs:2069) relates nothing; its only sources are bends at cut spoke tips and on stack arcs, and the
`Feature` bends round the kept 3V3 add nothing (erbs.rs:2105; inferred from `ERBS precedes {}`). D1 is drawn first, in the
middle of the bundle. Its expected form is blocked only by the reserved bands of D2, D3, SCL and SDA (octi.rs:720). The fix is the order, not the node set:
- with the lane order SDA→D4 the unchanged DP gives 0 free-angle and DRC 0;
- with the order D4→SDA it also gives 0.

U6: `precedes` has 2-cycles (S4↔S7, S1↔S6). The cycle break (erbs.rs:2285) takes the earliest wire of the given
order, which is S5, with 6 unmet predecessors. Orders that follow the relation do not help (135 → 151/152 mm). Every
expected form crosses its own taut chord, so it always crosses a reserved neighbour band. The edge wire's
one-sided elbow runs into the DB9 mounting hole. The one-sided form that fits is a 45/0 staircase close to the chord,
and the DP has no nodes along a long chord to build one (octi.rs:556-564).

## Proposal (this branch): P1-P3, all in the existing DP / order, no new search

- **P1 the taut run is always a chain.** A piece of the taut path counts as legal as drawn whatever its direction.
  - An octilinear piece enters with its direction, without the leg test, as the any-angle pieces already do.
  - The module doc already promises this ("the original taut segments are always available ... never an open wire because of the style").
  - Effect: one bad piece no longer keeps a whole run any-angle (F1).
  - Lead condition: no leg test does not mean no clearance pricing. In the first draw every taut piece taken as drawn (octilinear or any-angle) pays its clearance below the rule at the forced DP's price (`violation` weight; a short at `SHORT_FIRST` times it; `asis_x`, octi.rs). It is costly, not failed: a legal connector wins wherever one exists. Each octilinear piece kept where the leg test refused it is counted (`OctiOut::asis` / `asis_below`, a warning line per run). The full checker still checks the run. Test: `octilinear_taut_piece_is_a_chain_as_drawn_priced_and_counted`.
- **P2 the taut run's own crossings are portals.** An edge the run's taut path crosses is not a wall for that run.
  - The undrawn wire on that edge keeps its room through the existing graded lane-slot price (`gates`, P6 b). Its span is that wire's planned crossing less the lane spacing (`neighbour_bound`'s rule, extended to non-portal edges).
  - This is a price, not a cut-off (F2). The edge beyond the undrawn wire's planned crossing stays a wall (the topology). The gate is graded on the room the undrawn wire loses: `room x pitch x (x + x^2)`, `x = depth past (planned crossing - spacing) / pitch`. An edge the taut path crosses on both sides of a planned crossing stays a wall. Only an inner edge of the strip's region opens; a boundary edge stays a wall (r2h). r3 (esp RCA): the wall starts at the undrawn wire's planned crossing less the spacing (its full room), not at its planned crossing (r2) nor at its slack limit (r3a). r2 let the graded gate (at most about 2 pitches of length) buy the room the wire had no other place for: esp USB_D+ was forced (0 -> 14 violations), drc_new 49 -> 75. Lending the wire's packed slack made it worse (49 -> 83): the slack holds the wire's lane over its other edges. Walls at the spacing limit: esp 49 -> 46, no connection worse than base.
  - Lead condition (r2): one test where the priced portal loses against a legal 45 degree alternative. Superseded in r3: the esp RCA showed that a priced lost room is wrong in itself (see below), so by default the portal opens only in the room the plan left free and there is nothing to price. Test: `own_taut_crossings_are_portals_in_free_room` (walls at the spacing limit, open at the run's own crossing). The graded slack zone remains as the debug arm `TOPO_FD45_ON=slack`.
- **P3 sweep order (Staepelaere 1992).** Two parts:
  - *Relation.* Besides the bend relation, every two consecutive items of an ERBS cut get "the lower one first" along the layer's sweep axis. The relation is OrderSegments' full *precedes*: a precedes b if a vertical line meets a below b (PDF p. 22). It is acyclic per segment, and a topological sort gives the transformation order (PDF p. 29). Each segment is then pushed onto the already transformed ones below it, within its lower triangle (PDF pp. 16, 33). This is the room the DP needs: the undrawn neighbours lie on the other side. Kong's result is that the ERBS spacing admits these S-routes (cited PDF p. 18).
  - *One axis per group (r2f).* Staepelaere sweeps a layer in one direction. A pair's score is `[y, x]` summed over its cuts; one axis is read for each group of wires that meet on cuts (a connected component of the scored pairs): the axis with the larger summed evidence (y: lower first; x: left first). A pair with under 10 % of its evidence on that axis stays unrelated. Per-pair axes (r2) mixed "lower" and "left", which disagree on a bending bundle, and drew K5 from the middle (`add_sweep`, erbs.rs).
  - *Cycle break.* At wire level cycles exist (U6). When no wire is free, Kahn's sort takes the wire with the least unmet weight (r10: bend pairs 1, sweep pairs their side consistency; with bends only, the fewest unmet predecessors; ties: given order), not the earliest.
  - *Option.* `[drawing] octi_order = "sweep"` (default since r9, lead decision on the r8 ladder: 12 fewer new DRC items than bends, same unconnected/vias/routed, the user-approved U6 bundle) | `"bends"` (the bend relation only; r6 re-judge: neither order better on every count then), documented in topor.toml, configuration.md and the schema help.
- **P5 the taut pull keeps an octilinear leg unless the shortcut pays** (F7, draw.rs `pull_pts`). The pull after the octilinear form removed a corner between a 45 degree leg and a kept taut piece: free angle where 45 degrees fitted, the same mechanism. Now a corner next to an octilinear leg goes only where the shortcut is cheaper on the DP's scale (length + `any_angle` price). Graded: a large saving still wins. Test: `pull_keeps_an_octilinear_leg_unless_the_shortcut_pays`.
- **Layer first stays the plan's** (octi-fallback-layer-first). F4 stretches stay priced any-angle and counted. No drawing-level via pair in this branch.

- **P4 (r6, user-approved 2026-11-21, reports/first-draw-45-r2-u6-p4.png panel 3 = expected bundle).**
  - *Step 1, S-route chord nodes* (Staepelaere 1992, PDF pp. 18-19). Every non-octilinear taut segment of at least two pitches gets nodes along it, on the taut path, a pitch apart. A segment over 48 pitches gets 48 nodes, spaced further apart (`CHORD_NODES`, a work-size bound).
    - Between chord nodes the DP's elbows and Z shapes build the octilinear staircase close to the chord.
    - The steps go to the side where the neighbours leave room: with the sweep order, that is the side of the already drawn ones.
    - The bend and jog prices make a longer step cheaper wherever it is legal, so the DP takes as few steps as the walls allow.
    - A piece between two chord nodes may stay as drawn (P1). Debug arm `TOPO_FD45_OFF=p4`.
  - *Step 2, straightening* (StraightenLayer, ch. 5, PDF p. 15).
    - `Drawer::octi_straighten` runs right after the drawing, before the as-drawn snapshot.
      - It runs there so the variant table, which keeps the as-drawn variant on a tie, sees it.
    - It goes over the wires in the draw (sweep) order, to a fixpoint: at most 4 passes (`STRAIGHTEN_PASSES`). A wire handled before its neighbours gets another turn once they have moved.
    - Per run, it replaces the longest stretch of two or more bends that one form of its ends redraws at a lower price. The forms:
      - straight, elbows and centred Zs;
      - Zs whose middle leg runs at clearance beside a parallel drawn segment of another net (Staepelaere: each segment pushed onto the transformed ones, PDF pp. 16, 33).
    - Price, on the DP's scale: length + `style_price` (bends by angle, jog penalty) − `bundle_pull` × the length that follows a neighbour.
      - Where the bend lands (the depth of the long stretch) is a tie on length. The pull breaks it towards the drawn neighbour. Remaining ties go by form order.
    - An end leg of the run keeps its direction, and at least its length or the jog leg. Feet of the net's other wires stay vertices.
    - A step is taken only where the whole run's price drops by more than 1 nm (r6g). The stretch price alone is not enough: `style_price` is not a sum over stretches (a jog reads both legs beside it), so two overlapping stretches could each look cheaper than the other. On K5 that swapped forever (r6f, SIGKILL). With the whole-run price, every step lowers a bounded price, so the pass ends.
    - The replacement must pass `refit_run` (the full checker against everything drawn) and the split guard.
    - Option `[drawing] octi_straighten` (default true); debug arm `TOPO_FD45_OFF=p4s`.
    - Lead r6: the pattern's straight pad exit is not asserted. It goes to the shopping list as PAD-EXIT-AXIS, the DP's off-axis exit price.
  - Invariants: the result is legal or unchanged (refit_run), deterministic (vertex order; shapes by price, then index), and never longer than the price allows.
  - Test: `u6_pinrow_long_bundle_is_one_octilinear_bundle` (lead r6):
    - 0 free-angle, clean;
    - every wire at most 0.25 mm longer than its reference track (the pattern's formula). The bound is the reference length, not the octilinear pad-to-pad distance, because the second-row pads S6/S7 need their vertical leg between the first-row pads;
    - one bundle: every wire has a horizontal level of at least 4 mm, and neighbours in pin-row order are 0 to 0.5 mm apart, each below the one before. The level's y is not asserted (a tie).

- **r10 rules (lead-approved 2026-11-21).**
  - *Pad centre in P4.* A chord node whose stub into an own pad would be priced (`stub_price` > 0) is not a node: the pad end stays the pad centre (topo_pad_snap side_exit). The in-pad stub price is floored at one pitch (`OctiCfg::stub_nm(pitch)`), so an elbow bend inside the pad is never free at `bend_mm = 0`. Straightening never moves an end leg off its pad anchor (`pad_leave_lost`, both_ends).
  - *Tie floors.* A bend costs at least `BEND_TIE_NM` (1 um) per 45 degrees and the jog penalty uses the same floor: at `bend_mm = 0` a staircase (also one alternating long and tiny legs, verifier F8) loses the tie against its elbow instead of the node order deciding. Far below any length the copper can tell.
  - *P2 walls from ERBS (3a).* On an edge P2 opened, an undrawn same-layer wire's ERBS path is a wall unless the run's own taut path crosses it (Y4's octilinear rescue crossed Y1's band inside a triangle).
  - *Sweep weights (3b, graded).* A sweep pair weighs its side consistency `|sum of offsets| / sum of |offsets|` on the group's axis; the cycle break takes the least unmet weight (bend pairs weigh 1). Pairs that swap sides between cuts or layers fade out of the cycle break. Bend 2-cycles stay the bends' (the sweep breaking them put U6 out of order).
  - *Joint losers draw first (mechanism C), opt-in.* Drawing the wires the joint check rejected before the kept ones fixes tw2 swap_vias under the sweep order (drawn after their kept neighbours, those neighbours' octilinear redraws took the loser's room: X2 cut back and forced). On the board ladder it cost esp +15 DRC items (r10l vs the arms run), so it is off by default: `TOPO_FD45_ON=loserfirst`. The tw2 a_band test runs under `octi_order = bends` until TW2-SWEEP (shopping list) is solved.
  - *Loser corridors.* The octilinear DP also prices a leg inside a joint loser's ERBS corridor (its half width + the larger clearance + the run's half width either side) like a lane zone, `room x length x (x + x^2)`. Graded, no wall; it does not decide tw2 (the loser's ERBS is any-angle, its octilinear room lies elsewhere: the ROOM-FROM-OCTI caveat). Debug arm `TOPO_FD45_OFF=loser`.
  - Open: ROOM-FROM-OCTI, TW2-SWEEP (shopping list); STAIRCASE-HUG (shopping list, pattern first) (the DP prefers a many-bend staircase hugging an any-angle band over a 2-bend form, every leg at least the jog leg; tw2 X3 under the sweep order).
- **Debug env (copper-neutral):** `TOPO_FD45_OFF=p1,p2,p4,p4s,p5,lane,loser,padc,tie,p2w,sweepw` (arms off: padc = r10 pad-centre chord filter, stub floor and pad_leave_lost; tie = BEND_TIE_NM floors; p2w = P2 ERBS walls; sweepw = sweep side-consistency weights), `TOPO_FD45_ON=slack,loserfirst` (opt-in), `TOPO_STRDBG` (straightening steps), `TOPO_REDUCEDBG` (via reduction repairs), `TOPO_FREEDBG` (free-angle writers: FREEDP, FREESNAP per stage), `TOPO_RUNPROV` (run provenance, octi tag), `TOPO_ERBS_DEBUG` (ERBS order, weights and draws).

## Invariants, tests, cost

- **Invariants.**
  - The DP's form is the taut run wherever no 45° connector fits, never an open wire.
  - Walls stay for drawn copper, static objects, undrawn via chords, and edges the taut run does not cross.
  - The order is deterministic (ids, `total_cmp`). Copper is identical for any thread or lane count.
  - The joint commit (I1) is untouched: reserved bands stay hard.
- **Tests, test-first, one per mechanism, each killing its mutant.**
  - K5 fixture (model JSON in `engine/tests/fixtures/drcpat`): 0 free-angle, clean, and the e2e xfail removed.
  - Unit tests: P1, a taut run with a straight piece the leg test refuses; P2, a taut crossing a lane neighbour's non-portal edge; P3, consecutive cut items and the cycle break.
  - U6: report the free-angle number, with no pass/fail claim unless P4 is approved.
  - Ladder det s0 on 4 boards: the set per net no worse than base 5571d1bd.
- **Cost.** No new DP nodes (P1-P3). qmtech router CPU vs base is reported. A timing claim is made only from a ccx33 slot.
