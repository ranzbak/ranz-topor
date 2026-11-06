"""plugin/topor/model_ipc.py: the pure mapping functions fed with kipy wrappers built from protos (exact nm
values), layer roles (M4-R7), and export_model_ipc end to end against a stub of kipy's Board; the result
goes through the engine's model loader."""
import json
import math

import pytest

kipy = pytest.importorskip("kipy")
from google.protobuf.any_pb2 import Any
from kipy.board_types import (ArcTrack, BoardArc, BoardBezier, BoardCircle, BoardLayer, BoardPolygon, BoardRectangle,
                              BoardSegment, FootprintInstance, Pad, Track, Via, Zone)
from kipy.proto.board import board_types_pb2 as bt
from kipy.proto.common.types import base_types_pb2 as bp

from topor import model_ipc as M
from toporlib import rules as R

MM = 1_000_000
PSS, PT = bt.PadStackShape, bt.PadType
FCU, BCU, IN1 = BoardLayer.BL_F_Cu, BoardLayer.BL_B_Cu, BoardLayer.BL_In1_Cu
CODES = {"GND": 1, "VCC": 2}


def v2(m, x, y):
    m.x_nm, m.y_nm = x, y


def mk_pad(number="1", at=(0, 0), shape=PSS.PSS_RECTANGLE, size=(MM, 2 * MM), layers=(FCU,), net="GND",
           ptype=PT.PT_SMD, angle=0.0, ratio=0.0, drill=None, offset=(0, 0), clearance=None, uid="pad-" + "1"):
    p = bt.Pad()
    p.id.value = uid
    p.number = number
    p.type = ptype
    p.net.name = net
    v2(p.position, *at)
    ps = p.pad_stack
    ps.type = bt.PadStackType.PST_NORMAL
    ps.layers.extend(layers)
    ps.angle.value_degrees = angle
    cl = ps.copper_layers.add()
    cl.layer = FCU
    cl.shape = shape
    v2(cl.size, *size)
    v2(cl.offset, *offset)
    cl.corner_rounding_ratio = ratio
    if drill:
        v2(ps.drill.diameter, *drill)
    if clearance is not None:
        p.copper_clearance_override.value_nm = clearance
    return Pad(p)


def no_poly(pad, bl):
    raise AssertionError("polygon not expected")


def conv(pad, **kw):
    kw = {"ref": "U1", "net_codes": CODES, "cu_ids": {0, 2}, "poly_fn": no_poly, **kw}
    return M.pad_from_kipy(pad, **kw)


# ---------------------------------------------------------------- pads
def test_pad_native_shapes_and_fields():
    pd, hole = conv(mk_pad(shape=PSS.PSS_RECTANGLE, layers=(FCU,), angle=30.0, clearance=300_000, at=(1_000_001, -5)))
    assert pd == {"uuid": "pad-1", "ref": "U1", "number": "1", "net": 1, "attr": "smd", "center": [1_000_001, -5],
                  "rotation_deg": 30.0, "layers": [0], "shapes": {"0": {"kind": "rect", "w": MM, "h": 2 * MM}},
                  "drill": None, "local_clearance": 300_000, "net_tie_group": None}
    assert hole is None
    pd, _ = conv(mk_pad(shape=PSS.PSS_ROUNDRECT, ratio=0.25))
    assert pd["shapes"]["0"] == {"kind": "roundrect", "w": MM, "h": 2 * MM, "rc": 250_000}
    pd, _ = conv(mk_pad(shape=PSS.PSS_OVAL, size=(MM, 3 * MM)))
    assert pd["shapes"]["0"] == {"kind": "oval", "w": MM, "h": 3 * MM}
    pd, _ = conv(mk_pad(shape=PSS.PSS_CIRCLE, size=(1_600_000, 1_600_000)))
    assert pd["shapes"]["0"] == {"kind": "circle", "d": 1_600_000}


def test_pad_pth_drill_slot_layers_and_net_none():
    pd, hole = conv(mk_pad(ptype=PT.PT_PTH, layers=(FCU, BCU, IN1), drill=(800_000, 800_000), shape=PSS.PSS_CIRCLE,
                           size=(MM, MM), net=""))
    assert pd["layers"] == [0, 2] and set(pd["shapes"]) == {"0", "2"}  # In1 is not an enabled layer
    assert pd["net"] == 0 and pd["attr"] == "pth" and pd["drill"] == {"w": 800_000, "h": 800_000}
    assert hole == {"at": [0, 0], "d": 800_000, "kind": "pth"}
    _, hole = conv(mk_pad(ptype=PT.PT_NPTH, drill=(600_000, 1_200_000)))
    assert hole == {"at": [0, 0], "d": 1_200_000, "kind": "npth", "slot": [600_000, 1_200_000]}


def test_pad_poly_shapes_use_kicad_polygon_hull():
    ring = [(0, 0), (4 * MM, 0), (4 * MM, MM), (2 * MM, MM), (2 * MM, 2 * MM), (0, 2 * MM)]  # concave custom pad
    pd, _ = conv(mk_pad(shape=PSS.PSS_CUSTOM), poly_fn=lambda pad, bl: [[list(p) for p in ring]])
    pts = sorted(map(tuple, pd["shapes"]["0"]["pts"]))
    assert pd["shapes"]["0"]["kind"] == "poly"
    assert pts == sorted([(0, 0), (4 * MM, 0), (4 * MM, MM), (2 * MM, 2 * MM), (0, 2 * MM)])  # hull drops (2,1)
    # an offset rect is absolute geometry too
    pd, _ = conv(mk_pad(shape=PSS.PSS_RECTANGLE, offset=(100_000, 0)), poly_fn=lambda p, b: [[[0, 0], [2, 0], [2, 2]]])
    assert pd["shapes"]["0"]["kind"] == "poly"
    for shp in (PSS.PSS_TRAPEZOID, PSS.PSS_CHAMFEREDRECT):
        pd, _ = conv(mk_pad(shape=shp), poly_fn=lambda p, b: [[[0, 0], [MM, 0], [MM, MM]]])
        assert pd["shapes"]["0"]["kind"] == "poly"
    with pytest.raises(ValueError):
        conv(mk_pad(shape=PSS.PSS_CUSTOM), poly_fn=lambda p, b: None)


def test_pad_zone_settings_and_net_tie_group():
    p = mk_pad()
    zs = p.padstack.proto.zone_settings if hasattr(p.padstack, "proto") else None
    zs.zone_connection = bt.ZoneConnectionStyle.ZCS_FULL
    zs.thermal_spokes.gap.value_nm = 400_000
    zs.thermal_spokes.angle.value_degrees = 45.0
    pd, _ = conv(p, tie_group=2)
    # pcbnew's ZONE_CONNECTION values, as the harness exports them and the engine reads them (pourcheck.rs CONN_*):
    # NONE 0, THERMAL 1, FULL 2, THT_THERMAL 3 (K1: the table was off by one, FULL read as THT_THERMAL)
    assert (pd["zone_connection"], pd["thermal_gap"], pd["spoke_angle_deg"], pd["net_tie_group"]) == (2, 400_000, 45.0, 2)
    for style, want in ((bt.ZoneConnectionStyle.ZCS_NONE, 0), (bt.ZoneConnectionStyle.ZCS_THERMAL, 1),
                        (bt.ZoneConnectionStyle.ZCS_PTH_THERMAL, 3)):
        q = mk_pad()
        q.padstack.proto.zone_settings.zone_connection = style
        assert conv(q)[0]["zone_connection"] == want, style
    pd, _ = conv(mk_pad(), fp_zone_connection=0)  # inherited from the pad -> the footprint's NONE
    assert pd["zone_connection"] == 0 and "thermal_gap" not in pd and "spoke_angle_deg" not in pd
    pd, _ = conv(mk_pad())
    assert "zone_connection" not in pd


def test_pad_contains_rotation_and_shapes():
    pad = {"center": [0, 0], "rotation_deg": 90.0, "shapes": {"0": {"kind": "rect", "w": 4 * MM, "h": 2 * MM}}}
    assert M.pad_contains(pad, 0, [0, int(1.9 * MM)]) and not M.pad_contains(pad, 0, [int(1.9 * MM), 0])  # 90 deg
    assert not M.pad_contains(pad, 2, [0, 0])
    c = {"center": [10, 10], "rotation_deg": 0.0, "shapes": {"0": {"kind": "circle", "d": MM}}}
    assert M.pad_contains(c, 0, [10 + MM // 2, 10]) and not M.pad_contains(c, 0, [10 + MM // 2 + 1, 10])
    rr = {"center": [0, 0], "rotation_deg": 0.0, "shapes": {"0": {"kind": "roundrect", "w": 2 * MM, "h": 2 * MM, "rc": MM // 2}}}
    assert M.pad_contains(rr, 0, [MM - 1, 0]) and not M.pad_contains(rr, 0, [MM - 1, MM - 1])  # rounded corner
    o = {"center": [0, 0], "rotation_deg": 0.0, "shapes": {"0": {"kind": "oval", "w": 4 * MM, "h": 2 * MM}}}
    assert M.pad_contains(o, 0, [2 * MM - 1, 0]) and not M.pad_contains(o, 0, [2 * MM - 1, MM - 1])
    po = {"center": [0, 0], "rotation_deg": 0.0, "shapes": {"0": {"kind": "poly", "pts": [[0, 0], [10, 0], [0, 10]]}}}
    assert M.pad_contains(po, 0, [2, 2]) and not M.pad_contains(po, 0, [9, 9])


# ---------------------------------------------------------------- tracks / vias
def test_track_arc_via_mapping():
    t = bt.Track()
    t.id.value = "t1"
    v2(t.start, 1_000_001, 2)
    v2(t.end, 3 * MM, -4)
    t.width.value_nm = 250_000
    t.layer = BCU
    t.net.name = "VCC"
    t.locked = bp.LockedState.LS_LOCKED
    k = M.track_to_kept(Track(t), CODES)
    assert k == {"uuid": "t1", "net": 2, "layer": 2, "width": 250_000, "locked": True,
                 "prim": {"kind": "seg", "a": [1_000_001, 2], "b": [3 * MM, -4]}}
    a = bt.Arc()
    a.id.value = "a1"
    v2(a.start, 0, 0)
    v2(a.mid, MM, MM)
    v2(a.end, 2 * MM, 0)
    a.width.value_nm = 200_000
    a.layer = FCU
    a.net.name = "GND"
    k = M.track_to_kept(ArcTrack(a), CODES)
    assert k["prim"] == {"kind": "arc", "start": [0, 0], "mid": [MM, MM], "end": [2 * MM, 0]}
    assert (k["layer"], k["net"], k["locked"]) == (0, 1, False)

    v = Via()
    v.position = kipy.geometry.Vector2.from_xy(5 * MM, 6 * MM)
    v.diameter = 800_000
    v.drill_diameter = 400_000
    v.net = kipy.board_types.Net(name="GND")
    v.locked = True
    got = M.via_to_model(v, CODES, [0, 4, 2], free=True)
    assert got["at"] == [5 * MM, 6 * MM] and (got["d"], got["drill"], got["layers"]) == (800_000, 400_000, [0, 2])
    assert got["net"] == 1 and got["free"] and got["locked"] and got["uuid"] == v.id.value
    v.type = kipy.board_types.ViaType.VT_BLIND_BURIED
    v.padstack.drill.start_layer = IN1
    v.padstack.drill.end_layer = FCU
    assert M.via_to_model(v, CODES, [0, 4, 2])["layers"] == [0, 4]


# ---------------------------------------------------------------- graphics
def seg_shape(a, b, layer=FCU, width=250_000):
    s = BoardSegment()
    s.start = kipy.geometry.Vector2.from_xy(*a)
    s.end = kipy.geometry.Vector2.from_xy(*b)
    s.layer = layer
    s.attributes.stroke.width = width
    return s


def test_shape_to_prims_exact_kinds():
    assert M.shape_to_prims(seg_shape((1, 2), (3_000_001, 4)), 5000) == [
        {"kind": "seg", "a": [1, 2], "b": [3_000_001, 4]}]
    arc = BoardArc()
    arc.start = kipy.geometry.Vector2.from_xy(0, 0)
    arc.mid = kipy.geometry.Vector2.from_xy(MM, MM)
    arc.end = kipy.geometry.Vector2.from_xy(2 * MM, 0)
    assert M.shape_to_prims(arc, 5000) == [{"kind": "arc", "start": [0, 0], "mid": [MM, MM], "end": [2 * MM, 0]}]
    r = BoardRectangle()
    r.top_left = kipy.geometry.Vector2.from_xy(0, 0)
    r.bottom_right = kipy.geometry.Vector2.from_xy(10 * MM, 5 * MM)
    segs = M.shape_to_prims(r, 5000)
    assert [(s["a"], s["b"]) for s in segs] == [([0, 0], [10 * MM, 0]), ([10 * MM, 0], [10 * MM, 5 * MM]),
                                                ([10 * MM, 5 * MM], [0, 5 * MM]), ([0, 5 * MM], [0, 0])]


def test_shape_to_prims_flattened_closed_and_bounded():
    c = BoardCircle()
    c.center = kipy.geometry.Vector2.from_xy(0, 0)
    c.radius_point = kipy.geometry.Vector2.from_xy(5 * MM, 0)
    segs = M.shape_to_prims(c, 5000)
    assert len(segs) >= 8 and all(abs(math.hypot(*s["a"]) - 5 * MM) <= 1 for s in segs)
    assert segs[-1]["b"] == segs[0]["a"]  # closed
    b = BoardBezier()
    b.start, b.control1 = kipy.geometry.Vector2.from_xy(0, 0), kipy.geometry.Vector2.from_xy(0, 5 * MM)
    b.control2, b.end = kipy.geometry.Vector2.from_xy(5 * MM, 5 * MM), kipy.geometry.Vector2.from_xy(5 * MM, 0)
    segs = M.shape_to_prims(b, 5000)
    assert segs[0]["a"] == [0, 0] and segs[-1]["b"] == [5 * MM, 0]
    assert all(segs[i]["b"] == segs[i + 1]["a"] for i in range(len(segs) - 1))
    pb = bt.BoardGraphicShape()
    pwh = bp.PolygonWithHoles()
    for x, y in ((0, 0), (MM, 0), (MM, MM)):
        n = pwh.outline.nodes.add()
        v2(n.point, x, y)
    pb.shape.polygon.polygons.append(pwh)
    assert [(s["a"], s["b"]) for s in M.shape_to_prims(BoardPolygon(pb), 5000)] == [
        ([0, 0], [MM, 0]), ([MM, 0], [MM, MM]), ([MM, MM], [0, 0])]


def test_shape_to_polys_capsule_and_filled_rect():
    """M4-R8 / C25: obstacles CONTAIN the shape (circumscribed), with bounded excess (full check against pcbnew:
    tests/plugin/test_ipc_conservative.py)."""
    err = min(1000, M.OBSTACLE_ERR_NM)
    (ring,) = M.shape_to_polys(seg_shape((0, 0), (10 * MM, 0), width=200_000), 1000)
    xs, ys = [p[0] for p in ring], [p[1] for p in ring]
    assert -100_000 - err - 2 <= min(xs) <= -100_000 and 10 * MM + 100_000 <= max(xs) <= 10 * MM + 100_000 + err + 2
    assert -100_000 - err - 2 <= min(ys) <= -100_000 and 100_000 <= max(ys) <= 100_000 + err + 2
    for q in ring:  # every vertex on or outside the true capsule, by at most err (+ rounding)
        d = math.hypot(q[0] - min(max(q[0], 0), 10 * MM), q[1])
        assert 100_000 - 1 <= d <= 100_000 + err + 2, (q, d)
    r = BoardRectangle()
    r.top_left = kipy.geometry.Vector2.from_xy(2 * MM, MM)  # corners given in any order: normalised
    r.bottom_right = kipy.geometry.Vector2.from_xy(0, 0)
    r.attributes.stroke.width = 200_000
    r.attributes.fill.filled = True
    (box,) = M.shape_to_polys(r, 1000)
    xs, ys = [p[0] for p in box], [p[1] for p in box]
    assert (min(xs), max(xs), min(ys), max(ys)) == (-100_001, 2_100_001, -100_001, 1_100_001)
    for q in box:  # rounded corners of radius w/2 (KiCad's stroke), circumscribed
        dx, dy = max(-q[0], q[0] - 2 * MM, 0), max(-q[1], q[1] - MM, 0)
        assert dx == 0 or dy == 0 or 100_000 - 1 <= math.hypot(dx, dy) <= 100_000 + err + 2, q
    r.attributes.fill.filled = False
    assert len(M.shape_to_polys(r, 1000)) == 4  # four stroked sides


def test_pad_sample_points_lie_inside_the_pad():
    """Pour-reach probes never lie outside the pad copper (C26: reach errs toward 'not reached')."""
    for sh in ({"kind": "circle", "d": MM}, {"kind": "oval", "w": 3 * MM, "h": MM},
               {"kind": "roundrect", "w": 2 * MM, "h": MM, "rc": 400_000}, {"kind": "rect", "w": 2 * MM, "h": MM}):
        for rot in (0.0, 30.0, 90.0):
            pad = {"center": [5 * MM, 7 * MM], "rotation_deg": rot, "shapes": {"0": sh}}
            pts = M.pad_sample_points(pad, 0)
            assert len(pts) == 9 and all(M.pad_contains(pad, 0, q) for q in pts), (sh, rot)
    # a fill touching only the diagonal box corner of a round pad (outside its copper) does not reach it
    pad = {"ref": "U1", "number": "1", "net": 1, "layers": [0], "center": [0, 0], "rotation_deg": 0.0,
           "shapes": {"0": {"kind": "circle", "d": MM}}}
    corner = [[440_000, 440_000], [600_000, 440_000], [600_000, 600_000], [440_000, 600_000]]
    assert M.pour_reach(1, [0], [{"layer": 0, "outer": corner, "holes": []}], [pad], []) == ([], [])


# ---------------------------------------------------------------- zones
SQ = [(0, 0), (10 * MM, 0), (10 * MM, 10 * MM), (0, 10 * MM)]


def mk_pwh(pts):
    p = bp.PolygonWithHoles()
    for x, y in pts:
        v2(p.outline.nodes.add().point, x, y)
    p.outline.closed = True
    return p


def mk_zone(kind, layers, net="", filled=None, tracks=False, vias=False, name="z", uid="z-1", outline=SQ):
    z = bt.Zone()
    z.id.value, z.type, z.name = uid, kind, name
    z.layers.extend(layers)
    z.outline.polygons.append(mk_pwh(outline))
    if kind == bt.ZoneType.ZT_RULE_AREA:
        z.rule_area_settings.keepout_tracks = tracks
        z.rule_area_settings.keepout_vias = vias
    else:
        z.copper_settings.net.name = net
        z.copper_settings.clearance.value_nm = 300_000
        z.copper_settings.min_thickness.value_nm = 250_000
        z.copper_settings.connection.zone_connection = bt.ZoneConnectionStyle.ZCS_THERMAL
        z.copper_settings.connection.thermal_spokes.gap.value_nm = 400_000
        z.copper_settings.connection.thermal_spokes.width.value_nm = 350_000
    for layer, pwhs in (filled or {}).items():
        f = z.filled_polygons.add()
        f.layer = layer
        f.shapes.polygons.extend(pwhs)
    return Zone(z)


def test_zone_to_keepouts():
    z = mk_zone(bt.ZoneType.ZT_RULE_AREA, [FCU, BCU, IN1], tracks=True, name="ant")
    z.proto.outline.polygons.append(mk_pwh([(20 * MM, 0), (21 * MM, 0), (21 * MM, MM)]))
    ks = M.zone_to_keepouts(z, {0, 2}, 5000)
    assert ks == [
        {"name": "ant", "layers": [0, 2], "poly": [list(p) for p in SQ], "no_tracks": True, "no_vias": False,
         "no_pour": False, "no_pads": False, "no_footprints": False},
        {"name": "ant", "layers": [0, 2], "poly": [[20 * MM, 0], [21 * MM, 0], [21 * MM, MM]], "no_tracks": True,
         "no_vias": False, "no_pour": False, "no_pads": False, "no_footprints": False}]


def test_zone_to_pour_fill_reach_and_settings():
    hole = bp.PolyLine()
    for x, y in ((4 * MM, 4 * MM), (6 * MM, 4 * MM), (6 * MM, 6 * MM), (4 * MM, 6 * MM)):
        v2(hole.nodes.add().point, x, y)
    fill = mk_pwh(SQ)
    fill.holes.append(hole)
    z = mk_zone(bt.ZoneType.ZT_COPPER, [FCU, BCU], net="GND", filled={FCU: [fill]})
    pads = [
        {"ref": "R1", "number": "1", "net": 1, "layers": [0], "center": [MM, MM], "rotation_deg": 0.0,
         "shapes": {"0": {"kind": "rect", "w": MM, "h": MM}}},  # inside the fill
        {"ref": "R1", "number": "2", "net": 1, "layers": [0], "center": [5 * MM, 5 * MM], "rotation_deg": 0.0,
         "shapes": {"0": {"kind": "rect", "w": MM, "h": MM}}},  # in the hole
        {"ref": "R2", "number": "1", "net": 2, "layers": [0], "center": [MM, 2 * MM], "rotation_deg": 0.0,
         "shapes": {"0": {"kind": "rect", "w": MM, "h": MM}}},  # other net
        {"ref": "R3", "number": "1", "net": 1, "layers": [2], "center": [MM, MM], "rotation_deg": 0.0,
         "shapes": {"2": {"kind": "rect", "w": MM, "h": MM}}},  # B.Cu: the zone has no fill there
    ]
    vias = [{"uuid": "v1", "net": 1, "at": [2 * MM, 2 * MM], "layers": [0, 2]},
            {"uuid": "v2", "net": 1, "at": [5 * MM, 5 * MM], "layers": [0, 2]},
            {"uuid": "v3", "net": 2, "at": [2 * MM, 2 * MM], "layers": [0, 2]}]
    d = M.zone_to_pour(z, net_codes=CODES, cu_ids={0, 2}, pads=pads, vias=vias, max_err=5000)
    assert d["uuid"] == "z-1" and d["net"] == 1 and d["layers"] == [0, 2]
    assert d["pads"] == ["R1.1"] and d["vias"] == ["v1"]
    assert d["fills"] == [{"layer": 0, "outer": [list(p) for p in SQ],
                           "holes": [[[4 * MM, 4 * MM], [6 * MM, 4 * MM], [6 * MM, 6 * MM], [4 * MM, 6 * MM]]]}]
    assert (d["clearance"], d["min_thickness"], d["thermal_gap"], d["spoke_width"], d["pad_connection"]) == (
        300_000, 250_000, 400_000, 350_000, 1)  # THERMAL = 1 (pcbnew / engine), K1
    assert "warning" not in d
    unfilled = mk_zone(bt.ZoneType.ZT_COPPER, [FCU], net="GND")
    d = M.zone_to_pour(unfilled, net_codes=CODES, cu_ids={0, 2}, pads=pads, vias=vias, max_err=5000)
    assert "outline" in d["warning"] and d["fills"][0]["outer"] == [list(p) for p in SQ]
    assert d["pads"] == [] and d["vias"] == []  # M4-R8: an unfilled zone reaches nothing (not reached)


# ---------------------------------------------------------------- layer roles (M4-R7)
LAYERS_4 = ('(kicad_pcb (layers (0 "F.Cu" signal) (4 "In1.Cu" power) (6 "In2.Cu" power) (2 "B.Cu" signal) '
            '(5 "F.SilkS" user)))')


def test_layer_plan_4layer_s_gnd_pwr_s():
    plan, findings = M.layer_plan(LAYERS_4, {})
    assert findings == []
    assert [(g["name"], g["kind"], g["role"], g["role_source"], g["routable"]) for g in plan] == [
        ("F.Cu", "signal", "signal", "kicad", True), ("In1.Cu", "power", "power", "kicad", False),
        ("In2.Cu", "power", "power", "kicad", False), ("B.Cu", "signal", "signal", "kicad", True)]
    assert [g["id"] for g in plan] == [0, 4, 6, 2]


def test_layer_plan_default_stackup_and_config_override():
    all_signal = LAYERS_4.replace("power", "signal")
    plan, _ = M.layer_plan(all_signal, {})
    assert [(g["role"], g["role_source"], g["routable"]) for g in plan] == [
        ("signal", "default", True), ("gnd", "default", False), ("pwr", "default", False), ("signal", "default", True)]
    plan, _ = M.layer_plan(all_signal, {"In1.Cu": "mixed"})
    assert [(g["name"], g["role"], g["role_source"], g["routable"]) for g in plan if g["name"] == "In1.Cu"] == [
        ("In1.Cu", "mixed", "config", True)]


def test_layer_plan_unknown_count_needs_roles():
    t = '(kicad_pcb (layers (0 "F.Cu" signal) (4 "In1.Cu" signal) (2 "B.Cu" signal)))'
    plan, findings = M.layer_plan(t, {})
    assert [f.code for f in findings] == ["LAYER_ROLES_NEEDED"]
    assert [(g["role"], g["routable"]) for g in plan] == [(None, False)] * 3
    plan, findings = M.layer_plan(t, {"F.Cu": "signal", "In1.Cu": "gnd", "B.Cu": "signal"})
    assert findings == [] and [g["routable"] for g in plan] == [True, False, True]


# ---------------------------------------------------------------- export_model_ipc against a stub Board
class StubBoard:
    """The slice of kipy.board.Board that export_model_ipc calls."""

    def __init__(self, enabled, nets, footprints, tracks, vias, shapes, zones, texts=()):
        self.enabled, self.nets, self.fps, self.trk, self.vs, self.sh, self.zs, self.tx = (
            enabled, nets, footprints, tracks, vias, shapes, zones, texts)

    def get_enabled_layers(self): return self.enabled
    def get_nets(self): return [kipy.board_types.Net(name=n) for n in self.nets]
    def get_footprints(self): return self.fps
    def get_tracks(self): return self.trk
    def get_vias(self): return self.vs
    def get_shapes(self): return self.sh
    def get_zones(self): return self.zs
    def get_text(self): return self.tx
    def get_item_bounding_box(self, item): return None
    def get_pad_shapes_as_polygons(self, pad, layer): return None


class StubProject:
    path = "/nonexistent"


def mk_fp(ref, pads, ties=()):
    f = bt.FootprintInstance()
    f.layer = FCU
    for p in pads:
        a = Any()
        a.Pack(p.proto)
        f.definition.items.append(a)
    for g in ties:
        f.definition.net_ties.add().pad_number.extend(g)
    fi = FootprintInstance(f)
    fi.reference_field.text.value = ref
    return fi


def stub_board():
    p1 = mk_pad("1", (5 * MM, 5 * MM), PSS.PSS_RECTANGLE, (MM, MM), net="GND", uid="p1")
    p2 = mk_pad("2", (15 * MM, 5 * MM), PSS.PSS_ROUNDRECT, (MM, MM), net="VCC", uid="p2", ratio=0.25)
    thru = mk_pad("1", (30 * MM, 5 * MM), PSS.PSS_CIRCLE, (1_600_000, 1_600_000), net="GND", uid="p3",
                  layers=(FCU, BCU), ptype=PT.PT_PTH, drill=(800_000, 800_000))
    tie_a = mk_pad("1", (40 * MM, 5 * MM), net="GND", uid="t1")
    tie_b = mk_pad("2", (41 * MM, 5 * MM), net="VCC", uid="t2")
    fps = [mk_fp("R1", [p1, p2]), mk_fp("J1", [thru]), mk_fp("NT1", [tie_a, tie_b], ties=[("1", "2")])]
    t = bt.Track()
    t.id.value = "tr1"
    v2(t.start, 5 * MM, 5 * MM)  # inside p1 (centre)
    v2(t.end, 12 * MM, 5 * MM)  # free end
    t.width.value_nm = 250_000
    t.layer = FCU
    t.net.name = "GND"
    via = Via()
    via.position = kipy.geometry.Vector2.from_xy(30 * MM, 8 * MM)
    via.diameter = 800_000
    via.drill_diameter = 400_000
    via.net = kipy.board_types.Net(name="GND")
    edge = [seg_shape((0, 0), (50 * MM, 0), layer=BoardLayer.BL_Edge_Cuts, width=100_000),
            seg_shape((50 * MM, 0), (50 * MM, 30 * MM), layer=BoardLayer.BL_Edge_Cuts, width=100_000)]
    cu_line = seg_shape((0, 20 * MM), (10 * MM, 20 * MM), layer=FCU, width=200_000)
    ant = mk_zone(bt.ZoneType.ZT_RULE_AREA, [FCU, BCU], tracks=True, vias=True, name="ant", uid="ra")
    pour = mk_zone(bt.ZoneType.ZT_COPPER, [FCU], net="GND", uid="gnd-pour", filled={FCU: [mk_pwh(SQ)]})
    return StubBoard([FCU, BCU, BoardLayer.BL_F_SilkS], ["VCC", "GND", ""], fps, [Track(t)], [via],
                     edge + [cu_line], [ant, pour])


def test_export_model_ipc_end_to_end_and_engine_loads_it(engine):
    board = stub_board()
    rules = R.resolve_rules_from_project({}, None, netclass_of={"": "Default", "GND": "Default", "VCC": "Default"},
                                         classes={}, widths_before={})
    dru = '(rule r (condition "A.insideArea(\'ant\')") (constraint disallow via))'
    m = M.export_model_ipc(board, StubProject(), LAYERS_4.replace("power", "signal").replace(
        '(6 "In2.Cu" signal) ', "").replace('(4 "In1.Cu" signal) ', ""), rules, name="stub.kicad_pcb",
        layer_roles={}, dru_text=dru)
    assert list(m) == ["units", "board", "layers", "nets", "pads", "vias", "kept", "copper_shapes", "holes",
                       "outline", "keepouts", "net_ties", "pours", "teardrops", "dangling_ends", "rules"]
    assert m["teardrops"] == []
    assert [(g["id"], g["name"], g["kind"], g["routable"]) for g in m["layers"]] == [
        (0, "F.Cu", "signal", True), (2, "B.Cu", "signal", True)]
    assert m["nets"] == [{"code": 1, "name": "GND"}, {"code": 2, "name": "VCC"}]
    assert sorted(p["ref"] + "." + p["number"] for p in m["pads"]) == [
        "J1.1", "NT1.1", "NT1.2", "R1.1", "R1.2"]
    assert m["holes"] == [{"at": [30 * MM, 5 * MM], "d": 800_000, "kind": "pth"}]
    nt = next(p for p in m["pads"] if p["ref"] == "NT1" and p["number"] == "2")
    assert nt["net_tie_group"] == 0 and m["net_ties"] == [{"ref": "NT1", "groups": [["1", "2"]], "nets": [[1, 2]]}]
    assert len(m["kept"]) == 1 and m["kept"][0]["net"] == 1 and len(m["vias"]) == 1 and m["vias"][0]["free"]
    assert len(m["outline"]) == 2 and any(c["layer"] == 0 for c in m["copper_shapes"])
    assert [(k["name"], k["no_tracks"], k["no_vias"]) for k in m["keepouts"]] == [
        ("ant", True, True), ("dru:r", False, True)]
    assert [p["uuid"] for p in m["pours"]] == ["gnd-pour"] and m["pours"][0]["pads"] == ["R1.1"]
    # R1.1 (GND) touches the pour; its track starts in the pad and ends free
    ends = m["dangling_ends"]
    assert [(e["item"], e["end"]) for e in ends] == [("tr1", [12 * MM, 5 * MM])]
    json.dumps(m)
    # the engine's model loader accepts the extra layer keys (kind / role / role_source / routable)
    assert set(m["layers"][0]) == {"id", "name", "kind", "role", "role_source", "routable"}
    rep = json.loads(engine.check_items(json.dumps(m), "[]", "{}"))
    assert rep["checked"] == 0


def test_export_model_ipc_uses_the_given_net_codes():
    """MINOR 13: the caller passes ONE net-code table to the rule resolver and the exporter."""
    board = stub_board()
    codes = {"GND": 5, "VCC": 7}
    rules = R.resolve_rules_from_project({}, None, netclass_of={}, classes={}, widths_before={}, net_codes=codes)
    m = M.export_model_ipc(board, StubProject(), LAYERS_4.replace("power", "signal").replace(
        '(6 "In2.Cu" signal) ', "").replace('(4 "In1.Cu" signal) ', ""), rules, name="stub.kicad_pcb",
        layer_roles={}, dru_text="", net_codes=codes)
    assert m["nets"] == [{"code": 5, "name": "GND"}, {"code": 7, "name": "VCC"}]
    assert sorted(m["rules"]["per_net"]) == ["5", "7"]
    assert {p["net"] for p in m["pads"]} <= {0, 5, 7} and m["kept"][0]["net"] == 5
    assert m["pours"][0]["net"] == 5 and m["pours"][0]["pads"] == ["R1.1"]


def test_export_model_ipc_via_free_is_the_kicad_flag():
    """Ruling C23: ``free`` comes from the save_as copy text; a via the copy does not list is free (unsure ->
    fixed obstacle, never ripped). The old "no track end touches it" guess is gone: the stub's via has no track."""
    board = stub_board()
    board.vs[0].id.value = vid = "0b5b6ef8-2bbc-46d8-a400-554a387cf306"
    rules = R.resolve_rules_from_project({}, None, netclass_of={"": "Default"}, classes={}, widths_before={})
    base = '(kicad_pcb (version 20260206) (layers (0 "F.Cu" signal) (2 "B.Cu" signal))\n'
    for tail, want in (("", True),
                       (f'(via (at 30 8) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net "GND") (uuid "{vid}"))', False),
                       (f'(via (at 30 8) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (free yes) (net "GND") (uuid "{vid}"))', True),
                       ('(via (at 30 8) (size 0.8) (layers "F.Cu" "B.Cu") (net "GND") (uuid "other"))', True)):
        m = M.export_model_ipc(board, StubProject(), base + tail + "\n)\n", rules, name="x.kicad_pcb", dru_text="")
        assert m["vias"][0]["free"] is want, tail


def test_thick_arc_is_one_obstacle_ring_containing_its_stroke():
    """K1 (live dcsgonebadge): a 2 mm wide GND arc of radius 1 mm became 44 capsule rings (44 planner obstacles)
    over IPC, one polygon in the harness; now one ring that still contains the stroke."""
    import math as _m
    from toporlib import geom
    s, mid, e = (62_625_000, 53_000_000), (62_917_893, 52_292_893), (63_625_000, 52_000_000)
    rings = geom.arc_stroke_rings(s, mid, e, 2 * MM, 500)
    assert len(rings) == 1
    c, r, a0, sw = geom.arc_geometry(s, mid, e)
    for i in range(41):  # samples of the true stroke: the arc's points with discs of radius w/2 around them
        a = a0 + sw * i / 40
        px, py = c[0] + r * _m.cos(a), c[1] + r * _m.sin(a)
        for k in range(16):
            q = (px + 0.999 * MM * _m.cos(k * _m.pi / 8), py + 0.999 * MM * _m.sin(k * _m.pi / 8))
            assert geom.point_in_ring(q, rings[0]), (i, k)


def test_filled_stroked_concave_polygon_is_one_ring():
    """K1 (live dcsgonebadge): an "E"-shaped filled gr_poly with a 0.2 mm stroke became the polygon plus 12 edge
    capsules (13 overlapping obstacle rings; the engine's triangulation panicked on that model); now one ring,
    the polygon grown by half the stroke, which contains every edge capsule."""
    from toporlib import geom
    e = [(67302355, 58798981), (71149731, 58798981), (71149731, 59431728), (68314410, 59431728),
         (68314410, 61643783), (70485944, 61643783), (70485944, 62229118), (68314410, 62229118),
         (68314410, 64474364), (71149731, 64474364), (71149731, 65107111), (67302355, 65107111)]
    g = geom.grow_ring_outward(e, 100_000, 500)
    assert g is not None and geom.ring_is_simple(g)
    for cap in geom.polyline_stroke_rings(e, 200_000, 500, closed=True):
        for p in cap:
            q = (p[0], p[1])
            assert geom.point_in_ring(q, g) or min(geom.dist_pt_seg(q, g[i], g[(i + 1) % len(g)])
                                                   for i in range(len(g))) <= 600, p
    assert geom.grow_ring_outward([(0, 0), (10, 0), (10, 10), (0, 10)], 0, 500) is None


def test_custom_pad_polygon_arcs_are_flattened_outward():
    """K1 (live dcsgonebadge, 7 KiCad clearance violations at JP1.3): the arc side of a custom pad's polygon was
    flattened with vertices on the arc, up to max_error inside KiCad's copper. Now every point of the true arc lies
    inside the obstacle ring."""
    import math as _m
    from kipy.geometry import PolygonWithHoles
    from toporlib import geom
    p = bp.PolygonWithHoles()
    for x, y in ((0, 0), (MM, 0)):
        v2(p.outline.nodes.add().point, x, y)
    a = p.outline.nodes.add().arc  # D shape: half circle of radius 0.5 mm from (1, 0) via (1.5, 0.5) to (1, 1) mm
    v2(a.start, MM, 0), v2(a.mid, int(1.5 * MM), MM // 2), v2(a.end, MM, MM)
    v2(p.outline.nodes.add().point, 0, MM)
    p.outline.closed = True
    ring = M.pad_polygon_rings(PolygonWithHoles(p), 5000)[0]
    for k in range(1, 40):
        t = -_m.pi / 2 + _m.pi * k / 40
        q = (MM + 0.5 * MM * _m.cos(t) - 1, MM / 2 + 0.5 * MM * _m.sin(t))
        assert geom.point_in_ring(q, ring), k


def test_custom_pad_polygon_of_flattened_circles_is_grown_by_max_error():
    """K1: KiCad's pad polygon arrives as points ON the circles (chords inside the copper); the obstacle is grown by
    the board's max error so it contains the true circle (dcsgonebadge JP1.3: two filled r = 0.5 mm circles)."""
    import math as _m
    from kipy.geometry import PolygonWithHoles
    from toporlib import geom
    r, err = MM // 2, 5000
    n = int(_m.ceil(_m.pi / _m.acos(1 - err / r)))  # chord error = max_err, as KiCad flattens
    p = bp.PolygonWithHoles()
    for k in range(n):
        v2(p.outline.nodes.add().point, int(r * _m.cos(2 * _m.pi * k / n)), int(r * _m.sin(2 * _m.pi * k / n)))
    p.outline.closed = True
    ring = M.pad_polygon_rings(PolygonWithHoles(p), err)[0]
    for k in range(720):
        q = (r * _m.cos(_m.pi * k / 360), r * _m.sin(_m.pi * k / 360))
        assert geom.point_in_ring(q, ring), k
    assert max(_m.hypot(*q) for q in ring) <= r + err + 600
