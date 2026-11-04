"""Export a loaded pcbnew board as the BoardModel JSON dict (KiCad frame, Y down, integer nm)."""
from __future__ import annotations

import json
import re
from pathlib import Path

import pcbnew

from toporlib.dru import dru_courtyard_clearances, merge_dru_keepouts, parse_dru
from toporlib.geom import find_dangling, hull, track_item

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


_hull = hull


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


def _pad_zone_fields(pad, fp) -> dict:
    """Pad-level zone connection overrides for the engine's pour-reach check: `zone_connection`
    (KiCad ZONE_CONNECTION, pad override else footprint override, absent when inherited),
    `thermal_gap` (nm, absent when inherited) and `spoke_angle_deg`."""
    out: dict = {}
    conn = int(pad.GetLocalZoneConnection())
    if conn == int(pcbnew.ZONE_CONNECTION_INHERITED):
        try:
            conn = int(fp.GetLocalZoneConnection())
        except AttributeError:
            conn = int(pcbnew.ZONE_CONNECTION_INHERITED)
    if conn != int(pcbnew.ZONE_CONNECTION_INHERITED):
        out["zone_connection"] = conn
    try:
        gap = pad.GetLocalThermalGapOverride()
    except AttributeError:
        gap = None
    if gap is not None and int(gap) > 0:
        out["thermal_gap"] = int(gap)
    try:
        out["spoke_angle_deg"] = float(pad.GetThermalSpokeAngle().AsDegrees())
    except AttributeError:
        pass
    return out


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
                    **_pad_zone_fields(pad, fp),
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
def find_dangling_ends(board) -> list[dict]:
    """Track/arc endpoints touching no same-net pad, via or other same-net track/arc
    (`toporlib.geom.find_dangling`; pad containment from pcbnew's effective pad polygons)."""
    items = []
    vias_by_net: dict[int, list] = {}
    for t in board.GetTracks():
        if _is_via(t):
            vias_by_net.setdefault(int(t.GetNetCode()), []).append(
                (_pt(t.GetPosition()), int(t.GetWidth(pcbnew.F_Cu)) / 2.0,
                 {int(l) for l in t.GetLayerSet().Seq()}))
            continue
        items.append(track_item(_uuid(t), t.GetNetCode(), t.GetLayer(), t.GetWidth(), _pt(t.GetStart()),
                                _pt(t.GetEnd()), _pt(t.GetMid()) if _is_arc(t) else None))

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

    return find_dangling(items, vias_by_net, in_pad)


# ---------------------------------------------------------------- pours
def _ring(chain) -> list[list[int]]:
    return [[int(chain.CPoint(i).x), int(chain.CPoint(i).y)] for i in range(chain.PointCount())]


def _pour_fills(z, layers: list[int]) -> tuple[list[dict], str | None]:
    """Outlines of the zone's filled copper per layer (outer ring + holes), nm, KiCad frame.
    KiCad hands the fill fractured (holes as zero-width slits of the outer ring), so `holes`
    is usually empty; the engine rasters the rings even-odd either way.

    Falls back to the zone's drawn outline (with a warning string) when the filled list is
    unavailable. O(total fill points).
    """
    fills: list[dict] = []
    try:
        for l in layers:
            ps = z.GetFilledPolysList(l)
            for k in range(ps.OutlineCount()):
                fills.append({"layer": l, "outer": _ring(ps.Outline(k)),
                              "holes": [_ring(ps.Hole(k, h)) for h in range(ps.HoleCount(k))]})
        return fills, None
    except AttributeError as e:
        warn = f"filled polygon list unavailable ({e}); using zone outline"
    ol = _ring(z.Outline().Outline(0))
    return [{"layer": l, "outer": ol, "holes": []} for l in layers], warn


def _zone_settings(z) -> dict:
    """Fill settings the engine's pour-reach check needs (nm; KiCad ZONE_CONNECTION int)."""
    clr = z.GetLocalClearance()
    return {
        "clearance": int(clr) if clr is not None else 0,
        "min_thickness": int(z.GetMinThickness()),
        "thermal_gap": int(z.GetThermalReliefGap()),
        "spoke_width": int(z.GetThermalReliefSpokeWidth()),
        "pad_connection": int(z.GetPadConnection()),
    }


def _export_pours(board, cu: list[int]) -> list[dict]:
    """Real copper pours (not rule areas, not teardrop zones, net != 0) with the pads and vias they
    connect to after a refill; a pad/via is listed only if it has copper on one of the zone's layers.

    Needs the zones filled and the connectivity built; both run here (the board is modified in
    memory only, never saved).
    """
    zones = [z for z in board.Zones() if not z.GetIsRuleArea() and not z.IsTeardropArea()
             and int(z.GetNetCode()) != 0]
    if not zones:
        return []
    pcbnew.ZONE_FILLER(board).Fill(board.Zones())
    conn = board.GetConnectivity()
    conn.Build(board)
    out = []
    for z in zones:
        layers = sorted(int(l) for l in z.GetLayerSet().Seq() if int(l) in cu)
        pads = sorted(f"{p.GetParentFootprint().GetReference()}.{p.GetNumber()}"
                      for p in conn.GetConnectedPads(z) if any(p.IsOnLayer(l) for l in layers))
        vias = sorted(_uuid(t) for t in conn.GetConnectedTracks(z)
                      if _is_via(t) and any(t.IsOnLayer(l) for l in layers))
        fills, warn = _pour_fills(z, layers)
        d = {"uuid": _uuid(z), "net": int(z.GetNetCode()), "layers": layers,
             "pads": pads, "vias": vias, "fills": fills, **_zone_settings(z)}
        if warn:
            d["warning"] = warn
        out.append(d)
    out.sort(key=lambda d: d["uuid"])
    return out


def _courtyard_regions(board, dru_text: str | None) -> list[dict]:
    """The courtyard-bound custom clearance rules of the .kicad_dru with the courtyard polygons."""
    if not dru_text:
        return []

    want = dru_courtyard_clearances(parse_dru(dru_text))
    out = []
    for name, ref, cl in want:
        for fp in board.GetFootprints():
            if fp.GetReference() != ref:
                continue
            lay = pcbnew.F_CrtYd if fp.GetLayer() == pcbnew.F_Cu else pcbnew.B_CrtYd
            ps = fp.GetCourtyard(lay)
            for i in range(ps.OutlineCount()):
                o = ps.Outline(i)
                poly = [[int(o.CPoint(k).x), int(o.CPoint(k).y)] for k in range(o.PointCount())]
                if len(poly) >= 3:
                    out.append({"name": name, "ref": ref, "poly": poly, "clearance": cl})
    return out


# ---------------------------------------------------------------- top level
def export_model(board, rules: dict, *, name: str, dru_text: str | None = None) -> dict:
    """The BoardModel dict. `dru_text` (the .kicad_dru) adds its `disallow` rules as keepouts."""
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
    rules = {**rules, "clearance_regions": _courtyard_regions(board, dru_text)}
    keepouts, rules = merge_dru_keepouts(_export_keepouts(board), rules, dru_text,
                                         {l["name"]: l["id"] for l in layers})
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
        "keepouts": keepouts,
        "net_ties": net_ties,
        "pours": _export_pours(board, cu),
        "dangling_ends": find_dangling_ends(board),
        "rules": rules,
    }


def write_model_json(model: dict, path) -> Path:
    path = Path(path)
    path.write_text(json.dumps(model))
    return path
