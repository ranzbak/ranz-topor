"""BoardModel exporter over kicad-python (kipy 0.8): the IPC twin of ``harness.model.export_model`` (same
schema: ``units, board, layers, nets, pads, vias, kept, copper_shapes, holes, outline, keepouts, net_ties,
pours, dangling_ends, rules``; KiCad frame, Y down, integer nm). No pcbnew (M4-R1).

Everything that only reshapes data is a pure function over kipy wrappers (unit-tested with wrappers built from
protos): ``pad_from_kipy``, ``shape_to_prims``, ``shape_to_polys``, ``zone_to_keepouts``, ``zone_to_pour``,
``track_to_kept``, ``via_to_model``, ``layer_plan``. Only :func:`export_model_ipc` talks to a live ``Board``;
the live-only calls are marked ``UNVERIFIED`` (get_pad_shapes_as_polygons, get_item_bounding_box,
get_enabled_layers, get_zones filled polygons, ...).

Conventions (documented deviations from the harness, see the B3 report):
  * net codes are ``toporlib.rules.net_codes_by_name`` (sorted names, 1-based), shared with the rule resolver;
  * pad rotation is ``padstack.angle`` (absolute, as kipy's own footprint-rotation code treats it);
  * ``Via.free`` is approximated (no same-net track end touches the via), the IPC API has no such flag;
  * copper text becomes its bounding box; keepout layers are the copper layers only.

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
from toporlib.pcbfile import parse_layers
from toporlib.rules import net_codes_by_name

from .findings import Finding
from .ipc import kipy_to_pcbnew_layer
from .layers import is_routed, resolve_layer_roles_ex

_PSS = bt.PadStackShape
_HULL_SHAPES = (_PSS.PSS_TRAPEZOID, _PSS.PSS_CHAMFEREDRECT, _PSS.PSS_CUSTOM)
_ATTR = {bt.PadType.PT_PTH: "pth", bt.PadType.PT_SMD: "smd", bt.PadType.PT_EDGE_CONNECTOR: "conn",
         bt.PadType.PT_NPTH: "npth"}
_ZCS = bt.ZoneConnectionStyle
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


def _top_down(i: int) -> tuple:
    return (0, 0) if i == 0 else (2, 0) if i == 2 else (1, i)


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
def ring_pts(polyline, max_err: int) -> list[list[int]]:
    """Points of a kipy ``PolyLine`` (arc nodes flattened to `max_err`), consecutive duplicates removed."""
    pts: list[list[int]] = []
    for n in polyline.nodes:
        if n.has_point:
            new = [[int(n.point.x), int(n.point.y)]]
        elif n.has_arc:
            a = n.arc
            new = geom.flatten_arc((a.start.x, a.start.y), (a.mid.x, a.mid.y), (a.end.x, a.end.y), max_err)
        else:
            continue
        for p in new:
            if not pts or pts[-1] != p:
                pts.append(p)
    if len(pts) > 1 and pts[0] == pts[-1]:
        pts.pop()
    return pts


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


def pad_contains(pad: dict, layer: int, p) -> bool:
    """True when point `p` lies in the copper of model pad `pad` on `layer` (rotation and shape aware). O(1)
    (O(n) for a ``poly`` hull)."""
    sh = pad["shapes"].get(str(layer))
    if sh is None:
        return False
    if sh["kind"] == "poly":
        return geom.point_in_ring(p, sh["pts"])
    th = math.radians(pad["rotation_deg"])
    dx, dy = p[0] - pad["center"][0], p[1] - pad["center"][1]
    lx = dx * math.cos(th) - dy * math.sin(th)
    ly = dx * math.sin(th) + dy * math.cos(th)
    if sh["kind"] == "circle":
        return math.hypot(lx, ly) <= sh["d"] / 2.0
    w, h = sh["w"] / 2.0, sh["h"] / 2.0
    if sh["kind"] == "rect":
        return abs(lx) <= w and abs(ly) <= h
    if sh["kind"] == "oval":
        r = min(w, h)
        qx, qy = max(abs(lx) - (w - r), 0.0), max(abs(ly) - (h - r), 0.0)
        return math.hypot(qx, qy) <= r
    r = min(sh["rc"], w, h)  # roundrect
    qx, qy = max(abs(lx) - (w - r), 0.0), max(abs(ly) - (h - r), 0.0)
    return math.hypot(qx, qy) <= r


def pad_sample_points(pad: dict, layer: int) -> list[list[float]]:
    """Centre, corners and edge midpoints (absolute nm) of a model pad on `layer`: the probe points of the
    pour-reach test. O(1)."""
    sh = pad["shapes"].get(str(layer))
    cx, cy = pad["center"]
    if sh is None:
        return []
    if sh["kind"] == "poly":
        pts = sh["pts"]
        mids = [[(pts[i][0] + pts[(i + 1) % len(pts)][0]) / 2, (pts[i][1] + pts[(i + 1) % len(pts)][1]) / 2]
                for i in range(len(pts))]
        return [[cx, cy], *pts, *mids]
    w, h = (sh["d"], sh["d"]) if sh["kind"] == "circle" else (sh["w"], sh["h"])
    th = math.radians(pad["rotation_deg"])
    c, s = math.cos(th), math.sin(th)
    local = [(0, 0)] + [(sx * w / 2.0 * k, sy * h / 2.0 * k) for k in (0.98,) for sx in (-1, 0, 1)
                                                            for sy in (-1, 0, 1) if (sx, sy) != (0, 0)]
    return [[cx + lx * c + ly * s, cy - lx * s + ly * c] for lx, ly in local]


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


def shape_to_polys(shape, max_err: int) -> list[list[list[int]]]:
    """Copper graphic -> polygons (outer rings, nm). A stroked outline is a chain of round-capped capsules
    (overlapping rings); a filled rectangle / circle / polygon is its area (rectangles and circles grown by
    half the stroke width). `max_err` bounds the arc/cap error. O(output)."""
    w = _stroke_w(shape)

    def chain(pts, closed):
        n = len(pts) if closed else len(pts) - 1
        return [geom.capsule_ring(pts[i], pts[(i + 1) % len(pts)], w, max_err) for i in range(n)]

    if isinstance(shape, BoardSegment):
        return [geom.capsule_ring((shape.start.x, shape.start.y), (shape.end.x, shape.end.y), w, max_err)]
    if isinstance(shape, BoardArc):
        return chain(geom.flatten_arc((shape.start.x, shape.start.y), (shape.mid.x, shape.mid.y),
                                      (shape.end.x, shape.end.y), max_err), False)
    if isinstance(shape, BoardBezier):
        return chain(geom.flatten_bezier((shape.start.x, shape.start.y), (shape.control1.x, shape.control1.y),
                                         (shape.control2.x, shape.control2.y), (shape.end.x, shape.end.y),
                                         max_err), False)
    if isinstance(shape, BoardRectangle):
        a, b = shape.top_left, shape.bottom_right
        g = w // 2 if _filled(shape) else 0
        if _filled(shape):
            return [[[a.x - g, a.y - g], [b.x + g, a.y - g], [b.x + g, b.y + g], [a.x - g, b.y + g]]]
        return chain([(a.x, a.y), (b.x, a.y), (b.x, b.y), (a.x, b.y)], True)
    if isinstance(shape, BoardCircle):
        r = int(round(math.hypot(shape.radius_point.x - shape.center.x, shape.radius_point.y - shape.center.y)))
        c = (shape.center.x, shape.center.y)
        if _filled(shape):
            return [geom.circle_ring(c, r + w // 2, max_err)]
        return chain(geom.circle_ring(c, r, max_err), True)
    if isinstance(shape, BoardPolygon):
        out = []
        for poly in shape.polygons:
            pts = ring_pts(poly.outline, max_err)
            if len(pts) < 2:
                continue
            out += [pts] if _filled(shape) else chain(pts, True)
        return out
    return []


# ---------------------------------------------------------------- zones
def zone_to_keepouts(zone, cu_ids: set, max_err: int) -> list[dict]:
    """Rule-area zone -> one keepout per outline polygon (name, copper layers, poly, no_tracks, no_vias). O(points)."""
    ra = zone.proto.rule_area_settings
    layers = sorted({i for i in (cu_id(bl) for bl in zone.layers) if i is not None and i in cu_ids}, key=_top_down)
    out = []
    for pb in zone.proto.outline.polygons:
        out.append({"name": zone.name, "layers": layers, "poly": ring_pts(PolygonWithHoles(pb).outline, max_err),
                    "no_tracks": bool(ra.keepout_tracks), "no_vias": bool(ra.keepout_vias)})
    return out


def zone_to_pour(zone, *, net_codes: dict, cu_ids: set, pads: list, vias: list, max_err: int) -> dict:
    """Copper zone -> pour dict (fills per layer, settings, reached pads/vias computed geometrically).

    `pads` / `vias`: the model pad / via dicts. A same-net pad is reached on a zone layer when one of its probe
    points (:func:`pad_sample_points`) lies in that layer's fill (even-odd over all fill rings); a same-net via
    when its centre does (vias reach the layers of their span). With no filled polygons (never filled) the
    zone outline stands in for the fill and ``warning`` says so, as the harness does. O(F * (pads + vias))."""
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
        warn = "filled polygon list unavailable (zone not filled); using zone outline"
        ol = ring_pts(zone.outline.outline, max_err)
        fills = [{"layer": i, "outer": ol, "holes": []} for i in layers]
    rings: dict[int, list] = {}
    for f in fills:
        rings.setdefault(f["layer"], []).extend([f["outer"], *f["holes"]])
    reach_pads = sorted(f"{p['ref']}.{p['number']}" for p in pads if p["net"] == code and any(
        l in rings and any(geom.point_in_rings(pt, rings[l]) for pt in pad_sample_points(p, l))
        for l in p["layers"] if l in layers))
    reach_vias = sorted(v["uuid"] for v in vias if v["net"] == code and any(
        l in rings and geom.point_in_rings(v["at"], rings[l]) for l in layers if _in_span(v["layers"], l)))
    cs, th = zone.proto.copper_settings, zone.proto.copper_settings.connection.thermal_spokes
    d = {"uuid": zone.id.value, "net": code, "layers": layers, "pads": reach_pads, "vias": reach_vias,
         "fills": fills,
         "clearance": int(cs.clearance.value_nm), "min_thickness": int(cs.min_thickness.value_nm),
         "thermal_gap": int(th.gap.value_nm) if th.HasField("gap") else _DEFAULT_THERMAL_NM,
         "spoke_width": int(th.width.value_nm) if th.HasField("width") else _DEFAULT_THERMAL_NM,
         "pad_connection": _ZC_TO_KICAD.get(cs.connection.zone_connection, 0)}
    if warn:
        d["warning"] = warn
    return d


def _in_span(span: list, layer: int) -> bool:
    lo, hi = sorted(span, key=_top_down)
    return _top_down(lo) <= _top_down(layer) <= _top_down(hi)


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
                     layer_roles: dict | None = None, dru_text: str | None = None) -> dict:
    """BoardModel dict of the board open in KiCad, same schema as ``harness.model.export_model``.

    `board`: ``kipy.board.Board``; `project`: its ``Project`` (the ``.kicad_dru`` next to it is used for
    `disallow` keepouts unless `dru_text` is given); `pcb_copy_text`: the ``save_as`` copy's text (layer
    types); `rules`: the resolved rules dict (``toporlib.rules``, made with the same net codes); `layer_roles`:
    ``cfg.layer_roles``. The zones should be refilled before the call (``Board.refill_zones``), otherwise a
    pour falls back to its outline with a warning. The returned ``layers`` carry ``kind``, ``role``,
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

    net_names = [n.name for n in board.get_nets()]
    codes = net_codes_by_name(net_names)
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
    track_ends = {}
    for t in tracks:
        track_ends.setdefault(t.net.name, []).append((cu_id(t.layer), (t.start.x, t.start.y), (t.end.x, t.end.y)))

    def is_free(v) -> bool:
        r = int(v.padstack.copper_layers[0].size.x) / 2.0 if v.padstack.copper_layers else 0
        for l, s, e in track_ends.get(v.net.name, ()):
            if any(math.hypot(q[0] - v.position.x, q[1] - v.position.y) <= r for q in (s, e)):
                return False
        return True

    vias = [via_to_model(v, codes, cu_order, free=is_free(v)) for v in board.get_vias()]

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
            x0, y0, x1, y1 = bb.pos.x, bb.pos.y, bb.pos.x + bb.size.x, bb.pos.y + bb.size.y
            copper_shapes.append({"uuid": t.id.value, "net": 0, "layer": i, "owner_ref": owner,
                                  "poly": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]})

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
    pad_by_net: dict[int, list] = {}
    for p in pads:
        pad_by_net.setdefault(p["net"], []).append(p)
    items = [geom.track_item(k["uuid"], k["net"], k["layer"], k["width"],
                             k["prim"]["a"] if k["prim"]["kind"] == "seg" else k["prim"]["start"],
                             k["prim"]["b"] if k["prim"]["kind"] == "seg" else k["prim"]["end"],
                             k["prim"].get("mid")) for k in kept]
    vias_by_net: dict[int, list] = {}
    for v in vias:
        vias_by_net.setdefault(v["net"], []).append(
            (v["at"], v["d"] / 2.0, {l for l in cu_order if _in_span(v["layers"], l)}))

    def in_pad(net, layer, pt) -> bool:
        return any(pad_contains(p, layer, pt) for p in pad_by_net.get(net, ()))

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
        "dangling_ends": geom.find_dangling(items, vias_by_net, in_pad),
        "rules": rules,
    }


__all__ = ["export_model_ipc", "layer_plan", "pad_from_kipy", "pad_shape", "pad_contains", "shape_to_prims",
           "shape_to_polys", "zone_to_keepouts", "zone_to_pour", "track_to_kept", "via_to_model", "ring_pts",
           "cu_id"]
