"""Export a loaded pcbnew board as the BoardModel JSON dict (KiCad frame, Y down, integer nm)."""
from __future__ import annotations

import json
import math
import re
from pathlib import Path

import pcbnew

_KIND = {
    pcbnew.LT_SIGNAL: "signal",
    pcbnew.LT_POWER: "power",
    pcbnew.LT_MIXED: "mixed",
    pcbnew.LT_JUMPER: "jumper",
}
_ATTR = {
    pcbnew.PAD_ATTRIB_PTH: "pth",
    pcbnew.PAD_ATTRIB_SMD: "smd",
    pcbnew.PAD_ATTRIB_CONN: "conn",
    pcbnew.PAD_ATTRIB_NPTH: "npth",
}
_HULL_SHAPES = {
    pcbnew.PAD_SHAPE_TRAPEZOID,
    pcbnew.PAD_SHAPE_CHAMFERED_RECT,
    pcbnew.PAD_SHAPE_CUSTOM,
}


def load_board(pc):
    """Load the project settings first (when a .kicad_pro exists), then the board."""
    if pc.pro is not None:
        pcbnew.GetSettingsManager().LoadProject(str(pc.pro))
    return pcbnew.LoadBoard(str(pc.pcb))


# ---------------------------------------------------------------- helpers
def _pt(p) -> list[int]:
    return [int(p.x), int(p.y)]


def _cu_layers(board) -> list[int]:
    return [int(l) for l in board.GetEnabledLayers().CuStack()]


def _outline_pts(poly_set, idx=0) -> list[list[int]]:
    o = poly_set.COutline(idx)
    return [[int(o.CPoint(i).x), int(o.CPoint(i).y)] for i in range(o.PointCount())]


def _hull(pts: list[list[int]]) -> list[list[int]]:
    """Convex hull (Andrew monotone chain)."""
    pts = sorted(set(map(tuple, pts)))
    if len(pts) <= 2:
        return [list(p) for p in pts]

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lo: list = []
    for p in pts:
        while len(lo) >= 2 and cross(lo[-2], lo[-1], p) <= 0:
            lo.pop()
        lo.append(p)
    up: list = []
    for p in reversed(pts):
        while len(up) >= 2 and cross(up[-2], up[-1], p) <= 0:
            up.pop()
        up.append(p)
    return [list(p) for p in lo[:-1] + up[:-1]]


def _uuid(item) -> str:
    return item.m_Uuid.AsString()


def _is_arc(t) -> bool:
    return t.GetClass() == "PCB_ARC"


def _is_via(t) -> bool:
    return t.GetClass() == "PCB_VIA"


# ---------------------------------------------------------------- net ties
_GROUPS_RE = re.compile(r"\(net_tie_pad_groups((?:\s+\"[^\"]*\")+)\s*\)")


def _net_tie_groups(fp) -> list[list[str]]:
    """Pad-number groups of a net-tie footprint, parsed from its s-expression text."""
    if not fp.IsNetTie():
        return []
    io = pcbnew.PCB_IO_KICAD_SEXPR()
    io.Format(fp)
    m = _GROUPS_RE.search(io.GetStringOutput(False))
    if not m:
        return []
    return [
        [n.strip() for n in g.split(",") if n.strip()]
        for g in re.findall(r'"([^"]*)"', m.group(1))
    ]


# ---------------------------------------------------------------- pads
def _pad_shape(pad, layer: int) -> dict:
    shp = pad.GetShape(layer)
    size = pad.GetSize(layer)
    w, h = int(size.x), int(size.y)
    off = pad.GetOffset(layer)
    if (off.x or off.y) and shp not in _HULL_SHAPES:
        shp = None  # offset shape: absolute hull below
    if shp == pcbnew.PAD_SHAPE_CIRCLE:
        return {"kind": "circle", "d": w}
    if shp == pcbnew.PAD_SHAPE_RECT:
        return {"kind": "rect", "w": w, "h": h}
    if shp == pcbnew.PAD_SHAPE_ROUNDRECT:
        return {"kind": "roundrect", "w": w, "h": h, "rc": int(pad.GetRoundRectCornerRadius(layer))}
    if shp == pcbnew.PAD_SHAPE_OVAL:
        return {"kind": "oval", "w": w, "h": h}
    poly = pad.GetEffectivePolygon(layer)
    pts = [p for i in range(poly.OutlineCount()) for p in _outline_pts(poly, i)]
    return {"kind": "poly", "pts": _hull(pts)}


def _export_pads(board, cu: list[int], tie_groups: dict):
    pads, holes, net_ties = [], [], []
    cu_set = set(cu)
    for fp in board.GetFootprints():
        ref = fp.GetReference()
        groups = tie_groups.get(ref, [])
        by_num: dict[str, int] = {}
        for gi, g in enumerate(groups):
            for n in g:
                by_num.setdefault(n, gi)
        for pad in fp.Pads():
            layers = [int(l) for l in pad.GetLayerSet().Seq() if int(l) in cu_set]
            attr = _ATTR.get(pad.GetAttribute(), "smd")
            ds = pad.GetDrillSize()
            drill = {"w": int(ds.x), "h": int(ds.y)} if ds.x > 0 and ds.y > 0 else None
            lc = pad.GetLocalClearance()
            num = pad.GetNumber()
            pos = _pt(pad.GetPosition())
            pads.append(
                {
                    "uuid": _uuid(pad),
                    "ref": ref,
                    "number": num,
                    "net": int(pad.GetNetCode()),
                    "attr": attr,
                    "center": pos,
                    "rotation_deg": float(pad.GetOrientation().AsDegrees()),
                    "layers": layers,
                    "shapes": {str(l): _pad_shape(pad, l) for l in layers},
                    "drill": drill,
                    "local_clearance": int(lc) if lc is not None else None,
                    "net_tie_group": by_num.get(num) if groups else None,
                }
            )
            if drill is not None:
                hole = {
                    "at": pos,
                    "d": max(drill["w"], drill["h"]),
                    "kind": "pth" if attr in ("pth", "conn") else "npth",
                }
                if drill["w"] != drill["h"]:
                    hole["slot"] = [drill["w"], drill["h"]]
                holes.append(hole)
        if groups:
            pnets = {p.GetNumber(): int(p.GetNetCode()) for p in fp.Pads()}
            nets = []
            for g in groups:
                seen: list[int] = []
                for n in g:
                    c = pnets.get(n, 0)
                    if c and c not in seen:
                        seen.append(c)
                nets.append(seen)
            net_ties.append({"ref": ref, "groups": groups, "nets": nets})
    return pads, holes, net_ties


# ---------------------------------------------------------------- tracks / vias
def _track_prim(t) -> dict:
    if _is_arc(t):
        return {"kind": "arc", "start": _pt(t.GetStart()), "mid": _pt(t.GetMid()), "end": _pt(t.GetEnd())}
    return {"kind": "seg", "a": _pt(t.GetStart()), "b": _pt(t.GetEnd())}


def _export_copper(board):
    kept, vias = [], []
    for t in board.GetTracks():
        if _is_via(t):
            vias.append(
                {
                    "uuid": _uuid(t),
                    "net": int(t.GetNetCode()),
                    "at": _pt(t.GetPosition()),
                    "d": int(t.GetWidth(pcbnew.F_Cu)),
                    "drill": int(t.GetDrillValue()),
                    "layers": [int(t.TopLayer()), int(t.BottomLayer())],
                    "free": bool(t.GetIsFree()),
                    "locked": bool(t.IsLocked()),
                }
            )
        else:
            kept.append(
                {
                    "uuid": _uuid(t),
                    "net": int(t.GetNetCode()),
                    "layer": int(t.GetLayer()),
                    "width": int(t.GetWidth()),
                    "prim": _track_prim(t),
                    "locked": bool(t.IsLocked()),
                }
            )
    return kept, vias


# ---------------------------------------------------------------- graphics
def _graphics(board):
    """(item, owner_ref|None) for board drawings and footprint graphical items."""
    for d in board.GetDrawings():
        yield d, None
    for fp in board.GetFootprints():
        ref = fp.GetReference()
        for g in fp.GraphicalItems():
            yield g, ref


def _export_copper_shapes(board, cu: list[int], max_err: int):
    cu_set = set(cu)
    out = []
    for item, owner in _graphics(board):
        layer = int(item.GetLayer())
        if layer not in cu_set:
            continue
        cls = item.GetClass()
        ps = pcbnew.SHAPE_POLY_SET()
        if cls == "PCB_SHAPE":
            item.TransformShapeToPolygon(ps, item.GetLayer(), 0, max_err, pcbnew.ERROR_INSIDE)
        elif cls == "PCB_TEXT":
            item.TransformTextToPolySet(ps, 0, max_err, pcbnew.ERROR_INSIDE)
        else:
            continue
        for i in range(ps.OutlineCount()):
            out.append(
                {
                    "uuid": _uuid(item),
                    "net": int(item.GetNetCode()) if hasattr(item, "GetNetCode") else 0,
                    "layer": layer,
                    "poly": _outline_pts(ps, i),
                    "owner_ref": owner,
                }
            )
    return out


def _seg(a, b) -> dict:
    return {"kind": "seg", "a": _pt(a), "b": _pt(b)}


def _export_outline(board, max_err: int):
    out = []
    for item, _ in _graphics(board):
        if item.GetClass() != "PCB_SHAPE" or int(item.GetLayer()) != int(pcbnew.Edge_Cuts):
            continue
        s = item.GetShape()
        if s == pcbnew.SHAPE_T_SEGMENT:
            out.append(_seg(item.GetStart(), item.GetEnd()))
        elif s == pcbnew.SHAPE_T_ARC:
            out.append({"kind": "arc", "start": _pt(item.GetStart()), "mid": _pt(item.GetArcMid()),
                        "end": _pt(item.GetEnd())})
        elif s == pcbnew.SHAPE_T_RECTANGLE:
            c = [item.GetStart(), pcbnew.VECTOR2I(item.GetEnd().x, item.GetStart().y),
                 item.GetEnd(), pcbnew.VECTOR2I(item.GetStart().x, item.GetEnd().y)]
            out += [_seg(c[i], c[(i + 1) % 4]) for i in range(4)]
        elif s == pcbnew.SHAPE_T_BEZIER:
            pts = list(item.GetBezierPoints())
            out += [_seg(pts[i], pts[i + 1]) for i in range(len(pts) - 1)]
        else:  # circle / polygon: flattened outline
            ps = pcbnew.SHAPE_POLY_SET()
            item.TransformShapeToPolygon(ps, item.GetLayer(), 0, max_err, pcbnew.ERROR_INSIDE)
            for i in range(ps.OutlineCount()):
                pts = _outline_pts(ps, i)
                out += [{"kind": "seg", "a": pts[j], "b": pts[(j + 1) % len(pts)]} for j in range(len(pts))]
    return out


def _rule_areas(board):
    """Board rule areas, then rule areas owned by footprints (e.g. a module's antenna keepout)."""
    yield from board.Zones()
    for fp in board.GetFootprints():
        yield from fp.Zones()


def _export_keepouts(board):
    out = []
    for z in _rule_areas(board):
        if not z.GetIsRuleArea():
            continue
        ps = z.Outline()
        for i in range(ps.OutlineCount()):
            out.append(
                {
                    "name": z.GetZoneName(),
                    "layers": [int(l) for l in z.GetLayerSet().Seq()],
                    "poly": _outline_pts(ps, i),
                    "no_tracks": bool(z.GetDoNotAllowTracks()),
                    "no_vias": bool(z.GetDoNotAllowVias()),
                }
            )
    return out


# ---------------------------------------------------------------- dangling ends
def _arc_center(s, m, e):
    ax, ay = s
    bx, by = m
    cx, cy = e
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if d == 0:
        return None
    a2, b2, c2 = ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy
    ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d
    uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d
    return ux, uy


def _dist_pt_seg(p, a, b) -> float:
    dx, dy = b[0] - a[0], b[1] - a[1]
    L2 = dx * dx + dy * dy
    if L2 == 0:
        return math.hypot(p[0] - a[0], p[1] - a[1])
    t = max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L2))
    return math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy))


def _on_arc_sweep(p, c, s, m, e) -> bool:
    """True when the ray c->p lies within the angular sweep s->m->e."""
    tau = 2 * math.pi

    def ang(q):
        return math.atan2(q[1] - c[1], q[0] - c[0])

    a_s, a_m, a_e, a_p = ang(s), ang(m), ang(e), ang(p)
    if (a_m - a_s) % tau <= (a_e - a_s) % tau:  # counter-clockwise in atan2 frame
        return (a_p - a_s) % tau <= (a_e - a_s) % tau
    return (a_s - a_p) % tau <= (a_s - a_e) % tau


def _dist_pt_item(p, it) -> float:
    if it["arc"] is None:
        return _dist_pt_seg(p, it["s"], it["e"])
    c, r = it["arc"]
    if _on_arc_sweep(p, c, it["s"], it["m"], it["e"]):
        return abs(math.hypot(p[0] - c[0], p[1] - c[1]) - r)
    return min(math.hypot(p[0] - it["s"][0], p[1] - it["s"][1]),
               math.hypot(p[0] - it["e"][0], p[1] - it["e"][1]))


def _direction(it, end_is_e: bool):
    s, e = it["s"], it["e"]
    far = e if end_is_e else s
    near = s if end_is_e else e
    if it["arc"] is None:
        dx, dy = far[0] - near[0], far[1] - near[1]
    else:
        c, _ = it["arc"]
        dx, dy = -(far[1] - c[1]), far[0] - c[0]
        m = it["m"]
        if dx * (far[0] - m[0]) + dy * (far[1] - m[1]) < 0:
            dx, dy = -dx, -dy
    n = math.hypot(dx, dy) or 1.0
    return [dx / n, dy / n]


def find_dangling_ends(board) -> list[dict]:
    """Track/arc endpoints touching no same-net pad, via or other same-net track/arc.

    Same-net items are bucketed per (net, layer), so cost is linear in the item count
    for boards whose nets are small (the normal case).
    """
    items = []
    vias_by_net: dict[int, list] = {}
    for t in board.GetTracks():
        if _is_via(t):
            vias_by_net.setdefault(int(t.GetNetCode()), []).append(
                (_pt(t.GetPosition()), int(t.GetWidth(pcbnew.F_Cu)) / 2.0,
                 {int(l) for l in t.GetLayerSet().Seq()}))
            continue
        s, e = _pt(t.GetStart()), _pt(t.GetEnd())
        it = {"uuid": _uuid(t), "net": int(t.GetNetCode()), "layer": int(t.GetLayer()),
              "w": int(t.GetWidth()), "s": s, "e": e, "m": None, "arc": None}
        if _is_arc(t):
            it["m"] = _pt(t.GetMid())
            c = _arc_center(s, it["m"], e)
            if c is not None:
                it["arc"] = (c, math.hypot(s[0] - c[0], s[1] - c[1]))
        items.append(it)

    pads_by_net: dict[int, list] = {}
    for fp in board.GetFootprints():
        for pad in fp.Pads():
            pads_by_net.setdefault(int(pad.GetNetCode()), []).append(pad)
    poly_cache: dict = {}

    def in_pad(net, layer, p) -> bool:
        v = pcbnew.VECTOR2I(p[0], p[1])
        for pad in pads_by_net.get(net, ()):
            if not pad.IsOnLayer(layer):
                continue
            key = (_uuid(pad), layer)
            if key not in poly_cache:
                poly_cache[key] = pad.GetEffectivePolygon(layer)
            if poly_cache[key].Contains(v):
                return True
        return False

    groups: dict[tuple, list] = {}
    for it in items:
        groups.setdefault((it["net"], it["layer"]), []).append(it)

    out = []
    for it in items:
        net, layer = it["net"], it["layer"]
        same = groups[(net, layer)]
        for end_is_e in (False, True):
            p = it["e"] if end_is_e else it["s"]
            if net and in_pad(net, layer, p):
                continue
            if any(layer in ls and math.hypot(p[0] - c[0], p[1] - c[1]) <= r
                   for c, r, ls in vias_by_net.get(net, ())):
                continue
            if any(o is not it and (p in (o["s"], o["e"]) or _dist_pt_item(p, o) <= o["w"] / 2.0)
                   for o in same):
                continue
            out.append({"item": it["uuid"], "net": net, "layer": layer, "width": it["w"],
                        "end": list(p), "direction": _direction(it, end_is_e)})
    return out


# ---------------------------------------------------------------- top level
def export_model(board, rules: dict, *, name: str) -> dict:
    cu = _cu_layers(board)
    max_err = int(board.GetDesignSettings().m_MaxError) or 5000
    layers = []
    for l in cu:
        kind = _KIND.get(board.GetLayerType(l), "signal")
        layers.append({"id": l, "name": board.GetLayerName(l), "kind": kind,
                       "routable": kind in ("signal", "mixed")})
    nets = [{"code": int(c), "name": str(n.GetNetname())}
            for c, n in sorted(board.GetNetsByNetcode().items()) if int(c) != 0]
    tie_groups = {fp.GetReference(): _net_tie_groups(fp) for fp in board.GetFootprints() if fp.IsNetTie()}
    pads, holes, net_ties = _export_pads(board, cu, tie_groups)
    kept, vias = _export_copper(board)
    return {
        "units": "nm",
        "board": name,
        "layers": layers,
        "nets": nets,
        "pads": pads,
        "vias": vias,
        "kept": kept,
        "copper_shapes": _export_copper_shapes(board, cu, max_err),
        "holes": holes,
        "outline": _export_outline(board, max_err),
        "keepouts": _export_keepouts(board),
        "net_ties": net_ties,
        "dangling_ends": find_dangling_ends(board),
        "rules": rules,
    }


def write_model_json(model: dict, path) -> Path:
    path = Path(path)
    path.write_text(json.dumps(model))
    return path
