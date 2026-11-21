# grid-snap: drawn copper prefers the user's KiCad grid (pad snap first, packing first)

**What.** Track bends and vias land on the user's grid where that costs nothing the plan or the rules
need. User rules 2026-11-21: the trace still ends at the pad centre; where it does not fit, wires go off
the grid "same like in KiCad". So the grid is a graded price, never a cutoff. Order of precedence:
legal > planned room of others > 45 degree form > bundle packing (follow at clearance) > length and
bends > grid.

**Where the grid comes from (facts checked on disk, KiCad 10).** The grid *size* is not stored in the
project. It is an app setting: `~/.config/kicad/<ver>/pcbnew.json` `window.grid` (`sizes[]`,
`last_size`, and in KiCad 9+ the per-item overrides `overrides_enabled`, `override_wires[_idx]`,
`override_vias[_idx]`). KiCad's router uses those overrides for tracks and vias (KiCad master
`pcbnew/tools/pcb_grid_helper.cpp`, `GetItemGrid`: PCB_TRACE_T -> GRID_WIRES, PCB_VIA_T -> GRID_VIAS;
`GetGridSize`: override index when enabled). The grid *origin* is in the board: `(setup (grid_origin x
y))` in `.kicad_pcb` (qmtech: 75.927 45; the others none = 0,0), and over IPC
`board.get_origin(BoardOriginType.GRID)` (kipy 0.8). Source chain, first hit wins, reported in the
result: (1) `[drawing] grid_mm` (number or `[x, y]`) in topor.toml; (2) plugin: the running KiCad
version's `pcbnew.json` (read only), wire grid for tracks and via grid for vias; (3) none: warning
`GRID_MISSING`, grid snap off. Warning `GRID_COARSE` when a grid is > 0.5 mm (still used, priced).
Headless/harness runs only use (1) (the isolated KiCad home has no user grid). Model JSON gets
`grid: {track: [gx, gy], via: [gx, gy], origin: [x, y], source}` in nm.

**Pipeline (abstract first, draw once).** Planned room is not changed: the grid only uses slack.
- *Plan, metric stage:* via sites (`viaopt::relocate_vias`) and branch points
  (`relocate_junctions`) get the nearest via-grid points as extra candidates. The grid price is added to
  the force-balance or gap score. The drawing's `place_vias` tries the planned point first, as today.
- *Drawing, the octilinear DP (`octi.rs`):* first draw, ERBS form and forced draw all use it, so this is
  one price on the DP scale (user rule A1c). (a) Nodes: each taut vertex and each octagon node that the
  path wraps gets one grid companion. That is the nearest track-grid point not nearer to the corner's
  obstacle, if it lies in the strip and is clear. Companions have their own cap, so the old node set is
  unchanged. (b) Connectors: an elbow between two grid nodes bends on the grid. For square grids the Z
  shape also gets a variant whose middle leg is moved to the grid step nearest the centred one. (c)
  Price: every bend vertex the DP pays a `bend_price` for also pays
  `off_grid x min(1, d / d_max)`. Here d is the distance to the nearest grid point and
  d_max = half the grid diagonal. Pad-centre ends and fixed points (`keep`, via centres) are exempt.
  New `[factors] off_grid`, default weight 0.15 mm. That is below one 45 degree bend (0.25 mm), so the
  grid never buys a bend or a jog. It is also below the bundle `pull` of any leg longer than about
  0.5 mm, so a bundle stays packed. (d) The early return for "taut run already all-octilinear" is also
  skipped when grid snap is on and an interior vertex is off the grid. That costs extra DP calls; this
  will be measured.
- *Last spacing fallback (user 2026-11-17):* the same companions, offered only in a second DP pass
  when the first pass leaves an any-angle stretch or a forced violation in the run.

**Option** `[drawing] grid_snap = "off" | "fallback" | "prefer"`. `off` gives copper bit-identical to
main (hash-checked). The default is decided after measuring. Also `[drawing] grid_mm` (default
`"auto"`). All keys documented with KiCad pointers (View -> Grid Properties, Board Setup has none;
grid origin: Place -> Grid Origin).

**Off-grid survivors, by reason** (result.json `grid` block plus one warning line):
`pad_leg` (the bend joins a leg through an off-grid pad centre; KiCad gives the same), `packed`
(follows a neighbour at clearance), `room` (the companion is in planned room or not clear),
`any_angle`, `forced`, `fixed` (T foot, polish). Measured on the final items after tidy:
fraction of vertices exactly on the grid (integer nm), plus the fraction within 5 um.

**Data from the hand layouts** (cheap parse, no runs). Vias are mostly on a grid: eprom 100 % on
0.05 mm, esp 82 % on 1 mil, qmtech 57 % on 0.05 mm from its origin. Track vertices mostly are not
(<= 12 %; esp 39 % on 1 mil). So KiCad hand routing itself leaves bends off the grid next to pads and
in shoved bundles. Expect the gain mainly at vias and on free legs.

**Literature.** TopoR routes gridless. It snaps to the grid only in manual routing (TopoR 6.3 manual
p. 238, FAQ "grid-based and orthogonal routing"), and its editor flags "Not in a grid" (TopoR 5.0
help, options_routing). So this is KiCad-driven, not a TopoR method step. Octilinear node/port model:
Bast, Brosi & Storandt 2020 sections 3-4. KiCad's pad anchor ranks above the grid
(`pcb_grid_helper.cpp` `computeAnchors`: pad centre = ORIGIN | SNAPPABLE anchor, object tier).

**Invariants.** off = main copper. With prefer, the DP candidate set is a superset of off's, and
the chosen shape's non-grid price <= the off best + the off best's grid price. Pad-centre ends are
never moved. No new DRC class, KiCad connectivity unchanged (set per net). Copper is identical for any
lane or thread count (pure function).

**Tests.** A unit test of the grid distance, the companion point (outward, clear) and the price (square
and rectangular grid, origin offset). A test of the source chain (toml, pcbnew.json overrides, missing,
coarse; Python). New pattern `grid_snap_bends` (harness/drc_patterns.py, PROPOSED): two headers on a
2.54 mm grid with one free route around a pad (bends expected on the grid), a 0.4 mm-pitch bundle
through a passage (off the grid, `packed`), and a metric 0.5 mm-pitch SOIC (`pad_leg` survivors). It
has a PNG with the grid drawn and the survivors marked. Mutants of the companion, the price, the
early-return change and the exemptions. Ladder eprom/wiccon/esp/qmtech at det s0, rounds=1, grid
0.254 mm (the user's live KiCad grid; origin from each board), off vs prefer (+ fallback):
DRC/unconnected/vias/detour/runtime/on-grid fraction.

**Run budget** (<= EUR 0.60, one ccx43 hour): 1 build, 3 units (tip off with hash vs cached main;
tip prefer, 4 boards; tip fallback, 4 boards) plus 1 pattern/render unit.
