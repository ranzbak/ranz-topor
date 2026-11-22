# pad-pour-via: dog-bone via for a pour pad whose thermal spokes are all cut

Status: A + B approved by the lead (2026-11-22, "KiCad's zone_filler spoke rule is the spec"); implemented.
Branch pad-pour-via, rebased on stitch-vias r4 c513c70c.

## RCA (replay of the stitching on the r3 qmtech run, `examples/stitch_replay`)
Of the 5 pads the stitch-vias r1 RCA found fitting, the estimate (`pourcheck`) misjudges 3:
- **U3.3, ADV7511.27**: the estimate puts them in the main GND component; KiCad's refill draws no spoke.
- **ADV7511.78**: the estimate joins it to the piece with .80, which gets a stub; in KiCad .78 has no spoke.
- **U3.36, ADV7511.20**: correctly cut. The radial pad stub (straight from the centre, 32 directions) finds no
  legal end: a straight line leaves a 0.5 mm-pitch row only along the pad axis, and the via has no room there.

Cause of the misjudgement: differences from KiCad 10.0 (`pcbnew/zone_filler.cpp`, 10.0 branch as of 10.0.8:
`ZONE_FILLER::fillCopperZone`, the block "Add thermal relief spokes" (lines 2954-3046), and
`ZONE_FILLER::buildThermalSpokes` (lines 3392-3760)):
1. The pre-route fill still holds the **old spokes**. KiCad knocks out the relief (the pad grown by the thermal gap)
   and draws a spoke only when the spoke's test point lies in `testAreas` = fill − reliefs − clearance holes,
   opened by half the minimum width.
2. KiCad's **test point** sits at the pad-frame bbox + gap + `max_error` + min_width/2. The estimate puts it at
   bbox + gap + 1 µm, with a tolerance of half_min + 0.71 cells. Also, KiCad draws no spokes when the spoke
   width, clamped to the pad's minor size, is below the zone's minimum width.

## What changes
**A. Estimate (pourcheck), KiCad 10's spoke rule.**
- Knock out the relief of every own thermal pad before the opening.
- Put the test points where KiCad 10 puts them (`spoke_test_points`: `buildSpokesFromOrigin`, the pad-frame bbox
  grown by gap + `m_maxError` + min_width/2; the test point is `spoke.CPoint(3)`).
- No spokes when the spoke width clamped to the pad's minor size is below the zone's minimum width.
- Test-point hit: within half_min + 0.5 cell of the opened fill (`SPOKE_TOL_CELLS`; KiCad: `testAreas.Contains`).
- Spoke survival (`spoke_joins`): KiCad adds the spoke, subtracts the clearance holes again and opens the fill by
  half the minimum width; a spoke that foreign copper narrows on its way to the pad is gone. Redone in a patch around
  the spoke: the pad joins the islands that kept cells reach from the pad.
- Facing spokes: KiCad also keeps a spoke whose test point lies in another spoke whose test point lies in it ("Hit-test
  against other spokes", issue 13316); the two pads join (qmtech C31.2 / X2.2).
- Check against KiCad on the replay world (ripped input + routed copper, refilled by KiCad, connectivity per pad):
  qmtech r3, 85 pads judged cut; vs KiCad 5 false cut (old estimate 5) and 13 false connected (old 15: U3.3 and
  ADV7511.27 fixed). The 13 left (U7, U9, USB1, U1.3) are the same with the old estimate: not this change.
- This also changes pour completion during routing. That is correct per kicad-tools-are-the-spec, and will be
  measured.
- Check: every thermal GND pad of qmtech, estimate hits vs KiCad fill touching the pad (`spoketruth.py`), before and
  after.

**B. Dog-bone stub (`stitch.rs`, class 1 fallback, after the radial and fine stubs fail).**
- For a pad of a cut piece, the candidate stubs are:
  - leg 1 from the pad centre along one of its 4 spoke axes (pad frame), ending past the pad edge;
  - then straight, or a ±45° or ±90° leg 2;
  - a via of the net-class size at the end.
- The end may also be an existing own-net via (shared via).
- Generated on a 0.05 mm length lattice up to 3 mm + 2 mm. When a via grid is known, also the exact solutions that
  put the via on a grid point.
- Price (graded, nm, cheapest first):
  - length past the pad edge, plus 0.25 mm per 45° bend (0.5 mm per 90°);
  - `via_squeeze` on both layers, foreign-pad proximity, and the antipad-neck price (stitch-vias D2);
  - the BGA area price (100 via prices, power-via rule: shared vias outside BGA ok, under BGA a last resort);
  - the off-grid price of the via (graded `off_grid` × amount, from the grid source stitch-vias r4 exposes; the pad
    end stays at the pad centre). No price for a shared via other than its stub length.
- Legal only: the checker against all routed copper and the stitches placed so far, the mask-opening rule, the
  own-pad and own-copper sliver rules, and the via standing in solid main fill of another pour layer
  (`main_elsewhere`, depth r + ring).
- Stub width: the net width (at most the pad's minor size), necked in steps (0.75, 0.5, board minimum) at a graded
  price (0.5 mm for the whole width). qmtech: the 0.5 mm-pitch rows need 0.188 mm on ADV7511.27 / .37.
- Each part is checked alone (leg 1, via, leg 2); a failing leg fails every longer leg on its ray, a failing via site
  every leg ending there; both remembered, so the 2048 checker queries per pad (32·MAX_TRIES) go to new geometry.
- Placed after routing, against the drawn copper, so it never takes routed copper or planned room.
- No fit: the note "no dog-bone via fits for pad X.N (all spokes cut; boxed in by NET, NET ...)" lists the foreign
  nets the refused candidates hit. It goes into the existing POUR_CUTOFF finding.

Literature:
- TopoR 6.3 manual p. 65: "only fanouts will be placing for nets to internal plane layers". A plane-net pad is joined
  by a fanout via, not by routing.
- Memory, qmtech hand-fix lessons: the user fixed qmtech by hand with GND pad vias and nudges ≤ 0.3 mm.

Replay result (qmtech r3 world, KiCad refill + DRC on the saved copy): dog-bones on U3.3, U3.36, ADV7511.20, .27,
.37, .78; GND unconnected 27 -> 21; DRC error counts unchanged against the A-only arm.

Dev aid found on the way: `tools/stitch_replay_dump.py` wrote pcbnew net codes; the model numbers its nets itself, so
routed copper landed on the wrong nets (the "extra 3V3/1V8 cuts" of the replay). Items are now mapped by net name.

## Invariants
- No new DRC class.
- Every dog-bone via is the class via, checker-legal, and stands in solid fill on ≥ 2 layers.
- Copper is the same for any thread count.
- `stitch_islands = false` gives no stubs.

## Tests
- Synthetic fixture (harness/fixtures.py): a 0.5 mm-pitch pin row with a GND pad between two signal pads whose
  tracks box all 4 spoke ends; GND on F.Cu + B.Cu. The expected answer is the axis dog-bone with its via in the B.Cu
  fill; the radial stub has no legal answer.
- Pattern PNG labelled PROPOSED.
- Engine tests, each with a mutant:
  - the old-spoke relief;
  - the test-point offset;
  - the spoke-width cut;
  - dog-bone axis vs radial;
  - the 45° leg;
  - spoke survival (a track across the spoke inside the relief);
  - facing spokes;
  - the BGA price;
  - the grid price;
  - the shared via;
  - the no-fit warning naming the pad.
- Boards eprom/wiccon/esp/qmtech det s0, base = stitch-vias tip, judged on the saved file (kicad-cli refill + DRC):
  - qmtech: U3.3, U3.36, ADV.20/.27/.78 should leave the unconnected set;
  - per-pad set diff, DRC classes, vias.

## Results (ppv1, det s0, grid 0.254, base = stitch-vias r4 c513c70c `hz-runs/sv4-gonly2`, judged after KiCad's refill)
A (estimate, arm ga = `stitch_dogbone = false`) changes pour completion during routing on qmtech only:
- eprom, esp: copper identical to the base; wiccon: one stitching via 0.254 mm off; all three unconnected unchanged.
- qmtech: the plan differs on 75 nets (plan crossings 83 -> 77); cut pieces before stitching 41 -> 47 (the estimate
  now sees KiCad's cuts), unconnected 19 -> 26; KiCad DRC clearance 75 -> 62, tracks_crossing 3 -> 7, shorting 3 -> 4,
  hole_to_hole 1 -> 0. No new class.
B (arm g vs ga, same plan): only additions (28 items, 10 dog-bones: 8 new vias, 2 shared); DRC counts identical;
qmtech unconnected 26 -> 17 (10 pad/zone pairs gone, 2 regrouped); net against the base 19 -> 17. esp: 25 pads with
no fit (no via site in solid fill of the other layer, or boxed in by +3V3 / +BATT), unconnected unchanged.
Stitching time on qmtech 2.5 -> 18.9 s (shared VM, indicative only).

Pattern `pad_dogbone` (PROPOSED): fanouts routed with kept copper; topor's dog-bone = the expected answer (via at
(10, 6.85)), KiCad DRC 0, unconnected 0. Pattern APIs default a pad's spoke angle to 45 degrees; the pattern sets 90
(the footprint-library default for a rect SMD pad), else the neighbours cut every spoke before routing and the plane
stub (planes.terminals) serves the pad instead.
