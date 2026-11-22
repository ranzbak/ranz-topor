# stitch-vias: stitching vias after routing, before the zone refill

Status: DESIGN, for the lead (2026-11-21). Branch stitch-vias off main bbd6e0f1.

## What exists (D4, `topo/stitch.rs` + `pourcheck.rs`)
`pourcheck` rasters every (pour, layer) fill as KiCad would refill it, labels the islands, and
joins islands, pads, vias and tracks by union-find. Islands KiCad drops are labelled too.
`stitch` fixes only the **cut components KiCad keeps** (a pad, via or kept island off the
main component). Each one gets **one** via in the island, ranked by nearness to a pad plus the
`via_squeeze` slack price. Fallbacks are a pad stub and an island join. Pieces KiCad drops,
thin necks and plain area stitching are not handled. Measured on main: wiccon 5 pieces get 5
vias. qmtech 42 pieces get 26 stitches, 16 stay open (U3 rows, ADV7511 .18/.20/.37, U5.9,
C32.1: mostly pads with **0 islands**, every spoke cut).

## What is new: one region-targeted lattice stage (the lead's direction)
A **piece** is an island of one (pour, layer) in the refill estimate. Its **site area** is
the part of the piece where a via of the net-class size (first of `via_sizes`) fits. That
means fill depth ≥ r_via + max(zone min width, min_connection) on this layer, **and** on
another layer of the net's pours in that layer's main component. A piece with no other
pour layer under it gets no via (no dangling vias).
Candidate sites are lattice points at pitch `p`, in the KiCad frame, at multiples of `p` from
the board origin (`p` a multiple of 0.05 mm, so the points sit on the usual user grids).
Sites off the lattice, from the cell-lattice search D4 uses today, are a fallback only. They
pay a graded offset price.

Site price (graded, no cutoffs):
- `via_squeeze(slack)` on both layers (existing factor);
- foreign-pad proximity, `k·max(0, 1 − d/1 mm)²` (keeps pin rows and fanout room free);
- a BGA area adds `bga_share_cost` via prices (power-via rules: under a BGA = last resort);
- lattice offset (fallback sites only).

Legality is the existing checker plus the `own_pad_ok` / `own_copper_ok` sliver rules.
Choice per piece: greedy farthest-point. The next via is the site that maximises
(distance to the nearest via already placed or own-net via/THT pad in the piece) minus price.
This spreads vias for redundancy. Deterministic tie-break by (x, y).

Classes, in this order (one pass each, refill estimate redone between classes):
1. **cut** (KiCad keeps it, off main; today's D4): `n = max(min_vias, ceil(A/p²))`, with
   `min_vias` = 2 (lead) where room allows. A single via is still placed and reported.
   Pad stub and island join stay as fallbacks when the piece has no site.
2. **dropped** (pad-less island KiCad would remove): stitch it so the copper stays, with the
   same n rule. The piece counts only if `A ≥ p²/4` (option). Below that the via is not worth
   it, and the piece is reported.
3. **thin** (in main, but held by a neck): erode the main island by `thin_mm/2` (the EDT
   depth exists already). A core component with no own-net via or THT pad in it gets n vias.
4. **area** (option, default off): every lattice site of the main pour farther than `p` from
   any own-net via gets one. This is classic stitching, like Altium's via-stitching tool.

Runs: class 1 stays per variant (it is part of judging: open items). Classes 2-4 run **once
on the picked result** in `stitch_pours` before the hand-off, so the plugin's refill sees
them. They count in the result's via total under the class "pour stitching". They do not
change which variant is picked.

## Round 2 additions (lead 2026-11-22)

- **Solder-mask openings (F1, KiCad DRC is the spec).** Both exporters list graphics on F.Mask / B.Mask as
  `mask_openings` (copper layer exposed, polygon; pads' own openings are not graphics). Per opening the stage
  collects the nets whose copper it exposes (pads, kept tracks, vias, routed copper). A site whose via disc
  overlaps an opening that exposes another net is skipped (a through via shows on both outer layers), and a
  dropped island that overlaps such an opening on its layer is not stitched (keeping it would expose the net
  next to the other one: `solder_mask_bridge`). wiccon: solder_mask_bridge back to the base 3.
- **Antipad necks in other nets' pours (D2).** A site inside another net's pour fill pays
  `pitch · (1 − neck/thin)²` per such pour layer, neck = fill depth − via radius − the pour's clearance,
  thin = `stitch_thin_mm` (1 mm when 0). Graded: classes 2-4 skip a site whose neck goes to zero (price = the
  value of a via), class 1 takes it last.
- **`stitch_per_area`** (default true): false = every piece gets `stitch_min_vias` only (the D1 arm).

## Round 4: on the user's KiCad grid (lead-approved 2026-11-22)
User 2026-11-22: "We can wait for snap to grid to be completed, because the vias should use that as
well".
- **One grid source (G1).** The plugin resolves the grid with grid-snap's `topor/grid.py resolve`
  ([drawing] grid_mm, else KiCad's pcbnew.json, the via grid override first) also when grid_snap is off
  and stitching is on (`run.grid_run_options`). The engine reads `Options::grid_via_nm` (else
  `grid_track_nm`) and `grid_origin_nm`; `BoardModel.grid_origin` and the two exporter copies are gone.
  The drawn copper still uses the grid only with grid_snap on (gridsnap gates on the mode).
- **`StitchGrid::grid_at(p)`** returns the grid at a position. Today one grid holds everywhere; BGA-GRID
  (a courtyard's ball array) plugs in there without touching `place_spread`.
- **Lattice on the grid (G2).** The x/y pitch is rounded to a multiple of the via grid (at least one
  step). A changed pitch is named in the run log and in an engine note. The staggered shift is half the
  x pitch rounded to the grid. The origin is the grid origin, so every lattice site is a grid point.
- **Fallback and stub sites (G3).** A raster fallback site offers the via-grid corners of its cell, each
  through the same site tests. It stays itself at an off-grid price of k·d + off_grid·amount
  (d = its distance to the nearest grid point). k is above the steepest slope of every smooth price
  term (distance, lattice offset, squeeze on two layers, foreign-pad proximity, plane neck), so no
  smooth difference lets it beat a legal grid point near it; a step price (a BGA) still can. With no
  legal grid point in its cell it pays off_grid·amount only, so the piece still gets its via. The
  engine notes how many vias ended off the grid and why (no legal grid point near, or refused). A pad stub's via tries the nearest grid point before the ray point. *(Superseded in r5b: off the via grid never, see Round 5.)*
- **`stitch_on_grid`** (default true). No grid found = a warning, and the behaviour before round 4.
- **One graded site price (verifier V1/V2).** `place_spread` no longer ranks lattice sites first: a
  fallback site pays `LATTICE_OFFSET_WEIGHT` (2.0) times its distance to the lattice. Because the weight
  is above 1, a legal and unpriced lattice point still always wins. A BGA price (100 via prices), a plane
  neck or a squeeze now outweighs the lattice: V1 puts the class-1 via near its pad and outside the BGA.
  A site priced at or above the value of a via is skipped instead of ending the piece (V2).
- Own-net copper refusals no longer count towards `MAX_TRIES`, as before stitch-vias.

## Round 5: deliberate placement (user 2026-11-22: "looks like random placement, I expected either for the placer to search for the corners, or to have a placement grid")

**RCA of today's off-lattice vias** (wiccon g arm at c513c70c, notes of result.json: 30 of 84 stitch vias on
the lattice, 54 on the via grid only). The lattice itself is one board lattice (`Raster::lattice_sites`,
pourcheck.rs:160, origin = grid origin): there is no per-piece offset. The causes:
1. **Seed at a target** (14 of 20 pieces: the first via off the lattice). Class 1 places its first via at
   min(distance to the piece's pads + price), class 2 at the island's `at` point; a grid point beside the pad
   beats a lattice point a few mm away (offset weight 2 < the distance gained).
2. **Saturated spread**: the next vias maximise min(d, pitch) − price. Every site at least a pitch from the
   seed ties at "pitch", so a raster/grid point priced 2·offset wins over a lattice point nearer than
   pitch − 2·offset to an off-lattice seed: the rows drift with the seed.
3. **Count above the lattice**: `want = ceil(A/p²)` counts the whole piece, the legal lattice points lie in
   the site area (eroded by via radius + min width): the shortfall is filled from the raster's grid corners
   (G3), i.e. wherever a STEP cell's corner lands.
4. Checker refusals (MAX_TRIES) take the next best site, usually a raster point beside the refused lattice
   point: counted on the boards in this round (`blocked` in the notes).

**Rule (classes 1-4, one function `place_deliberate`)**
- **Lattice fill**: a piece whose site area holds at least `stitch_min_vias` legal board-lattice points gets
  vias on lattice points only (at most `want`; farthest-point among lattice points when fewer are wanted).
  Rows and columns line up across pieces because the lattice is the board's.
- **Extremities** (minimum-via repair): a piece with fewer legal lattice points gets its vias at its
  extremities: farthest-point sampling over the legal **via-grid** sites of the piece, seeded at the site
  farthest from the piece's centroid, no saturation (score = distance to the nearest chosen via or seed −
  site cost). A rectangle gets opposite corners first, then the other two; a strip or a thin-neck core its
  two ends. Never off the via grid (r5b, user 2026-11-22: "Off-grid: NEVER"): see below.
- Lattice fill placing fewer than the minimum (refusals): the rest at the extremities, anchored on the vias placed.
- **Blocked lattice point** (site tests fail or the checker refuses): `[planes] stitch_nudge_mm` (new,
  default 0 = drop it). > 0: the via may move to a via-grid point at most that far, priced
  2·distance on top of its cost (graded, cheapest first); never to an arbitrary raster point.
  Default drop: the share "on the lattice or at a corner" is then 100 %, and the minimum per piece is kept
  by the extremity rule, so a drop never loses a piece.
- `stitch_min_vias = 1` with a one-via piece keeps the D4 rule (first via near the pad), as the option's
  doc says ("1 = the behaviour before stitching vias"): stitching off stays bit-identical.
- Prices stay graded (squeeze, proximity, BGA, plane neck); a corner moves inwards while the squeeze price
  falls faster than the distance it gives up (a via with a full diameter of slack, as a designer insets it).

**r5b (user answers 2026-11-22)**: blocked lattice point = drop (default stays 0); neck price stays graded;
**off the via grid never**. With a via grid, `Ctx::sites` offers only grid points (a raster site gives the
grid points of its cell; the raster point itself is never a site, a legal one is only counted), the
extremity rule has no off-grid fallback, and a pad stub ends on the nearest grid point of its ray. The r4
off-grid price (k·d + off_grid·amount, `Site::off`, the "off the via grid" notes) is gone. A piece that gets
fewer vias than wanted carries the reason in its note, naming the piece: "no more legal site on the via
grid G x G mm; N legal spot(s) between grid points not used: stitching vias never leave the via grid",
or "fewer legal board-lattice points than wanted", or "no spot of the piece has room for a via that also
stands in the net's fill on another layer" (pattern piece E: 1 of 2 on the 10 mil grid, its left pocket
lies between the grid columns 2.286 and 2.540). Without a grid (stitch_on_grid false, or none found) the
raster point is the site, as before. `stitch_area` (user: "the option to put them everywhere on the pcb")
uses the same lattice-only rule (`Ctx::place_lattice`, value = pitch so a BGA site is never worth a via,
blocked points dropped).

**Tests**: pattern fixture extended (PROPOSED) with a small piece (no lattice point: 2 corner vias) and a strip
(two ends); engine tests: corners of a rectangle, two ends of a strip, lattice-only fill with a blocked point
dropped / nudged ≤ stitch_nudge_mm on the grid, D4 single via unchanged; mutants on each.

## Options (`[planes]`, documented in schema.py + docs/user/configuration.md)
- `stitch_islands` (exists) = class 1;
- `stitch_min_vias` = 2;
- `stitch_pitch_x_mm` = `stitch_pitch_y_mm` = 5.0, `stitch_lattice` = "staggered" (r3, user 2026-11-22: x and y spacing, straight or staggered = odd rows shifted by x/2; default staggered, user 2026-11-22 "Default is staggered, I like it more");
- `stitch_dropped` = true;
- `stitch_thin_mm` = 1.0 (0 = off);
- `stitch_area` = false;
- `stitch_on_grid` = true (round 4).

The KiCad pointer for these is zone properties (min width, island removal) and Board Setup →
vias. Defaults decided after cost: the via count added per board.

## Literature
- TopoR does not stitch: TopoR 6.3 manual p. 121 (islands are left to the user), p. 184 (pour
  last), and the TopoR 5.0 help (plane integrity not verified).
- Pitch: ST AN5923 (RF layout for STM32WL3x) gives the stitching via distance as
  λg/20 ≤ D ≤ λg/10. Altium's "Via Stitching & Via Shielding" doc cites "no further apart than
  λ/10 and preferably as often as λ/20". The same doc says sites that would break a design
  rule are skipped. That is the lattice-plus-checker model used here.
- 5 mm ≈ λg/20 at about 1.5 GHz in FR4. AN5923's page is not checked locally: the download
  timed out. I will add it with the page number or drop it.

## Invariants
- No new DRC class.
- Every via: net-class size, checker-legal, own-net sliver-free, standing in solid fill on at
  least 2 layers of its net (never dangling).
- Copper is the same for any thread count.
- Off (`stitch_islands=false`, classes off) = main copper bit-identical.

## Tests
- Synthetic fixture (harness/fixtures.py, 2 layers, GND on both). F.Cu is cut into:
  - a padded cut piece;
  - a pad-less piece;
  - a thin-neck piece;
  - a piece with B.Cu removed under it (expect 0 vias).

  Expected answer is lattice vias, ≥ 2 per piece. Pattern PNG labelled PROPOSED goes to the
  user.
- Engine tests per class, each with a mutant: n rule, farthest-point spread, no dangling,
  BGA price, lattice alignment.
- Boards eprom/wiccon/esp/qmtech, det seed 0. Saved file → kicad-cli refill + DRC. Compare
  the unconnected set per net/pad, the isolated-copper items, the DRC classes, vias added,
  and the filled area per (net, layer), before → after. Bases are cached (fg1-5571d1b,
  fd45r2-base). The wiccon F.Cu GND pieces are the first board case.

## Not in this branch
- Prevention pricing.
- bornhack.
- The qmtech 0-island pads (U3, ADV7511 rows) need a dog-bone pad stub (out along the pad
  axis, then 45° to a via). That is a separate item after per-item RCA. I ask first.
