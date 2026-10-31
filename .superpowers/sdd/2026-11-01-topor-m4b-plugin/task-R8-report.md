# Task R8 report: M4-R8 conservative IPC approximations, exact via free flag, keep_unconnected_vias

Branch: worktree-agent-ab46e98980f15a66a (fast-forwarded to main 8b2a7b6 first; the worktree base predated B5-B10).
Status: DONE_WITH_CONCERNS (all ruling assertions met on every snapshot board and the synthetic sets; the concerns
are UNVERIFIED live-server behaviour and three small non-conservative corners that no snapshot board has, listed
under LIMITATIONS).

## What changed, per file
- `toporlib/geom.py`: circumscribed (outward) geometry, next to the unchanged inscribed functions (harness and preview
  keep using those, so their behaviour does not change): `tangent_arc_pts` (every edge on a tangent of the circle),
  `chord_arc_pts` (vertices on the arc, for concave sides), `arc_geometry`, `circle_ring_outward`,
  `capsule_ring_outward`, `arc_stroke_rings` (one annular-sector polygon with tangent caps; capsule fallback when
  radius <= half width), `polyline_stroke_rings`, `bezier_stroke_rings` (per-chord offsets: w/2 + err/3 on the
  convex side, w/2 + KiCad's flattening error on the concave side, two-radius discs at the joints, plain discs at
  the ends), `flatten_arc_outward`. Every one states its Big-O.
- `plugin/topor/model_ipc.py`:
  - `shape_to_polys` rewritten: every copper graphic CONTAINS KiCad's shape. Our vertex error is
    `min(max_error, OBSTACLE_ERR_NM = 500 nm)` (see "Decisions"). Rectangles: corners normalised, KiCad 10
    `corner_radius` honoured; filled shapes grown by half the stroke with round corners (the old square growth
    exceeded KiCad's shape by 0.41 x w/2); filled polygons now include their stroke (it was ignored = smaller
    than KiCad's).
  - `ring_pts(..., outward=True)`: an arc node bulging out of the ring is flattened by its tangent polyline, one
    bulging in keeps vertices on the arc; used for rule-area keepouts and filled polygons.
  - Copper text: `text_margin` / `text_box_poly` / `TEXT_BBOX_GROW = 0.5` (x the larger text size): KiCad's
    bounding box is grown, because it does NOT contain the glyphs (numbers below).
  - `via_free(uuid, flags)` + `export_model_ipc` reads `free` from `toporlib.pcbfile.via_free_flags(pcb_copy_text)`;
    a via not in the copy -> True. The "no track end touches it" guess is gone.
  - `pad_sample_points`: all 9 probes lie inside the model pad (a circle / oval / roundrect corner probe is pulled
    in onto the rounded edge by bisection; poly pads keep only probes inside the hull, pulled 2 % inward).
  - `zone_to_pour`: a zone without filled polygons still stands in with its outline (warning) but reaches no pad
    or via ("not reached").
  - Module docstring: the approximation list with directions (= LIMITATIONS below).
- `toporlib/pcbfile.py`: `via_free_flags(text) -> {uuid: bool}`; KiCad 10 `(free yes)` + `(uuid "…")` (checked on a
  pcbnew re-saved copy), legacy `(free)` + `(tstamp …)` (wiccon snapshot), `(free no)`, top-level vias only.
- `plugin/topor/scope.py`: `rip_plan` keeps a via when it is free OR (`[run] keep_unconnected_vias`, default true) no
  same-net `kept` track end lies within the via radius on a copper layer of its span (`_touched_vias`, per-net
  index, O(V + K + sum v_n k_n)); a via without a position is kept. `RipPlan.kept_unconnected_vias` counts them.
- `plugin/topor/schema.py`: the one new key `[run] keep_unconnected_vias` (bool, true, doc verbatim from the brief,
  KiCad place "KiCad → via Properties → Free"). `docs/user/configuration.md` regenerated (`--check` clean).
- Tests:
  - NEW `tests/plugin/test_ipc_conservative.py` (offline, pcbnew on the test side only; every snapshot board loaded
    from a tmp copy, never cached across tests): copper graphics of all 6 snapshot boards + 79 synthetic shapes at
    max_error 1500 and 5000 (segment, arcs small / tight / > 180 deg / clockwise, circles and rectangles filled /
    unfilled, KiCad 10 corner radius, rotated rectangle -> polygon, concave polygon filled / unfilled, Bezier, three
    widths, open shapes rotated 37 deg, filled polygon without stroke); texts (snapshot + 480 synthetic: stroke and
    Noto Sans font, bold+italic, 0/30/90/217 deg, mirrored on B.Cu, knockout, strokes 0.02..0.8 mm, sizes 0.5..5 mm
    incl. wide, descenders / accents / brackets / multi-line) + a test documenting that KiCad's box alone is not the
    copper; fills polygon-equivalence + pour reach subset (one refill per board); via free flags == GetIsFree.
  - `tests/plugin/test_ipc_parity.py`: live comparisons now assert directions (copper shapes IPC ⊇ harness within
    1 um, fills symmetric difference <= 1 um^2, pour reach IPC ⊆ harness, via free exact, pads M4-R5); skewed-mid
    arcs are compared with pcbnew's polygon of the true arc (`with_true_arc_reference`); two new offline self-tests
    (directions; the skewed-arc replacement).
  - `tests/plugin/test_model_ipc.py`: shape test updated to the circumscribed bounds; new probe-inside-pad test, new
    free-flag-from-copy test; unfilled-zone reach expectation changed to "reaches nothing" (spec change by M4-R8).
  - `tests/plugin/test_scope.py`: `tiny()` vias / tracks got positions so v1 / v3 are routing vias (the existing
    policy assertions are unchanged); new `test_rip_plan_keep_unconnected_vias` (stitching via kept, free kept,
    routing via ripped, rim touch ripped, other-net end / out-of-radius end / blind span all untouched, key false
    rips the untouched unflagged ones but never a free one, position-less via kept).
  - `tests/plugin/test_pcbfile.py`: `via_free_flags` token forms. `tests/plugin/test_kipy_surface.py`: pins
    `Board.get_text`, `get_item_bounding_box`, `BoardText.attributes`, `TextAttributes.size`,
    `GraphicRectangleAttributes.corner_radius`.

## Measured numbers
Copper graphics (C25), worst over all shapes: pcbnew polygon minus (IPC + 1 um) = 0 nm^2 (board max_error and 20 nm
fine reference), IPC minus (pcbnew + max_error + 1 um) = 0 nm^2:
bornhack2026 77 shapes, dcsgonebadge 71, nametag-sao 1, eprom / wiccon / wedding-gift 0; synthetic 79 x 2 max_errors.
Copper text: 480 synthetic + bornhack2026's 1 knockout text, glyph / knockout copper outside the grown box = 0 nm^2.
KiCad's ungrown `GetBoundingBox()` misses up to 0.33 x the text size (stroke font, 0.02 mm stroke, "ÅÉ ÿ"); hence 0.5.
Fills: symmetric difference 0 nm^2 on every zone layer (bornhack 7 layers 11 391 mm^2, dcsgone 2 / 7 973, eprom 2 /
1 070, nametag 2 / 24 514, wedding 4 / 13 343, wiccon 2 / 17 355).
Pour reach (C26), IPC-only = 0 on every board; "pcbnew reached, IPC not" (allowed): dcsgonebadge pads 2 vias 2, all
others 0 (bornhack 151 pads / 750 vias reached, eprom 4 / 17, nametag 14 / 35, wedding 28 / 0, wiccon 28 / 11).
Via free (C23): parsed == GetIsFree on all boards, re-saved KiCad 10 text and original snapshot text (bornhack 870 vias
/ 680 free, dcsgone 203 / 2, eprom 17 / 17, nametag 50 / 35, wedding 0, wiccon 36 / 11 (legacy file)).
Run flow (FakeBoard + harness model, existing_copper = rip, Reject; kicad-cli DRC), keep_unconnected_vias true / false:

| board | vias / free flag | vias ripped | newly kept by C24 | routed | new vias | routing_new | unconnected_new | unconnected items |
|---|---|---|---|---|---|---|---|---|
| eprom-to-kernal | 17 / 17 | 0 / 0 | 0 | 27/27 | 0 | 0 | 0 | 0 |
| wiccon-2023 | 36 / 11 | 25 / 25 | 0 | 62/62 | 37 | 0 | 0 | 0 |

Identical with the key true and false: on these two boards every unflagged via is a routing via.

## Decisions (please review)
- **Our obstacle error 500 nm, not max_error.** KiCad's ERROR_INSIDE polygon may sit up to max_error inside the true
  curve; ours sits up to e outside. The bound "IPC ⊆ pcbnew + (max_error + 1 um)" (C25) then needs e + max_error <=
  max_error + 1 um, i.e. e < 1 um; with e = max_error the measured excess was up to 6e9 nm^2 per shape. 500 nm keeps
  0 excess everywhere. Cost: more vertices per copper graphic (e.g. a 0.25 mm track cap 18 instead of 6 segments);
  graphics are few. It is an internal precision constant, not a topor.toml key.
- **Arcs with a skewed mid point** (stored mid != angular midpoint; bornhack2026 AE1 has two, 180.1 deg): pcbnew's
  `TransformShapeToPolygon` of such an arc is NOT the arc (off-centre by ~5.5 um), while KiCad's DRC shape
  (`GetEffectiveShape`) IS the true arc (probed at +-0.3 um of the stroke edges over the sweep,
  `assert_drc_shape_is_the_arc`). The IPC side builds the exact arc from start / mid / end; the tests compare it
  with pcbnew's polygon of the same arc drawn with its true midpoint. Synthetic arcs use true midpoints for the same
  reason (KiCad writes them so).
- **Bezier**: KiCad strokes its own flattening; the obstacle is grown by max_error on the concave side only, so both
  ⊇ and the excess bound hold (a symmetric growth contained KiCad's polygon but broke the excess bound).
- **Text margin is relative** (0.5 x size, not a constant): the deficit of KiCad's box scales with the size.
- **Unfilled zone reaches nothing** (was: everything inside its outline): the conservative direction of M4-R8.
- **Probe points pulled inside round pads**: the old diagonal probes of a circle / oval / roundrect pad lay outside
  the copper. Measured counts were already 0 IPC-only before and after; this is the "stricter, never looser" rule.
- `run.py` (QB12's file) is not touched: `RipPlan.kept_unconnected_vias` is available for the run log / report.
- `keep_unconnected_vias` affects only `rip_plan` (plugin); the harness's own `rip_tracks` is unchanged.

## UNVERIFIED (live server)
- `get_item_bounding_box(text)` = pcbnew `GetBoundingBox()` (the margin is sized for that box).
- How the server serialises a KiCad 10 hatched fill (kipy 0.8 has only filled / unfilled), arc mid points (stored
  vs computed), Bezier flattening tolerance used by KiCad's DRC, footprint fields on copper layers.
- `test_ipc_parity` live run (user, GUI checklist).

## Tests run
- `.venv/bin/pytest -q tests/plugin` -> 212 passed, 7 skipped (6 live parity + 1 pre-existing) in 355 s.
- `.venv/bin/pytest -q` default suite -> see FULL_SUITE line at the end.
- `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check` -> "configuration reference is up to date".
- Run-flow measurement: scratch script driving `topor.run.run` exactly like tests/plugin/test_run_flow.py (numbers above).

## LIMITATIONS
Remaining approximations of the IPC model exporter (`plugin/topor/model_ipc.py`), each with its direction. "⊇" = the
obstacle contains KiCad's copper (conservative); "not reached" = pour reach errs toward fewer reached items
(conservative); NOT conservative items are marked and none occurs on the snapshot boards.
1. Copper graphics (segments, arcs, circles, rectangles incl. corner radius, polygons): circumscribed polygons,
   vertex error min(max_error, 500 nm) -> ⊇; excess <= 0.5 um over the true shape (<= max_error + 1 um over KiCad's
   polygon). Filled polygons: holes ignored -> ⊇ (excess = hole area).
2. Copper Bezier curves: grown by max_error on the concave side -> ⊇ KiCad's polygon at the board max_error; if
   KiCad's DRC flattens Beziers more coarsely than max_error, up to that difference is not covered (NOT conservative
   in that case, UNVERIFIED; no snapshot board has a copper Bezier).
3. KiCad 10 hatched fills: kipy 0.8 has only filled / unfilled; a hatched shape arriving as unfilled is only its
   outline stroke (NOT conservative, UNVERIFIED; none on the snapshot boards).
4. Copper text: KiCad's bounding box grown by 0.5 x the text size -> ⊇ glyphs / knockout box (measured worst need
   0.33 x); excess up to the margin on every side. Footprint fields on a copper layer are not exported (as in the
   harness; NOT conservative; none on the snapshot boards).
5. Pour fills: KiCad's filled polygons as they are (exact); a zone without filled polygons uses its outline and
   reaches nothing -> "not reached".
6. Pour reach: probes strictly inside the model pad in the same-net fill, vias by their centre -> "not reached";
   custom / trapezoid / chamfered pads are their convex hull, so a probe may lie outside a concave pad's copper
   (possible over-reach; 0 cases on the snapshot boards). kicad-cli DRC stays the oracle (M4-R6).
7. Via free: KiCad's exact flag from the save_as copy; a via not found -> free (kept, fixed obstacle).
8. Pads: exact (M4-R5); custom / trapezoid / chamfered / offset pads -> convex hull of KiCad's polygon (⊇, holes
   ignored).
9. Keepouts: arc edges flattened outward (⊇). Board outline: segments / arcs / rectangles exact; circles / Beziers /
   polygons flattened with vertices on the curve (an Edge.Cuts circle used as an internal cut-out is up to
   max_error smaller: NOT conservative by <= max_error, far below any edge clearance).
10. rip_plan (`keep_unconnected_vias`): "touched" = a same-net track END within the via radius on a layer of the
    via's span; a via that a track only passes through is kept (conservative: kept vias are fixed obstacles).

## Concerns
- Live behaviour (text bbox, hatched fills, Bezier tolerance) is UNVERIFIED; items 2, 3, 4 (fields) and 9 are the only
  non-conservative corners, none present on the six snapshot boards.
- The live parity test now needs pcbnew SHAPE_POLY_SET (harness venv, as before) and imports helpers from
  test_ipc_conservative.py.

FULL_SUITE: .venv/bin/pytest -q -> 379 passed, 7 skipped, 22 deselected in 618 s.
