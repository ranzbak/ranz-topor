# bundle-turn: room at a fork, local relaxation, re-fit after straightening

RCA: scratch/agent-wt/reports/bundle-turn-rca-20261127-0949.md (incl. its addendum). The patterns `turn_fork_corner` and
`hole_bundle_room` (harness/bundle_turn_patterns.py) were approved by the user on 2026-11-27.

## 1. Planned room where an undrawn neighbour turns off (octi.rs `octilinear_dp`, draw.rs `neighbour_fork`)

**What.** The `planned_room` zones join the bounds of the same undrawn lane neighbour on two consecutive crossed edges.
At a fork the two wires share no next edge: the outer wire ends on a pad, or the inner one wraps the pad's corner.
There the neighbour's room cost 0, and the first-drawn wire could take an equal-length early bend into the neighbour's
slot (JS1_5 / JS1_9 at RN2).

**Change.** Per crossed edge and side, when the undrawn lane neighbour's adjacent pass in the next (or previous)
triangle is not on the wire's own next (previous) edge, add one more zone. It runs from the bound on this edge to the
neighbour's planned point beyond: its crossing of its own next edge (ERBS, else taut, as `neighbour_x`), or its run end
when it ends inside the triangle. That point is offset by the lane spacing towards the wire. A neighbour that changes
layer in the triangle is skipped, because its via has its own walls. The zone is a graded price like every zone, not a wall.

**Depth (fix round r2, 2026-11-27).** The r1 zone was a half plane, unbounded in depth. A leg of the same run 2.6 mm
away, past the neighbour itself, paid `(d / pitch)^2` per mm, and the DP bent the run's pad end leg 30 µm off the pad
axis to pay less: qmtech crop Q3, ADV_AVDD at x 137.22, a hook into C19.2 (topo_dvn `crop_q3_avdd_enters_along_the_pad_axis`).
The fork zone now ends on the far side of the neighbour, `2 gap` deep: from the bound line, `gap` to the neighbour,
`gap` past it. The price is a tent, `x = min(d, 2 gap - d) / pitch`, highest on the neighbour's line. Zones between two
bounds keep `x = d / pitch` (infinite depth). RoomPre carries the depth, bit-identical to the reference scan (fuzz test
`room_and_follow_indexes_match_the_scans`, half its zones finite).

**Run ends (r2).** A fork zone that covers the run's own end (its pad or via) is not added. The end is fixed there,
closer to the neighbour than the lane spacing, and the two wires neck down to each other at their pads. Pricing that
room bent the approach away from the end: judge pattern `distant_radius`, W1 14.309 mm against its ERBS 14.234 mm
(topo_m3_patterns). Item 1's effect stays: in `turn_fork_corner` N2 has its reference 3 bends, 4 with the arm off.

**Literature.** Bast, Brosi & Storandt 2020 §4.3 p. 7 (room kept for lines drawn later). TopoR 6.3 manual p. 239
(wires stay apart where there is room).

**Debug arm.** `TOPO_FD45_OFF=fork`.

## 2. Forced runs relaxed only at the bottleneck (exact.rs `exact_fix_local`, draw.rs forced block)

**What.** `exact_fix_relaxed` relaxed the clearance to every foreign object along the whole run, so the relaxed taut
run also cut corners where there was room (JS1_9 at H2).

**Change.** `Wp.relax_on` holds a small set of object ids, at most 8, and `need_of` relaxes only those.
`exact_fix_local` starts with the set empty. For each stretch it cannot repair, it adds the objects that the failing
segment comes too close to, then repairs again. The f-ladder and the bisection are unchanged. The run-wide relaxation
is used only when no local set gives a path at any f, so never-drop is unchanged. The candidate is tagged
`relaxed-local`, and the never-drop line counts it.

**Literature.** TopoR 6.3 manual p. 237: the nominal clearance wherever wires can be set apart, the minimal clearance
only in the narrowest spots.

**Debug arm.** `TOPO_FD45_OFF=relaxloc`.

**Test (r2).** exact.rs `forced_runs_are_relaxed_only_at_their_bottleneck`: a two-track gap that is illegal at the
full clearance and an object C 3 mm further on, close to the straight run but legal at its clearance. The local repair
relaxes the gap's two tracks only and keeps C at the nominal clearance; the run-wide repair relaxes C too. It fails when
`need_of` ignores `relax_on` and when the set never grows.

## 3. Re-fit after straightening (core.rs straightening block, draw.rs `refit_forced`)

**What.** Never-drop runs are forced against their neighbours' first drafts. The straightening later moves those
neighbours, but it never touches a wire that has violations. Its cap of 4 was reached while it was still changing.

**Change.** After the straightening passes, every wire with violations is drawn again by force, in draw order, against
the current copper:
- Its vias stay.
- The new copper is drawn beside the old, which is the same net and so no obstacle.
- The new copper is kept if its `viol_key` is lower, or equal and more than 1 µm shorter.
- Otherwise the old copper, its crossings and its counters are restored.

If any wire was kept, the straightening runs again, with the same per-phase bound. This is one re-fit round. The cap
becomes the option `[drawing] straighten_passes` (default 4, unchanged). Convergence is reported in the straightening
line (fixpoint, or bound reached), and there is a new line for the re-fit.

**Literature.** TopoR p. 237 (as in item 2). Staepelaere 1992 ch. 5 p. 15 (StraightenLayer).

**Option and default.** `[drawing] refit_forced`, off by default (lead 2026-11-27). On qm (det s0, bt1r5 arms) the re-fit
cost about 75 s cpu for 1 fewer DRC error: 15 redraws, 3 kept, 0 legal. Only wires whose violation partner, or a wire on
one of their planned edges, moved are drawn again. Parked: re-fit only the violating runs (`refit_run`), not the whole
wire by force.

**Debug arm.** `TOPO_FD45_OFF=refit`.

## 4. Over-full only when it does not fit at the factors a turn reaches (pack.rs `oblique_split`, `Taut::sq`)

**What.** In `turn_fork_corner`, P4 rip-up (core.rs `p4_victims`) marked the corridor edges as over-full: the two
horizontal edges and the diagonal edge from T1's corners to the wall. It put congestion history on them, and N3 was
rerouted round the wall (7.99 mm against 6.72 mm). The loop kept that detour.

The corridor fits N2 and N3 crossing square: the edge is 1.156 mm, the spans run from 0.325 to 0.831 mm, and the pitch
is 0.45 mm. It was over-full only through the oblique factor `infl_dir`, taken from the taut paths that cut the corridor
diagonally: 1.31 on the horizontal edges and 1.9 on the diagonal one. With that factor the edges were over by 0.084 and
0.314 mm.

**Change.** The per-edge model packs an over-full edge-layer a second time. In that second packing each pass gets the
oblique factor it can reach inside its sleeve (pack.rs `Taut::sq`, `reach_factor`).

- The sleeve cone: the crossing directions of the lines from a point of the pass's previous portal to a point of its
  next portal. A portal is the wire's free span on that edge, or a point at a terminal or a turn vertex. The extreme
  directions are among the four end-to-end lines.
- The reachable factor is the smallest factor over that cone, sampled every 1/8 of each side of the taut direction, plus
  the edge normal when it lies inside the cone.
- A wire in a straight oblique sleeve keeps its full factor. A narrow channel that the zero-width taut path cuts across
  diagonally allows a square crossing, down to 1. Both are graded, with no angle threshold.
- What that second packing lacks is the over-fill: over-full, rip-up victims, and the predicted violation.
- The part only the taut crossing lacks, `over - max(reachable, 0)`, is a graded price of `oblique_price` mm per mm, in
  the squeeze channel.
- `fits_without` (P4's rip-until-it-fits) uses the same rule.
- The drawing's lane packing (draw.rs `pack`) packs such an edge-layer at the reachable factors, but only when it does not
  fit at the taut angles and does fit at the reachable ones. Edges that fit at the taut angles get the same copper as
  before.
- The taut directions are unchanged.

The option is `[capacity] oblique_price`: default 1, and 0 restores the old rule.

**First version and why it changed (bt1r2, 2026-11-27).** The first version used factor 1 for every pass, so every
oblique crossing counted as square. Arm B (item 4 alone, det s0) moved 156 of 464 qm connections:

- JS2_2 went from 0 to 36 drawing_loss. Its 4-wire bundle runs straight at about 45° through a corridor. P4 no longer
  ripped it, and `pack` set its lanes at w+s along the edge, so the perpendicular pitch was (w+s)·sin and the lanes
  overlapped.
- esp model_gap went from 3 to 10.

A wire that runs straight on also crosses the feature-pair cuts beside the CDT edge, and the oblique factor stands for
those cuts. Only a wire that turns can trade it for length.

**Literature.** Leiserson & Maley 1985: an any-angle layout is routable iff no cut between two features carries more than
its length, with no angle term. Maley 1987 p. 32: a wire crosses every cut on its way, not only the triangulation's
edges. TopoR 6.3 manual p. 237.

## Invariants

- Copper is unchanged where none of the three fires:
  - no fork zone without an undrawn neighbour that turns off;
  - with no violating stretch the set stays empty, and the result equals `exact_fix` at f;
  - nothing is re-fitted without violations.
- A re-fit never raises a wire's violation key.
- Bit-identical for any thread or lane count: the re-fit is sequential, in draw order.

## Tests

- The two approved patterns, run end to end (det s0). `turn_fork_corner_no_violation_no_detour` covers items 4 (N3 keeps
  the corridor) and 1 (N2's 3 bends, 4 with the arm off).
- Unit tests on `exact_fix_local` (item 2, see above), on `reach_factor` (pack.rs
  `reach_factor_keeps_the_oblique_sleeve_and_finds_the_normal`: a straight 45° sleeve keeps 1/sin, the normal between two
  samples gives exactly 1) and on `refit_line` / `straighten_line` (both forms).
- Mutants on each changed line.
- Boards: det s0 with 4 boards against main (`ladder_diff --json`); the qm crop PNG at RN2 and under H2; real KiCad
  Cleanup per net with A1 off and on.
