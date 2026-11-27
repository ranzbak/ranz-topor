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

**Debug arm.** `TOPO_FD45_OFF=refit`.

## Invariants

- Copper is unchanged where none of the three fires:
  - no fork zone without an undrawn neighbour that turns off;
  - with no violating stretch the set stays empty, and the result equals `exact_fix` at f;
  - nothing is re-fitted without violations.
- A re-fit never raises a wire's violation key.
- Bit-identical for any thread or lane count: the re-fit is sequential, in draw order.

## Tests

- The two approved patterns, run end to end (det s0): each mechanism, and each arm off, shows its failure.
- Unit tests on `exact_fix_local` (relaxed only near the bottleneck) and on `refit_line` / `straighten_line` (both forms).
- Mutants on each changed line.
- Boards: det s0 with 4 boards against main (`ladder_diff --json`); the qm crop PNG at RN2 and under H2; real KiCad
  Cleanup per net with A1 off and on.
