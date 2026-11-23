# draw-via-neck: via shift, neck only where the room ends, pad axis entry (ERBS drawing)

**What / why (user 2026-11-23, "fix: the drawing step").** qmtech Q2: a via jammed against the U9 column, its neighbour
RST necked to 0.09. Q3: ADV_AVDD necked from 0.37 mm below the pad row and hooking into the pad through its side gap.
TopoR moves vias and T-points to optimal positions by force iteration, then refines ("most violations are eliminated by
procedures for automatic via shifting", 6.3 manual p. 179 F7, p. 183; Dayan & Dai 1991 UCSC-CRL-91-40 pp. 2-6); the
minimum width only where needed (p. 56); don't stretch the wire to the pad origin (p. 71).

**RCA (missed-alt af9d2ddf + TOPO_ALTAPPLY, same loop as the user PNGs; logs in wt-missed-alt-runs/dvn-dbg).**
1. *Via (Q2, CTS).* `place_vias` (draw.rs 2054-2290) searches only the traversals of the plan's via interval
   (here (42, 45): four sliver triangles in the U9 fan-out, inradius 0.011-0.104 mm; the plan logged `nosite 1`). The
   planned site (97.527, 60.500) lies on pad U9.11 (VIAMISS, tight). The search stops at the first traversal with any
   legal site (draw.rs ~2193 `best.is_some_and(|b| b.1 != k)`). Its one legal site has 0.009 mm of room and a
   planned-room price of 9.37 mm (into RST's corridor), and it is taken: the price only ranks sites inside that
   traversal and is never compared with a shift. A legal planned site is also taken without its price (5.4 shortcut,
   ~2099). RST then necks (`plan_neck_run`).
   **Caveat:** by hand, even a via moved down CTS's B.Cu run leaves RST ~0.08 mm short: VBUS (pin 8) turns 45 deg at
   the column and takes the room that RST and the CTS stub need. That is fan-out nesting (SMD-FANOUT-STAIRCASE).
2. *Neck length (Q3).* `plan_neck` (draw.rs 4564-4615): the zone is a circle round the pad centre. Its radius is the
   far end of the last necked pass's triangulation edge plus w + s. `plan_neck_run` (4670) necks everything inside it
   at one width. `trim_neck` (4702) runs only for forced runs (4035). Pattern `neck_in_pad_row` today: the WHOLE 6 mm
   wire at 0.3. Expected: 0.286 mm past the row tip.
3. *Hook (Q3).* In the plan dump, wire 409 (ADV_AVDD) has a taut path ending (137.129, 71.239) -> (137.35, 70.884) ->
   centre. Passes 51-53 cross the own pad's spoke edges (centre -> corners): the plan reaches the centre through the
   pad's right-side triangle, the 0.2 mm gap to pin 28, necked (need 0.045). The ERBS ends at the centre and follows
   that path, and octi's axis `Exit` shape never fits because the previous node lies beside the pad, not past its tip.
   No step re-anchors a stretch at its own pad's tip, although tip and side entry are the same class (only own copper
   lies between).

**Proposal (drawing only; planner untouched).**
- **A. Via shift** (`place_vias`).
  - Candidates: today's sites, plus rings round the planned/best site (0.5-2 via diameters, 12 directions), plus sites
    in traversals up to `[drawing] via_shift_mm` (1.0) outside `[ka, kb]`. The passes in between change layer only
    where the per-layer class test holds (`via_reach` / `via_side_ok`).
  - One graded price for all: `via_plan_price` + squeeze + shift `room x dv x k (x + x^2)`, where x = shift / via
    diameter and k = 0.5. A shift of one via diameter costs less than one pitch of a neighbour's planned room.
  - No first-traversal break while the best price > 0. At price 0 today's pick stays (the measured qmtech bus case).
    A planned site with price > 0 competes rather than wins outright.
  - Then `via_shift_passes` (2) re-price every via with all the others placed (TopoR's iteration).
- **B. Neck trim.** Every plan-necked run is trimmed per zone. The radius is the cheapest of the bisection samples by
  `necked length x k_neck + planned_room price of the full-width part` (corridors of undrawn wires). So the neck is
  only where the room ends, and longer only where another trace needs the room. Pad-end zones stay anchored at the
  pad: never narrow-wide-narrow, and the T-split rule is unchanged (fit_patterns `neck_fit` keeps its answer).
- **C. Pad tip anchor.** An ERBS stretch that ends at its own elongated pad (an octi `Exit`) through the pad's own
  spoke passes ends at the tip leave point instead (the axis, about one trace radius out). This applies only when no
  other-net feature lies between the side path and the axis leg; otherwise the stretch is kept. The axis `Exit` shape
  then applies, and B drops the side neck.
- **Options (schema + docs):** `[drawing] via_shift_mm` 1.0, `via_shift_passes` 2, `neck_trim` true,
  `pad_tip_anchor` true. All judgement weights are graded; none is a cutoff.

**Invariants.** The class is kept (the same crossings per layer). Undrawn traces keep their planned room (A and B
price it). The drawing never drops anything. Copper is identical for any thread count.

**Tests.**
- The patterns below.
- One unit test per mechanism, each killing its mutant (the first-traversal break, the shift price, the trim
  predicate, the anchor's class guard).
- Q2/Q3 redrawn with TOPO_ALTAPPLY (missed-alt + these commits).
- Deterministic seed-0 esp + qmtech vs tw-on res-tip: DRC by class, judged per connection.

**PROPOSED patterns** (harness/dvn_patterns.py, PNGs reports/dvn-*-PROPOSED.png; today = tw-on c7f8bc8f):

| pattern | expected | today |
|---|---|---|
| neck_in_pad_row | 0.296 neck to 0.286 mm past the tip | **reproduces**: the whole wire at 0.3 |
| neck_shared_channel | neck down to y 6.716, where S leaves | reproduces in part: neck to 7.238, S drawn off |
| via_jam_shift | via below the column, N full width | **does not reproduce**: the plan puts the via in free room |
| pad_axis_entry | axis leg to 0.11 mm past the tip, then 45 deg | **does not reproduce**: today's exit is on the axis |
