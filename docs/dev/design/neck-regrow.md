# neck-regrow: a neck only where room is missing, and never narrow-wide-narrow

Branch neck-regrow, base main a18b3716. Evidence: hz-runs/nrg1 (the qmtech ADV7511 crop of via-plan, main + TOPO_NECKDBG).

## RCA

**ADV_DVDD (pad ADV7511KSTZ1.19).** On main a18b3716 the crop draws ADV_DVDD at 0.25 mm everywhere. The 0.09 mm piece in the
via-plan report came from main 6fbe3550, before draw-via-neck. Two causes combined:

- (a) The plan necks the first two passes of the wire anyway. `State::exit_tight` (state.rs) fires. Those two passes are
  edges inside pad 19's own rounded-corner fan. Their evenly spaced pass points lie 0.24 mm from GND pad 18 (need
  0.275 mm). The drawing leaves along the pad axis through the tip, where the room is 0.35 mm.
- (b) Full width failed only because of an edge clearance that narrowing cannot remove: the crop terminal XT1 sits
  0.06 mm from the crop outline. The old drawing then necked the plan's zone at pad 19 to the floor. On main,
  `plan_neck_widest` keeps full width there.

So (b) is fixed on main. (a) is a phantom neck that is still in the plan. It is counted as a necked stretch, frees
capacity that does not exist, and the drawing necks that zone whenever the run fails anywhere.

**ADV_SDA / ADV_SCL.** In the crop the necks at both ends are real, but they are crop artefacts:
- the crop has no existing copper, so the nets get the 0.25 mm default (DEFAULTS_USED warning), against 0.127 mm on the
  board;
- the terminals are 0.28 mm apart and 0.06 mm from the crop edge.

On the full qmtech board (det s0 dvn7s and real settings dvn7tq, tree = main), ADV_SDA and ADV_SCL are 0.127 mm and
ADV_DVDD is 0.25 mm all along. So "should not neck" does not reproduce on the board, but the shape is a real mechanism.

**Regrow, mechanism R-a (plan).** With `neck_from_pad`, `refit_wire` and `cut_neck_widths` neck each bottleneck
cluster from its nearer pad end. A wire with bottlenecks near both ends gets na > 0 and nb > 0, so the drawing has two
end zones and full width between them (SDA/SCL: 0.10 | 0.25 | 0.10).

**Regrow, mechanism R-b (drawing).** `neck_items` narrows every piece of every run of the wire whose midpoint lies
inside a zone circle. Two cases regrow:
- a run that leaves the circle and comes back in;
- another run of the same wire (another layer after a via) that passes within a pad's circle.

`fit_neck` / `trim_neck` resize the circles but keep this rule.

## Fix (proposal)

1. `exit_tight` (plan):
   - skip passes whose edge has both ends on the own pad's outline (the wire is still on its own copper there);
   - for an elongated pad (the drawing's `pad_exit` test: long side > 1.2 x short side), also test the drawing's exit
     leg: centre to past the tip plus s + w/2, along the axis towards the wire;
   - the exit is tight only when both the pass-point stretch and the axis leg are too close to other-net copper.
2. R-a: when a wire's necks would form two stretches (both ends), join them into one stretch over the whole wire. This
   applies in `refit_wire` and `cut_neck_widths`. Rules behind it: user 2026-11-05 (a longer neck costs no capacity,
   the narrowest part sets the current); 2026-11-07 (never narrow-wide-narrow); 2026-11-26 (never widen after a neck).
3. R-b: an end zone in `neck_items` becomes the contiguous stretch from its own end:
   - zone a narrows only the first run, from the pad up to the first exit from the circle;
   - zone b narrows only the last run, the same from its end;
   - middle runs get no end-zone neck;
   - the whole-wire zone (∞) and the mid zones (only with `neck_from_pad = false`) stay as they are.

**Invariant:** per wire at most one necked stretch, and it touches a pad end or spans the whole wire. Along the wire
the width never rises again after it has dropped, except where a second end neck is joined (whole wire). The neck
widths stay graded (widest that passes, TopoR 6.3 manual p. 56: the minimum width only where needed).

**Open, for the user (not in this branch unless approved).** The 2026-11-07 rule says "neck back to the T split". Today
a neck from a pad stops one pitch past the bottleneck, also on a wire whose other end is a T junction. Should such a
wire be necked from the pad all the way to the junction?

## Tests

- Rust unit tests, each failing on main:
  - `exit_tight` on a 0.3 x 1.6 row pad with 0.35 mm room on the axis;
  - `neck_items` on a run that re-enters its zone (one narrow piece, not two);
  - `refit_wire` with bottlenecks at both ends (all passes necked).
- Mutants on each changed line.
- Proposed patterns (PNG first, not in the suite):
  - `neck_both_ends` (R-a): a wire between two small terminals, each flanked by a foreign pad 0.28 mm away. Main draws
    narrow-wide-narrow; expected: one neck end to end.
  - `neck_reentry` (R-b): a pad-end neck whose path comes back past its pad.
  - The phantom plan neck (1) does not show in main's copper (main draws DVDD full), so it gets a unit test plus the
    plan dump, not a PNG.
- Boards: det s0 on the 4 boards vs cache a94cde2b (= main tree), the qm crop, and a real KiCad Cleanup comparison.
