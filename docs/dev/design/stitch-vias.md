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

## Options (`[planes]`, documented in schema.py + docs/user/configuration.md)
- `stitch_islands` (exists) = class 1;
- `stitch_min_vias` = 2;
- `stitch_pitch_x_mm` = `stitch_pitch_y_mm` = 5.0, `stitch_lattice` = "straight" (r3, user 2026-11-22: x and y spacing, straight or staggered = odd rows shifted by x/2);
- `stitch_dropped` = true;
- `stitch_thin_mm` = 1.0 (0 = off);
- `stitch_area` = false.

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
