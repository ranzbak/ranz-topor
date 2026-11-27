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

## As built (r2, lead OK 2026-11-26)

1. `exit_tight` (state.rs):
   - skips passes whose edge has both ends on the own pad's outline, and passes nearer to the other end than to
     this one (on a short wire the "first two" passes from a free end were the other end's bottleneck: both ends
     tight, the whole wire necked; topo_dvn neck_in_pad_row / neck_shared_channel, arm nrg_near);
   - keeps the board outline at the edge clearance besides other nets' copper (a terminal 0.15 mm from the edge has
     a tight exit; main caught it only through the previous point's bug; topo_dvn neck_edge_terminal, arm nrg_edge);
   - the cheap test on the even pass points stays the first filter;
   - then the taut path at the nominal width (`State::taut_prefix`: `gap_spans` ports of the nominal item, the
     wire's turns (junction vertices: free or node, never obstacle corners), `pack::funnel`) up to the last judged pass, relaxed round the clearance discs:
     a funnel leg between two edge ports can cut a fan corner closer than w/2 + s (neck_reentry: 0.40 mm against
     0.45 at the upper wall's corner), the drawing wraps it at w/2 + s, so the leg is bent out at its nearest point
     (6 passes, a rubber band). Tight only if the relaxed path is still too close (1 um slack): it was pushed into
     other copper;
   - then the drawing's axis exit of an elongated pad (`pad_axis`, as `octi::pad_exit`).
2. Join (user 2026-11-26, option a): the plan keeps a neck at each end and the middle's full-width room
   (neighbours laid out as without the join; r1 joined in refit_wire / cut_neck_widths and so freed room, qmtech DRC
   223 -> 255); the drawing (`Draw::plan_neck`) necks the whole wire when both ends are necked. A whole-wire neck on a wire
   wider than the board default warns apart (`Necking { whole_supply }`, "the whole wire (both ends needed a neck;
   a supply-width net ...)"), for POWER-NECK-MONOTONE.
3. `neck_items`: end zones narrow only the contiguous stretch from their own end.
4. Crop tool (harness/abstract_crop.py): Default-class nets get their hand width as a crop class `crop_w<um>`
   (the board routes them at their existing copper's width; the crop has none, so it routed qmtech's ADV_SDA /
   ADV_SCL at 0.25 instead of 0.127: false findings).

Debug arms (TOPO_FD45_OFF): nrg_join, nrg_contig, nrg_relax, nrg_near, nrg_edge. Debug prints (TOPO_NECKDBG): EXITTIGHT,
EXITTAUT, CUTNECK, PLANNECK, NECKWHY.

**Not in this branch: neck back to the T split.** The user rule (necks run from the pad back to the T split) means a
pad-end neck on a wire whose other end is a node (T junction) should run the whole wire. Today it stops one pitch past
the bottleneck. It is a two-line change in `refit_wire` / `cut_neck_widths`, but it lengthens necks on every T branch
(less planned room, the drawing still tries full width first), so it needs its own pattern (user approval) and board
runs: a separate plan item.

**Known limit.** exact.rs pieces and the octi pricing still use the circle zones of the drawing's neck plan for
pricing (search approximation only; the copper uses `neck_items`).

## As built (r4, lead 2026-11-27): the taut exit test behind `[search] exit_taut`

- `[search] exit_taut` (default **false**) selects `State::exit_tight`'s test. false = main's test
  (the stretch through the even points of the first two passes). true = the taut test of r2/r3:
  own-pad passes skipped, nearer-end passes only, the relaxed taut path, the board outline as a wall,
  the pad-axis exit.
- Why off: on esp and qmtech the truer exits (9 exits on esp that main called tight have room)
  reshuffle the plan into vias without a legal site and into crossing-vs-length flips. Both are
  priced too low today: unsited vias go to via-plan r3 (site truth), the flips to CROSS-VS-LENGTH.
  The taut test becomes the default once via-plan r3's site truth has landed.
- Flip measured and **reverted** (defaults-flip-a1, 2026-11-27, before via-plan r3 landed; lead decision: the
  default stays false; reports/defaults-flip-a1-20261127-1035.md): the warning holds. **Gate for flipping it after
  via-plan's site truth lands: each item below gone** (det s0 C1 + real esp, real KiCad Cleanup per net). det s0 C1, exit_taut alone vs main: qm DRC 200 -> 229, unconnected 16 -> 14, detours > 1.5
  23 -> 22 but new ones (FPGA_SD_SW net 32.7 -> 67.0 mm, ADV_CEC 19.4 -> 35.6 mm, ADV_TX2- 1.49 -> 2.93);
  esp 17 -> 37, unconnected 8 -> 11: the USB pair (U4.19/U4.20 -> D8.1/D5.1) dives to B.Cu with vias that have
  no site ("via 1 of 3: no site in its interval"), shorting D8.1, +BATT and IO8_SCL. Real esp 10 -> 31 (30
  cycles). Real KiCad Cleanup then loses connections main keeps (det qm 7 nets, esp 4, real esp IO20_RX).
  The user patterns neck_both_ends and neck_reentry match only with it (reentry fails at exit_taut false).
- On by default (regrow): the plan keeps both end necks with full-width room in the middle and
  the drawing necks the whole wire (option a); end zones narrow only their contiguous stretch; the
  whole-supply warning; crop widths; crop_q3 checks the hook geometry.
- Debug arms (TOPO_FD45_OFF): nrg_exit (main's test even with the key on), nrg_ownpad, nrg_taut,
  nrg_axis, nrg_near, nrg_edge, nrg_relax, nrg_join, nrg_contig.

## As built (r6, verifier r5 findings)

- **Silent full-width run (esp USB_D- U4.20 -> U3.26, +2 clearance at U3.27).** Not a later stage: the run was
  drawn so. `Drawer::fit_neck` turns the plan's whole-wire zone (slot 0, unbounded) into a circle round end `a` or
  end `b` and keeps the shorter neck. r5 put both candidates in slot 0, but since R-b (`neck_items`) slot 0 is the
  prefix of a run that starts at `a`, slot 1 the suffix of a run that ends at `b`. On the last run of a via wire
  (via -> U3.26) neither candidate narrowed anything, `trim_neck` dropped the zone (no violation more than its own
  base), the "shortest neck" (0 mm) won, and `plan_neck_run` returned the run at full width as legal: no NECKED, no
  VIOLATION (log: `NECKFIT wire 136 wn 134485 plan [inf] trim [] fit []`). Fix: the candidate from `b` goes to
  slot 1, and a candidate is judged by its violations first, then by its length. Guard (never silent): the fitted
  copper is checked (`plan_neck_run`; `trim_checked` for a forced run that came out legal); where it breaks a rule
  the plan's zones, which passed at that width, are drawn.
- **Never wider again after a neck, across vias.** Each run is necked on its own (full width first), so a via wire
  whose two ends both need the neck came out narrow | full | narrow across its vias. `Drawer::monotone_necks`, after
  the last run: the wire's track widths from `a` to `b` are made monotone by narrowing only (the cheaper of the
  running minimum from either end, by narrowed width x length). Narrower copper keeps every clearance and grazes
  no own-net pad it did not graze, so no rule breaks, and the widths are ones the wire already has. Not for pair
  envelopes, `Drawer::widths`, or `neck_from_pad = false`. Then the necks are read off the copper (one entry per
  contiguous stretch), so a whole-wire neck over several runs is one stretch and the whole-supply warning fires.
  Reported: "neck profile (never wider again after a neck): ...".
- Tests: in-module `draw.rs` (the last run's trim from `b`, the fit check on a middle run, the profile helpers,
  `monotone_necks`, `plan_neck` on a pair envelope vs a single), `topo_neck_regrow`
  `a_via_wire_necked_at_both_ends_is_one_neck_across_its_vias` (neck_both_ends with B.Cu open and an F.Cu wall
  across: two vias; r5 drew the run into P2's channel at full width, silently). Debug arms (`TOPO_FD45_OFF`):
  nrg_fitslot, nrg_fitcheck, nrg_trimcheck, nrg_mono.
