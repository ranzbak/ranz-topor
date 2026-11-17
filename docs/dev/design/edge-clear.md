## edge-clear (2026-11-17): track ends off the pad centre, copper-to-edge as KiCad measures it, rule C

### Cause (fx pattern `edge_pads`, main 0a53849, seq and erbs: x = 0.600, 1 copper_edge_clearance each)
0.2 x 0.5 mm pads at x = 0.6, a 0.25 mm track, copper-to-edge 0.5. The plan is right; every run starts and ends at
the pad CENTRE (`draw_wire`, `cutflow::wire_stretches`), where the track's end disc (wider than the pad) is 0.025 mm
into the edge clearance: no legal start node, never-drop forces the straight line at 0.600. Neck cannot help (pad
0.2 < the board's min track width 0.25). Second: `edge_clear()` added half the Edge.Cuts stroke (39e7297).

### Rules
- **A. Pad-end anchor** (`topo::padend`): a run end on a pad ends at the centre unless the end disc there breaks a
  STATIC clearance (board edge, foreign pads/holes/shapes/keepouts; the drawing's own `need_of` / `seg_clear`); then at
  the nearest point inside the pad (inset 1 um, KiCad connectivity: the end in the pad) that keeps them (push from the
  worst violated object; 16-ray bisection fallback). None legal: the centre (never drop prices it, counted). Once after
  the vias, before ERBS and drawing (draw-order independent); used by the drawing's run ends, the octilinear pad-exit
  leg and the ERBS stretch curves (`cutflow::Plan.ends`). Log line "pad ends: ...".
- **B. Copper-to-edge from the Edge.Cuts centre line**: `Rules::edge_clear() = edge_clearance`. KiCad 10 sets the
  stroke width 0 for edge shapes (pcbnew/drc/drc_test_provider_edge_clearance.cpp, tag 10.0.8 l. 295-298:
  `if( item->IsOnLayer( Edge_Cuts ) ) stroke.SetWidth( 0 );`); the 10.0.8 DRC of edge_pads reports "actual 0.4750 mm"
  for the track at 0.600 against a 0.05 mm-stroke edge (no half stroke). Every `edge_clear()` user (checker, obstacles,
  planner corridor `proj.edge_clear`, drawing ents, tidy, ERBS via the packer) and the raw `edge_clearance` users
  (exit stub, ignored-rule factor) agree.
- **C. `[rules] edge_clearance_zero`** (user decision 2026-11-17; `toporlib/edge_zero.py`, applied by
  `plugin/topor/run.py` to the engine's `rules.edge_clearance`), consulted only when the resolved copper-to-edge is 0
  (a non-zero Board Setup / custom value always wins); one value for all layers: `"zone_clearance"` (default) = the
  biggest clearance of the board's copper pours; `"board"` = 0 as KiCad; `"pours"` = the biggest per-layer least
  distance of the fills near the edge. KiCad's filler does not use a zone's clearance for the edge: Edge.Cuts is knocked
  out at `max(PHYSICAL_CLEARANCE, EDGE_CLEARANCE)` with line widths ignored (pcbnew/zone_filler.cpp tag 10.0.8,
  `ZONE_FILLER::buildCopperItemClearances`, lambda `knockoutGraphicClearance`, l. ~2409-2450); the zone local clearance
  enters only `CLEARANCE_CONSTRAINT` (pcbnew/drc/drc_engine.cpp `DRC_ENGINE::EvalRules` l. ~1983), the implicit
  edge constraint is `bds.m_CopperEdgeClearance` (l. 231-232). Measured on refilled wiccon / qmtech: the fills reach
  the edge line (0.00002 / 0.0005 mm), pour clearances 0.508 / 0.254. Every mode keeps the EDGE_CLEARANCE_ZERO warning
  (Board Setup -> Design Rules -> Constraints -> Copper to edge clearance) with the value used; report.md Settings.

### Invariants
- Centre legal => anchor = centre bit-for-bit. The anchor lies inside the pad, its disc legal against static features
  whenever such a point exists; it is only taken when the centre is illegal (never a new violation).
- C is stricter than KiCad or equal, never looser; KiCad's DRC is unchanged by it.

### Tests
Engine `topo_pad_end` (edge_pads x 0.625..0.6251 both modes, corner pad, no legal point, legal centres stay, inset),
`check_clearance::edge_clearance_ignores_the_outline_stroke`; pytest `tests/test_fx_edge_pads.py` (mechfix, KiCad DRC
0 and x optimal, seq + erbs); `tests/plugin/test_edge_zero.py`; run flow `test_edge_zero_rule_c_*`,
`test_edge_non_zero_board_value_wins_in_the_run`; mutants `tools/edge_mutate.py`.

### Follow-up
Neck ladder before the centre fallback when no legal anchor exists (neck the end to the widest width that fits, floor
= min track width, warning): not a small change (the neck is decided by the planner / drawing per stretch, the anchor
per end); counts of "no legal point" ends per board in the edge-clear report.
