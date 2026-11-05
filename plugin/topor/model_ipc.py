"""BoardModel exporter over kicad-python (kipy 0.8): the IPC twin of ``harness.model.export_model`` (same
schema: ``units, board, layers, nets, pads, vias, kept, copper_shapes, holes, outline, keepouts, net_ties,
pours, dangling_ends, rules``; KiCad frame, Y down, integer nm). No pcbnew (M4-R1).

Everything that only reshapes data is a pure function over kipy wrappers (unit-tested with wrappers built from
protos): ``pad_from_kipy``, ``shape_to_prims``, ``shape_to_polys``, ``zone_to_keepouts``, ``zone_to_pour``,
``track_to_kept``, ``via_to_model``, ``layer_plan``. Only :func:`export_model_ipc` talks to a live ``Board``;
the live-only calls are marked ``UNVERIFIED`` (get_pad_shapes_as_polygons, get_item_bounding_box,
get_enabled_layers, get_zones filled polygons, ...). The kipy-free pour-reach / dangling-end geometry
(``pad_contains``, ``pad_sample_points``, ``pour_reach``, ``dangling_ends``) lives in ``topor.pourreach`` and is
re-exported here (gate review MINOR 3).

Conventions (documented deviations from the harness, see the B3 report):
  * net codes are ``toporlib.rules.net_codes_by_name`` (sorted names, 1-based) of ONE net list, passed to both
    the rule resolver and the exporter (``IpcBoard.net_codes``, gate review MINOR 13);
  * pad rotation is ``padstack.angle`` (absolute, as kipy's own footprint-rotation code treats it);
  * keepout layers are the copper layers only.

Approximations (M4-R8: allowed only when CONSERVATIVE; each is tested in that direction in
tests/plugin/test_ipc_conservative.py and, live, tests/plugin/test_ipc_parity.py). "⊇" = the IPC obstacle
contains KiCad's copper; "not reached" = pour reach errs toward fewer reached items:
  1. copper graphics (segments, arcs, circles, rectangles incl. KiCad 10 corner radius, polygons): circumscribed
     polygons (tangent construction), vertex error ``min(max_error, OBSTACLE_ERR_NM)`` -> ⊇, outward excess
     <= 0.5 um over the true shape (<= max_error + 1 um over KiCad's own polygon). Arcs use the exact circle of
     start / mid / end, i.e. KiCad's DRC arc, also when the stored mid point is not the angular midpoint (KiCad's
     own polygon of such an arc is off-centre by a few um; bornhack2026 AE1 has two). Filled polygons: holes
     ignored (⊇, excess = the hole).
  2. copper Bezier curves: KiCad strokes its own flattened polyline; the obstacle is grown by max_error on the
     concave side of the bend -> ⊇ KiCad's polygon at the board max_error. If KiCad's DRC flattens a Bezier
     more coarsely than max_error, up to that difference is not covered (UNVERIFIED; no snapshot board has a
     copper Bezier).
  3. hatched fills (KiCad 10): kipy 0.8 knows only filled / unfilled; a hatched shape that arrives as unfilled
     becomes its outline stroke only, the hatch lines are missing (NOT conservative; UNVERIFIED how the server
     serialises it; no snapshot board has one).
  4. copper text: KiCad's bounding box (``get_item_bounding_box``, UNVERIFIED = pcbnew ``GetBoundingBox()``)
     grown by ``TEXT_BBOX_GROW`` x the text size -> ⊇ glyphs / knockout box (KiCad's box alone is not: it misses
     descenders and accents by up to 0.33 x the size). Excess: up to the margin on every side. Footprint
     fields placed on a copper layer are not exported (as in the harness; NOT conservative; none on the
     snapshot boards).
  5. pour fills: KiCad's ``Zone.filled_polygons`` as they are (polygon-equivalent, symmetric difference 0 on
     every snapshot board). A zone without filled polygons stands in with its outline (warning) and reaches
     nothing -> "not reached".
  6. pour reach: probe points strictly inside the model pad (centre + 8 points at 98 % towards the shape's
     boundary) inside the same-net fill; a via when its centre is -> "not reached" (thermal-relief vias,
     spokes between probes). Custom / trapezoid / chamfered pads are their convex hull, so a probe can lie
     outside a concave pad's copper (0 such cases on the snapshot boards). kicad-cli DRC stays the oracle
     (M4-R6).
  7. via ``free``: KiCad's exact flag from the save_as copy text (``toporlib.pcbfile.via_free_flags``); a via the
     copy does not list -> free = True (unsure -> fixed obstacle, never ripped).
  8. pads: exact (M4-R5) except custom / trapezoid / chamfered / offset pads -> convex hull of KiCad's polygon
     (⊇; holes ignored).
  9. keepouts: arc edges flattened outward (⊇). Board outline (Edge.Cuts): segments / arcs / rectangles exact;
     circles / Beziers / polygons flattened with vertices on the curve (an Edge.Cuts circle used as an internal
     cut-out is up to max_error smaller: NOT conservative by <= max_error, far below the edge clearance).

Complexity: O(P + T + V + S + Z * F) in pads, tracks, vias, shapes and zones with F fill points, plus one IPC
round trip per call and one ``get_pad_shapes_as_polygons`` call per pad layer that needs a polygon (custom /
trapezoid / chamfered / offset pads); dangling ends are linear for small nets (``toporlib.geom``).
"""

from __future__ import annotations

import math
from pathlib import Path

from kipy.board_types import (ArcTrack, BoardArc, BoardBezier, BoardCircle, BoardLayer, BoardPolygon,
                              BoardRectangle, BoardSegment, BoardText, Pad, Zone)
from kipy.geometry import PolygonWithHoles
from kipy.proto.board import board_types_pb2 as bt

from toporlib import geom
from toporlib.dru import merge_dru_keepouts
from toporlib.pcbfile import parse_layers, via_free_flags
from toporlib.rules import net_codes_by_name

from .findings import Finding
from .ipc import kipy_to_pcbnew_layer
from .layers import is_routed, resolve_layer_roles_ex
from .pourreach import _top_down, dangling_ends, pad_contains, pad_sample_points, pour_reach  # noqa: F401 - re-exported

_PSS = bt.PadStackShape
_HULL_SHAPES = (_PSS.PSS_TRAPEZOID, _PSS.PSS_CHAMFEREDRECT, _PSS.PSS_CUSTOM)
_ATTR = {bt.PadType.PT_PTH: "pth", bt.PadType.PT_SMD: "smd", bt.PadType.PT_EDGE_CONNECTOR: "conn",
         bt.PadType.PT_NPTH: "npth"}
_ZCS = bt.ZoneConnectionStyle
#: kipy island removal mode -> KiCad ISLAND_REMOVAL_MODE (0 always, 1 never, 2 below the minimum area)
_IRM_TO_KICAD = {bt.IslandRemovalMode.IRM_UNKNOWN: 0, bt.IslandRemovalMode.IRM_ALWAYS: 0,
                 bt.IslandRemovalMode.IRM_NEVER: 1, bt.IslandRemovalMode.IRM_AREA: 2}
#: kipy ZoneConnectionStyle -> KiCad ZONE_CONNECTION (INHERITED 0, NONE 1, THERMAL 2, FULL 3, THT_THERMAL 4)
_ZC_TO_KICAD = {_ZCS.ZCS_UNKNOWN: 0, _ZCS.ZCS_INHERITED: 0, _ZCS.ZCS_NONE: 1, _ZCS.ZCS_THERMAL: 2,
                _ZCS.ZCS_FULL: 3, _ZCS.ZCS_PTH_THERMAL: 4}
_DEFAULT_THERMAL_NM = 500_000  # KiCad ZONE_SETTINGS default (gap and spoke width)


# ---------------------------------------------------------------- layers
def cu_id(bl) -> int | None:
    """kipy ``BoardLayer`` -> engine copper id (F.Cu 0, B.Cu 2, In<n>.Cu 2n+2), None for non-copper. O(1)."""
    try:
        return kipy_to_pcbnew_layer(bl)
    except ValueError:
        return None


def layer_plan(pcb_copy_text: str, cfg_roles: dict[str, str]) -> tuple[list[dict], list[Finding]]:
    """Model ``layers`` entries from the board file's ``(layers ...)`` table and the ``[layers]`` roles of
    topor.toml (46 row 13, M4-R7). Per copper layer, top -> bottom::

        {"id", "name", "kind": KiCad type (signal|power|mixed|jumper), "role", "role_source":
         "config"|"kicad"|"default", "routable": role in (signal, mixed)}

    Roles come from ``topor.layers.resolve_layer_roles_ex``; a layer it cannot resolve (unknown copper count,
    no config) has ``role`` None, ``role_source`` "none", ``routable`` False, and a ``LAYER_ROLES_NEEDED``
    finding is returned. Returns (entries, findings). O(L)."""
    cu = parse_layers(pcb_copy_text)
    roles, sources, findings = resolve_layer_roles_ex([(g["name"], g["type"]) for g in cu], dict(cfg_roles or {}))
    out = []
    for g in cu:
        role = roles.get(g["name"])
        out.append({"id": g["id"], "name": g["name"], "kind": g["type"], "role": role,
                    "role_source": sources.get(g["name"], "none"),
                    "routable": role is not None and is_routed(role)})
    return out, findings


# ---------------------------------------------------------------- polygons
def ring_pts(polyline, max_err: int, *, outward: bool = False) -> list[list[int]]:
    """Points of a kipy ``PolyLine`` (arc nodes flattened to `max_err`), consecutive duplicates removed.

    ``outward=False``: vertices on the arcs (inscribed, as KiCad's own ERROR_INSIDE polygons). ``outward=True``
    (obstacles, M4-R8): the ring CONTAINS the true outline: an arc that bulges out of the ring is flattened by
    its tangent polyline (``geom.flatten_arc_outward``), an arc that bulges into it keeps its vertices on the arc
    (chords then lie outside the shape). O(points)."""
    pieces = _ring_pieces(polyline)
    sign = _ring_sign(pieces) if outward else 0.0
    pts: list[list[int]] = []
    for pc in pieces:
        if pc[0] == "pt":
            new = [pc[1]]
        else:
            _, a, m, e = pc
            bulge = (e[0] - a[0]) * (m[1] - a[1]) - (e[1] - a[1]) * (m[0] - a[0])
            if outward and bulge * sign < 0:  # the arc bulges away from the interior: convex side
                new = geom.flatten_arc_outward(a, m, e, max_err)
            else:
                new = geom.flatten_arc(a, m, e, max_err)
        for q in new:
            if not pts or pts[-1] != q:
                pts.append(q)
    if len(pts) > 1 and pts[0] == pts[-1]:
        pts.pop()
    return pts


def _ring_pieces(polyline) -> list[tuple]:
    """The nodes of a kipy ``PolyLine`` as ``("pt", [x, y])`` / ``("arc", start, mid, end)`` tuples. O(nodes)."""
    out: list[tuple] = []
    for n in polyline.nodes:
        if n.has_point:
            out.append(("pt", [int(n.point.x), int(n.point.y)]))
        elif n.has_arc:
            a = n.arc
            out.append(("arc", [int(a.start.x), int(a.start.y)], [int(a.mid.x), int(a.mid.y)],
                        [int(a.end.x), int(a.end.y)]))
    return out


def _ring_sign(pieces) -> float:
    """Sign of the shoelace area of the ring through every node point (arc mid points included). O(n)."""
    pts = [q for pc in pieces for q in ([pc[1]] if pc[0] == "pt" else [pc[1], pc[2], pc[3]])]
    a = sum(pts[i][0] * pts[(i + 1) % len(pts)][1] - pts[(i + 1) % len(pts)][0] * pts[i][1]
            for i in range(len(pts))) if pts else 0
    return 1.0 if a >= 0 else -1.0


def _ring_edges(pieces) -> list[tuple]:
    """The outline of `pieces` as closed edges ``("seg", a, b)`` / ``("arc", s, m, e)``. O(n)."""
    out: list[tuple] = []
    n = len(pieces)
    for i, pc in enumerate(pieces):
        if pc[0] == "arc":
            out.append(("arc", pc[1], pc[2], pc[3]))
        start = pc[1] if pc[0] == "pt" else pc[3]
        nxt = pieces[(i + 1) % n]
        end = nxt[1]
        if start != end:
            out.append(("seg", start, end))
    return out


def _hull_of(rings: list) -> list[list[int]]:
    return geom.hull([tuple(p) for r in rings for p in r])


# ---------------------------------------------------------------- pads
def _stack_layer(ps, bl):
    """The ``PadStackLayer`` that applies to copper layer `bl` (None for a padstack without copper)."""
    cl = ps.copper_layers
    if not cl:
        return None
    for c in cl:
        if c.layer == bl:
            return c
    if ps.type == bt.PadStackType.PST_FRONT_INNER_BACK and bl not in (BoardLayer.BL_F_Cu, BoardLayer.BL_B_Cu):
        for c in cl:
            if c.layer == BoardLayer.BL_In1_Cu:
                return c
    return cl[0]  # PST_NORMAL: one entry for every copper layer


def _round(x: float) -> int:
    return int(math.floor(x + 0.5))


def pad_shape(pad, bl, poly_fn) -> dict:
    """Model shape of `pad` on kipy copper layer `bl`: circle / rect / roundrect / oval from the padstack, a
    convex-hull ``poly`` (absolute nm) for trapezoid / chamfered / custom pads and for any pad with an offset.
    `poly_fn(pad, bl) -> list[list[(x, y)]] | None` supplies KiCad's polygon for the latter (live:
    ``Board.get_pad_shapes_as_polygons``). O(1) plus the polygon hull."""
    sl = _stack_layer(pad.padstack, bl)
    w, h = int(sl.size.x), int(sl.size.y)
    off = sl.offset
    shp = sl.shape
    if (off.x or off.y) and shp not in _HULL_SHAPES:
        shp = None
    if shp == _PSS.PSS_CIRCLE:
        return {"kind": "circle", "d": w}
    if shp == _PSS.PSS_RECTANGLE:
        return {"kind": "rect", "w": w, "h": h}
    if shp == _PSS.PSS_ROUNDRECT:
        return {"kind": "roundrect", "w": w, "h": h, "rc": _round(min(w, h) * sl.corner_rounding_ratio)}
    if shp == _PSS.PSS_OVAL:
        return {"kind": "oval", "w": w, "h": h}
    rings = poly_fn(pad, bl) if poly_fn is not None else None
    if not rings:
        raise ValueError(f"pad {pad.number}: KiCad returned no polygon on layer {BoardLayer.Name(bl)}")
    return {"kind": "poly", "pts": _hull_of(rings)}


def pad_from_kipy(pad, *, ref: str, net_codes: dict, cu_ids: set, poly_fn=None, fp_zone_connection: int = 0,
                  tie_group: int | None = None) -> tuple[dict, dict | None]:
    """One footprint pad -> (model pad dict, hole dict or None). `net_codes`: net name -> code (``""`` -> 0);
    `cu_ids`: enabled engine copper ids; `fp_zone_connection`: the footprint's KiCad ZONE_CONNECTION override
    (0 = inherited); `tie_group`: index of the net-tie group containing this pad number. O(layers)."""
    ps = pad.padstack
    layers = []
    shapes = {}
    for bl in ps.layers:
        i = cu_id(bl)
        if i is not None and i in cu_ids and i not in layers:
            layers.append(i)
            shapes[str(i)] = pad_shape(pad, bl, poly_fn)
    layers.sort(key=_top_down)
    attr = _ATTR.get(pad.pad_type, "smd")
    d = ps.drill.diameter
    drill = {"w": int(d.x), "h": int(d.y)} if d.x > 0 and d.y > 0 else None
    pos = [int(pad.position.x), int(pad.position.y)]
    p = pad.proto
    out = {
        "uuid": pad.id.value,
        "ref": ref,
        "number": pad.number,
        "net": int(net_codes.get(pad.net.name, 0)) if pad.net.name else 0,
        "attr": attr,
        "center": pos,
        "rotation_deg": float(ps.angle.degrees),
        "layers": layers,
        "shapes": shapes,
        "drill": drill,
        "local_clearance": int(p.copper_clearance_override.value_nm) if p.HasField("copper_clearance_override")
        else None,
        "net_tie_group": tie_group,
    }
    zs = ps.zone_settings.proto
    conn = _ZC_TO_KICAD.get(zs.zone_connection, 0) or int(fp_zone_connection)
    if conn:
        out["zone_connection"] = conn
    if zs.thermal_spokes.HasField("gap") and zs.thermal_spokes.gap.value_nm > 0:
        out["thermal_gap"] = int(zs.thermal_spokes.gap.value_nm)
    if zs.thermal_spokes.HasField("angle"):
        out["spoke_angle_deg"] = float(zs.thermal_spokes.angle.value_degrees)
    hole = None
    if drill is not None:
        hole = {"at": pos, "d": max(drill["w"], drill["h"]), "kind": "pth" if attr in ("pth", "conn") else "npth"}
        if drill["w"] != drill["h"]:
            hole["slot"] = [drill["w"], drill["h"]]
    return out, hole


# ---------------------------------------------------------------- tracks / vias
def track_to_kept(t, net_codes: dict) -> dict:
    """``Track`` / ``ArcTrack`` -> ``kept`` entry. O(1)."""
    if isinstance(t, ArcTrack):
        prim = {"kind": "arc", "start": [t.start.x, t.start.y], "mid": [t.mid.x, t.mid.y], "end": [t.end.x, t.end.y]}
    else:
        prim = {"kind": "seg", "a": [t.start.x, t.start.y], "b": [t.end.x, t.end.y]}
    return {"uuid": t.id.value, "net": int(net_codes.get(t.net.name, 0)) if t.net.name else 0,
            "layer": cu_id(t.layer), "width": int(t.width), "prim": prim, "locked": bool(t.locked)}


def via_to_model(v, net_codes: dict, cu_order: list, free: bool = False) -> dict:
    """``Via`` -> model via. `cu_order`: enabled engine copper ids top -> bottom (to order a blind/buried
    via's span). O(1)."""
    ps = v.padstack
    top = ps.copper_layer(BoardLayer.BL_F_Cu) or (ps.copper_layers[0] if ps.copper_layers else None)
    if v.type == bt.ViaType.VT_THROUGH or not (cu_id(ps.drill.start_layer) is not None
                                               and cu_id(ps.drill.end_layer) is not None):
        span = [0, 2]
    else:
        span = sorted((cu_id(ps.drill.start_layer), cu_id(ps.drill.end_layer)), key=_top_down)
    return {"uuid": v.id.value, "net": int(net_codes.get(v.net.name, 0)) if v.net.name else 0,
            "at": [int(v.position.x), int(v.position.y)], "d": int(top.size.x) if top else 0,
            "drill": int(ps.drill.diameter.x), "layers": span, "free": bool(free), "locked": bool(v.locked)}


def via_free(uuid: str, flags: dict) -> bool:
    """KiCad's "free" flag of via `uuid` from :func:`toporlib.pcbfile.via_free_flags` of the save_as copy.
    A via the copy does not list (uuid not found, unparsable copy) is free = True: unsure -> it stays a fixed
    obstacle and is never ripped (M4-R8, ruling C23). O(1)."""
    return bool(flags.get(uuid, True))


# ---------------------------------------------------------------- graphics
def _stroke_w(shape) -> int:
    return int(shape.attributes.stroke.width)


def _filled(shape) -> bool:
    return bool(shape.attributes.fill.filled)


def shape_to_prims(shape, max_err: int) -> list[dict]:
    """Edge.Cuts shape -> outline primitives (``seg`` / ``arc`` dicts), as the harness: segments and arcs
    exact, a rectangle as 4 segments, a Bezier / circle / polygon flattened to segments at `max_err` (the
    flattening is NOT point-identical to KiCad's). O(output)."""
    def seg(a, b):
        return {"kind": "seg", "a": [int(a[0]), int(a[1])], "b": [int(b[0]), int(b[1])]}

    def ring_segs(pts):
        return [seg(pts[i], pts[(i + 1) % len(pts)]) for i in range(len(pts))]

    if isinstance(shape, BoardSegment):
        return [seg((shape.start.x, shape.start.y), (shape.end.x, shape.end.y))]
    if isinstance(shape, BoardArc):
        return [{"kind": "arc", "start": [shape.start.x, shape.start.y], "mid": [shape.mid.x, shape.mid.y],
                 "end": [shape.end.x, shape.end.y]}]
    if isinstance(shape, BoardRectangle):
        a, b = shape.top_left, shape.bottom_right
        return ring_segs([(a.x, a.y), (b.x, a.y), (b.x, b.y), (a.x, b.y)])
    if isinstance(shape, BoardBezier):
        pts = geom.flatten_bezier((shape.start.x, shape.start.y), (shape.control1.x, shape.control1.y),
                                  (shape.control2.x, shape.control2.y), (shape.end.x, shape.end.y), max_err)
        return [seg(pts[i], pts[i + 1]) for i in range(len(pts) - 1)]
    if isinstance(shape, BoardCircle):
        r = int(round(math.hypot(shape.radius_point.x - shape.center.x, shape.radius_point.y - shape.center.y)))
        return ring_segs(geom.circle_ring((shape.center.x, shape.center.y), r, max_err))
    if isinstance(shape, BoardPolygon):
        out = []
        for poly in shape.polygons:
            out += ring_segs(ring_pts(poly.outline, max_err))
        return out
    return []


def _rect_corners(shape) -> tuple[int, int, int, int]:
    a, b = shape.top_left, shape.bottom_right
    return min(a.x, b.x), min(a.y, b.y), max(a.x, b.x), max(a.y, b.y)


def _corner_radius(shape) -> int:
    p = shape.proto.shape.rectangle
    return int(p.corner_radius.value_nm) if p.HasField("corner_radius") else 0


def _rounded_box(x0, y0, x1, y1, rc: float, grow: float, max_err: int) -> list[list[int]]:
    """Ring CONTAINING the box (x0, y0)-(x1, y1) with corner radius `rc`, grown by `grow` on every side (corner
    arcs of radius rc + grow, tangent construction). rc + grow == 0 gives the sharp box. O(1/sqrt(max_err))."""
    rr = rc + grow
    if rr <= 0:
        return [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    cs = ((x1 - rc, y1 - rc, 0.0), (x0 + rc, y1 - rc, math.pi / 2), (x0 + rc, y0 + rc, math.pi),
          (x1 - rc, y0 + rc, 3 * math.pi / 2))
    out: list[list[int]] = []
    for cx, cy, a0 in cs:
        for q in geom.tangent_arc_pts((cx, cy), rr, a0, math.pi / 2, max_err):
            if not out or out[-1] != q:
                out.append(q)
    return out


def _stroke_edges(edges, w: int, max_err: int) -> list[list[list[int]]]:
    """Rings containing the stroke of every edge (``seg`` capsule / ``arc`` annular sector, round caps). O(n)."""
    out = []
    for ed in edges:
        if ed[0] == "seg":
            out.append(geom.capsule_ring_outward(ed[1], ed[2], w, max_err))
        else:
            out += geom.arc_stroke_rings(ed[1], ed[2], ed[3], w, max_err)
    return out


#: Error (nm) of OUR circumscribed obstacle polygons, capped below KiCad's max_error: KiCad's own polygon may sit
#: up to max_error inside the true curve, so ours must stay within ~1 um outside it for the bound "IPC obstacle
#: within KiCad's polygon grown by max_error + 1 um" (ruling C25) to hold everywhere.
OBSTACLE_ERR_NM = 500


def shape_to_polys(shape, max_err: int) -> list[list[list[int]]]:
    """Copper graphic -> obstacle polygons (outer rings, nm) that CONTAIN KiCad's shape (M4-R8, ruling C25):
    every arc / cap / circle vertex is placed by the tangent construction (``toporlib.geom`` ``*_outward``), so
    the outward excess over the true shape is at most ``min(max_err, OBSTACLE_ERR_NM)`` (+ 1 nm). A stroked
    segment is a capsule, a stroked arc one annular sector with round caps, a stroked Bezier
    ``geom.bezier_stroke_rings``, stroked rectangle / circle / polygon outlines a chain of capsules / sectors; a filled rectangle
    (with KiCad 10 corner radius), circle or polygon is its area grown by half the stroke width (a filled
    polygon's holes are ignored: superset). Our vertex error is ``min(max_err, OBSTACLE_ERR_NM)``; a Bezier is
    grown on its concave side by `max_err` because KiCad strokes its own flattening of the curve. O(output)."""
    max_err, err = int(max_err), min(int(max_err), OBSTACLE_ERR_NM)
    w = _stroke_w(shape)
    filled = _filled(shape)
    if isinstance(shape, BoardSegment):
        return [geom.capsule_ring_outward((shape.start.x, shape.start.y), (shape.end.x, shape.end.y), w, err)]
    if isinstance(shape, BoardArc):
        return geom.arc_stroke_rings((shape.start.x, shape.start.y), (shape.mid.x, shape.mid.y),
                                     (shape.end.x, shape.end.y), w, err)
    if isinstance(shape, BoardBezier):
        return geom.bezier_stroke_rings((shape.start.x, shape.start.y), (shape.control1.x, shape.control1.y),
                                        (shape.control2.x, shape.control2.y), (shape.end.x, shape.end.y), w,
                                        err, kicad_err=max_err)
    if isinstance(shape, BoardRectangle):
        x0, y0, x1, y1 = _rect_corners(shape)
        rc = min(_corner_radius(shape), (x1 - x0) // 2, (y1 - y0) // 2)
        if filled:
            return [_rounded_box(x0, y0, x1, y1, rc, w / 2.0, err)]
        if rc <= 0:
            return _stroke_edges([("seg", [x0, y0], [x1, y0]), ("seg", [x1, y0], [x1, y1]),
                                  ("seg", [x1, y1], [x0, y1]), ("seg", [x0, y1], [x0, y0])], w, err)
        k = rc * (1 - math.sqrt(0.5))  # corner arc mid point offset from the box corner, per axis
        edges = [("seg", [x0 + rc, y0], [x1 - rc, y0]), ("arc", [x1 - rc, y0], [x1 - k, y0 + k], [x1, y0 + rc]),
                 ("seg", [x1, y0 + rc], [x1, y1 - rc]), ("arc", [x1, y1 - rc], [x1 - k, y1 - k], [x1 - rc, y1]),
                 ("seg", [x1 - rc, y1], [x0 + rc, y1]), ("arc", [x0 + rc, y1], [x0 + k, y1 - k], [x0, y1 - rc]),
                 ("seg", [x0, y1 - rc], [x0, y0 + rc]), ("arc", [x0, y0 + rc], [x0 + k, y0 + k], [x0 + rc, y0])]
        return _stroke_edges(edges, w, err)
    if isinstance(shape, BoardCircle):
        r = math.hypot(shape.radius_point.x - shape.center.x, shape.radius_point.y - shape.center.y)
        c = (shape.center.x, shape.center.y)
        if filled:
            return [geom.circle_ring_outward(c, r + w / 2.0, err)]
        hw = w / 2.0
        if r - hw > err:  # annulus as two annular half-sectors (outer tangent, inner on the circle)
            out = []
            for a0 in (0.0, math.pi):
                outer = geom.tangent_arc_pts(c, r + hw, a0, math.pi, err)
                inner = geom.chord_arc_pts(c, max(r - hw - 1.0, 0.0), a0 + math.pi, -math.pi, err)
                out.append(outer + inner)
            return out
        return [geom.circle_ring_outward(c, r + hw, err)]  # stroke covers the centre: a disc
    if isinstance(shape, BoardPolygon):
        out = []
        for poly in shape.polygons:
            pieces = _ring_pieces(poly.outline)
            if len(pieces) < 2:
                continue
            if filled:
                ring = ring_pts(poly.outline, err, outward=True)
                if len(ring) >= 3:
                    out.append(ring)
            if w > 0 or not filled:
                out += _stroke_edges(_ring_edges(pieces), w, err)
        return out
    return []


#: Growth of KiCad's text bounding box, as a fraction of the larger text size (M4-R8, ruling C25). KiCad's
#: ``GetBoundingBox()`` of a text is its nominal box, NOT its copper: descenders, accents and brackets of the
#: KiCad stroke font reach up to 0.33 x the text size outside it (thin strokes; measured offline in
#: tests/plugin/test_ipc_conservative.py over stroke / outline fonts, sizes 0.5..5 mm, strokes 0.02..0.8 mm,
#: bold / italic / rotated / mirrored / knockout / multi-line). 0.5 keeps every measured glyph inside.
TEXT_BBOX_GROW = 0.5


def text_margin(t) -> int:
    """Margin (nm) around the bounding box of kipy text `t` (``BoardText`` / ``BoardTextBox``):
    ``TEXT_BBOX_GROW`` x its larger text size. O(1)."""
    try:
        sz = t.attributes.size
        size = max(abs(int(sz.x)), abs(int(sz.y)))
    except AttributeError:
        size = 0
    return int(math.ceil(TEXT_BBOX_GROW * size))


def text_box_poly(bb, margin: int = 0) -> list[list[int]]:
    """Copper text obstacle: KiCad's bounding box (``Box2``: pos + size; a negative size is normalised) grown
    by `margin` (:func:`text_margin`). A superset of the glyph / knockout copper (M4-R8: never smaller). O(1)."""
    xa, xb = sorted((int(bb.pos.x), int(bb.pos.x + bb.size.x)))
    ya, yb = sorted((int(bb.pos.y), int(bb.pos.y + bb.size.y)))
    x0, y0, x1, y1 = xa - margin, ya - margin, xb + margin, yb + margin
    return [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]


# ---------------------------------------------------------------- zones
def zone_to_keepouts(zone, cu_ids: set, max_err: int) -> list[dict]:
    """Rule-area zone -> one keepout per outline polygon (name, copper layers, poly, no_tracks, no_vias, no_pour,
    no_pads, no_footprints; the last two are carried, unused); arc
    edges are flattened outward (the keepout contains KiCad's area, M4-R8). O(points)."""
    ra = zone.proto.rule_area_settings
    layers = sorted({i for i in (cu_id(bl) for bl in zone.layers) if i is not None and i in cu_ids}, key=_top_down)
    out = []
    for pb in zone.proto.outline.polygons:
        out.append({"name": zone.name, "layers": layers, "poly": ring_pts(PolygonWithHoles(pb).outline, max_err, outward=True),
                    "no_tracks": bool(ra.keepout_tracks), "no_vias": bool(ra.keepout_vias),
                    "no_pour": bool(ra.keepout_copper), "no_pads": bool(ra.keepout_pads),
                    "no_footprints": bool(ra.keepout_footprints)})
    return out


def zone_to_pour(zone, *, net_codes: dict, cu_ids: set, pads: list, vias: list, max_err: int) -> dict:
    """Copper zone -> pour dict (fills per layer, settings, reached pads/vias computed geometrically).

    `pads` / `vias`: the model pad / via dicts. A same-net pad is reached on a zone layer when one of its probe
    points (:func:`pad_sample_points`) lies in that layer's fill (even-odd over all fill rings); a same-net via
    when its centre does (vias reach the layers of their span). With no filled polygons (never filled) the
    zone outline stands in for the fill and ``warning`` says so, as the harness does, but it reaches no pad or
    via (M4-R8: "not reached"). O(F * (pads + vias))."""
    code = int(net_codes.get(zone.net.name, 0)) if zone.net is not None and zone.net.name else 0
    layers = sorted({i for i in (cu_id(bl) for bl in zone.layers) if i is not None and i in cu_ids}, key=_top_down)
    fills: list[dict] = []
    for bl, polys in zone.filled_polygons.items():
        i = cu_id(bl)
        if i is None or i not in layers:
            continue
        for pwh in polys:
            fills.append({"layer": i, "outer": ring_pts(pwh.outline, max_err),
                          "holes": [ring_pts(hh, max_err) for hh in pwh.holes]})
    warn = None
    if not fills:
        warn = "filled polygon list unavailable (zone not filled); using zone outline, reaching nothing"
        ol = ring_pts(zone.outline.outline, max_err)
        fills = [{"layer": i, "outer": ol, "holes": []} for i in layers]
        reach_pads, reach_vias = [], []  # M4-R8: an unfilled zone connects nothing ("not reached")
    else:
        reach_pads, reach_vias = pour_reach(code, layers, fills, pads, vias)
    cs, th = zone.proto.copper_settings, zone.proto.copper_settings.connection.thermal_spokes
    d = {"uuid": zone.id.value, "net": code, "layers": layers, "pads": reach_pads, "vias": reach_vias,
         "fills": fills,
         "clearance": int(cs.clearance.value_nm), "min_thickness": int(cs.min_thickness.value_nm),
         "thermal_gap": int(th.gap.value_nm) if th.HasField("gap") else _DEFAULT_THERMAL_NM,
         "spoke_width": int(th.width.value_nm) if th.HasField("width") else _DEFAULT_THERMAL_NM,
         "pad_connection": _ZC_TO_KICAD.get(cs.connection.zone_connection, 0),
         "island_removal": _IRM_TO_KICAD.get(cs.island_mode, 0), "min_island_area": float(cs.min_island_area)}
    if warn:
        d["warning"] = warn
    return d


# ---------------------------------------------------------------- the exporter
def _is_copper_pour(z) -> bool:
    return z.type == bt.ZoneType.ZT_COPPER and z.net is not None and bool(z.net.name)


def _net_tie_groups(fp) -> list[list[str]]:
    return [list(nt.pad_number) for nt in fp.definition.proto.net_ties]


def _dru_text(project, board_name: str, dru_text: str | None) -> str | None:
    if dru_text is not None:
        return dru_text
    try:
        f = Path(project.path) / (Path(board_name).stem + ".kicad_dru")
        return f.read_text() if f.is_file() else None
    except (OSError, TypeError, AttributeError):
        return None


def export_model_ipc(board, project, pcb_copy_text: str, rules: dict, *, name: str,
                     layer_roles: dict | None = None, dru_text: str | None = None,
                     net_codes: dict | None = None) -> dict:
    """BoardModel dict of the board open in KiCad, same schema as ``harness.model.export_model``.

    `board`: ``kipy.board.Board``; `project`: its ``Project`` (the ``.kicad_dru`` next to it is used for
    `disallow` keepouts unless `dru_text` is given; ``IpcBoard.read_model`` passes the save_as COPY's file,
    "" when the copy has none); `pcb_copy_text`: the ``save_as`` copy's text (layer types); `rules`: the
    resolved rules dict (``toporlib.rules``); `net_codes`: net name -> code, the SAME table the rules were
    resolved with (default: ``net_codes_by_name`` of ``board.get_nets()``); `layer_roles`: ``cfg.layer_roles``.
    The fills are the live board's as they are (possibly stale): the run flow replaces fills, reach and dangling
    ends from the kicad-cli refilled copy (``run.refill_pours``, ruling C29); a zone without fills stands in with
    its outline (warning) and reaches nothing. The returned ``layers`` carry ``kind``, ``role``,
    ``role_source`` and ``routable`` (:func:`layer_plan`); its findings are the caller's business (call
    :func:`layer_plan` for them). UNVERIFIED live: every ``board.*`` call below."""
    max_err = int(rules.get("max_error") or 5000)
    plan, _ = layer_plan(pcb_copy_text, layer_roles or {})
    enabled = [i for i in map(cu_id, board.get_enabled_layers()) if i is not None]  # UNVERIFIED
    plan = [g for g in plan if g["id"] in set(enabled)] or [
        {"id": i, "name": f"Cu{i}", "kind": "signal", "role": "signal", "role_source": "default", "routable": True}
        for i in sorted(enabled, key=_top_down)]
    cu_order = [g["id"] for g in plan]
    cu_ids = set(cu_order)

    codes = dict(net_codes) if net_codes is not None else net_codes_by_name([n.name for n in board.get_nets()])
    nets = [{"code": c, "name": n} for n, c in sorted(codes.items(), key=lambda kv: kv[1])]

    def poly_fn(pad, bl):  # UNVERIFIED: Board.get_pad_shapes_as_polygons
        pwh = board.get_pad_shapes_as_polygons(pad, bl)
        return [ring_pts(pwh.outline, max_err)] if pwh is not None else None

    footprints = list(board.get_footprints())
    pads, holes, net_ties, fp_shapes, fp_zones, fp_texts = [], [], [], [], [], []
    for fp in footprints:
        ref = fp.reference_field.text.value
        groups = _net_tie_groups(fp)
        by_num: dict[str, int] = {}
        for gi, g in enumerate(groups):
            for n in g:
                by_num.setdefault(n, gi)
        fpc = _ZC_TO_KICAD.get(fp.overrides.zone_connection, 0)
        fpads = [it for it in fp.definition.items if isinstance(it, Pad)]
        for pad in fpads:
            pd, hole = pad_from_kipy(pad, ref=ref, net_codes=codes, cu_ids=cu_ids, poly_fn=poly_fn,
                                     fp_zone_connection=fpc, tie_group=by_num.get(pad.number) if groups else None)
            pads.append(pd)
            if hole is not None:
                holes.append(hole)
        if groups:
            pnets = {p.number: codes.get(p.net.name, 0) if p.net.name else 0 for p in fpads}
            nl = []
            for g in groups:
                seen: list[int] = []
                for n in g:
                    c = pnets.get(n, 0)
                    if c and c not in seen:
                        seen.append(c)
                nl.append(seen)
            net_ties.append({"ref": ref, "groups": groups, "nets": nl})
        fp_zones += [it for it in fp.definition.items if isinstance(it, Zone)]
        fp_texts += [(it, ref) for it in fp.definition.items if isinstance(it, BoardText)]
        fp_shapes += [(sh, ref) for sh in fp.definition.shapes]

    tracks = list(board.get_tracks())
    kept = [track_to_kept(t, codes) for t in tracks if cu_id(t.layer) is not None]
    free_flags = via_free_flags(pcb_copy_text)  # ruling C23: KiCad's own flag, read from the save_as copy
    vias = [via_to_model(v, codes, cu_order, free=via_free(v.id.value, free_flags)) for v in board.get_vias()]

    # copper graphics and the board outline
    shapes = [(s, None) for s in board.get_shapes()] + fp_shapes
    copper_shapes, outline = [], []
    for s, owner in shapes:
        i = cu_id(s.layer)
        if i is not None and i in cu_ids:
            for ring in shape_to_polys(s, max_err):
                copper_shapes.append({"uuid": s.id.value, "net": codes.get(s.net.name, 0) if s.net.name else 0,
                                      "layer": i, "poly": ring, "owner_ref": owner})
        elif s.layer == BoardLayer.BL_Edge_Cuts:
            outline += shape_to_prims(s, max_err)
    for t, owner in [(t, None) for t in board.get_text()] + fp_texts:  # UNVERIFIED: get_item_bounding_box
        i = cu_id(t.layer) if hasattr(t, "layer") else None
        if i is None or i not in cu_ids:
            continue
        bb = board.get_item_bounding_box(t)
        if bb is not None:
            copper_shapes.append({"uuid": t.id.value, "net": 0, "layer": i, "owner_ref": owner,
                                  "poly": text_box_poly(bb, text_margin(t))})

    # zones: rule areas (board, then footprint-embedded) and copper pours
    zones = list(board.get_zones())
    keepouts = []
    for z in zones + fp_zones:
        if z.is_rule_area():
            keepouts += zone_to_keepouts(z, cu_ids, max_err)
    layer_ids = {g["name"]: g["id"] for g in plan}
    keepouts, rules = merge_dru_keepouts(keepouts, rules, _dru_text(project, name, dru_text), layer_ids)
    pours = sorted((zone_to_pour(z, net_codes=codes, cu_ids=cu_ids, pads=pads, vias=vias, max_err=max_err)
                    for z in zones if _is_copper_pour(z)), key=lambda d: d["uuid"])

    # dangling ends
    dangling = dangling_ends(kept, vias, pads, cu_order)

    return {
        "units": "nm",
        "board": name,
        "layers": plan,
        "nets": nets,
        "pads": pads,
        "vias": vias,
        "kept": kept,
        "copper_shapes": copper_shapes,
        "holes": holes,
        "outline": outline,
        "keepouts": keepouts,
        "net_ties": net_ties,
        "pours": pours,
        "dangling_ends": dangling,
        "rules": rules,
    }


__all__ = ["export_model_ipc", "layer_plan", "pad_from_kipy", "pad_shape", "pad_contains", "shape_to_prims",
           "shape_to_polys", "zone_to_keepouts", "zone_to_pour", "track_to_kept", "via_to_model", "via_free", "ring_pts",
           "cu_id", "text_box_poly", "text_margin", "pour_reach", "pad_sample_points", "dangling_ends"]
