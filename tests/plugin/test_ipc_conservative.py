"""M4-R8 / rulings C23-C26: the IPC exporter's approximations err in the CONSERVATIVE direction. Offline:
pcbnew is the oracle on the test side only (plugin/ and toporlib/ never import it, test_no_pcbnew).

  * copper graphics (C25): IPC obstacle ⊇ pcbnew TransformShapeToPolygon (within 1 um) and IPC minus
    (pcbnew inflated by max_error + 1 um) is empty (bounded excess); every copper graphic of every snapshot
    board plus synthetic shapes;
  * copper text (C25): pcbnew GetBoundingBox() (what the IPC server returns, UNVERIFIED) ⊇ the text copper;
  * fills: zone_to_pour fills are polygon-equivalent to pcbnew's filled polygons (symmetric difference ≈ 0);
  * pour reach (C26): model_ipc.pour_reach fed with pcbnew-derived pads / vias / fills ⊆ pcbnew connectivity;
  * via free (C23): toporlib.pcbfile.via_free_flags == pcbnew GetIsFree() per via on every snapshot board
    re-saved through pcbnew (KiCad 10 text).

All snapshot boards are loaded from copies in tmp_path (copy_project); nothing is ever written to a snapshot.
Polygon areas come from pcbnew SHAPE_POLY_SET boolean operations.
"""
from __future__ import annotations

import math

import pytest

pcbnew = pytest.importorskip("pcbnew")
kipy = pytest.importorskip("kipy")

from kipy.board_types import BoardShape, to_concrete_board_shape  # noqa: E402
from kipy.proto.board import board_types_pb2 as bt  # noqa: E402
from kipy.proto.common.types import base_types_pb2 as bp  # noqa: E402

from harness.paths import BOARD_SNAPSHOTS, board_pcb  # noqa: E402
from topor import model_ipc as M  # noqa: E402
from topor.ipc import pcbnew_to_kipy_layer  # noqa: E402
from toporlib import pcbfile  # noqa: E402

UM = 1000
MM = 1_000_000
EPS_AREA = 1.0 * UM * UM  # nm^2: boolean-op slivers; 1 um^2 is far below any real shape
FINE_ERR = 20  # nm: pcbnew reference polygons this close to the true curve
INFLATE_ERR = 10  # nm: arc error of the reference inflation
BOARDS = sorted(BOARD_SNAPSHOTS)


# ---------------------------------------------------------------- polygon math (pcbnew, test side only)
def sps(rings) -> "pcbnew.SHAPE_POLY_SET":
    """Union of the rings (each a list of [x, y] nm)."""
    out = pcbnew.SHAPE_POLY_SET()
    for r in rings:
        if len(r) < 3:
            continue
        one = pcbnew.SHAPE_POLY_SET()
        one.NewOutline()
        for x, y in r:
            one.Append(int(x), int(y), -1, -1, True)
        one.Simplify()
        out.BooleanAdd(one)
    return out


def inflated(p, d: int):
    q = p.CloneDropTriangulation()
    if d:
        q.Inflate(int(d), pcbnew.CORNER_STRATEGY_ROUND_ALL_CORNERS, INFLATE_ERR)
    return q


def minus_area(a, b) -> float:
    """Area of a minus b."""
    q = a.CloneDropTriangulation()
    q.BooleanSubtract(b)
    return q.Area()


def item_poly(item, err: int):
    """pcbnew's own polygon of `item` (ERROR_INSIDE, the reference the IPC obstacle must contain); see reference_item."""
    item = reference_item(item)
    ps = pcbnew.SHAPE_POLY_SET()
    item.TransformShapeToPolygon(ps, item.GetLayer(), 0, int(err), pcbnew.ERROR_INSIDE)
    ps.Simplify()
    return ps


def arc_mid_skew(item) -> float:
    """Angle (rad) between an arc's stored mid point and its true angular midpoint (0 for non-arcs)."""
    if item.GetClass() != "PCB_SHAPE" or item.GetShape() != pcbnew.SHAPE_T_ARC:
        return 0.0
    c, s, m = item.GetCenter(), item.GetStart(), item.GetArcMid()
    a_s = math.atan2(s.y - c.y, s.x - c.x)
    a_m = math.atan2(m.y - c.y, m.x - c.x)
    a_true = a_s + math.radians(item.GetArcAngle().AsDegrees()) / 2
    return abs((a_m - a_true + math.pi) % (2 * math.pi) - math.pi)


def reference_item(item):
    """`item`, or for an arc whose stored mid point is not its angular midpoint (bornhack2026 AE1 has two,
    near 180 degrees) a clone with the true midpoint: pcbnew's TransformShapeToPolygon of such an arc is NOT the
    arc (its polygon is up to ~6 um off-centre), while KiCad's DRC shape (GetEffectiveShape) IS the arc through
    start / centre / end; :func:`assert_drc_shape_is_the_arc` proves that on every such arc. The IPC exporter
    gets start / mid / end and builds the exact arc."""
    if arc_mid_skew(item) < 1e-4:
        return item
    c = pcbnew.PCB_SHAPE(None, pcbnew.SHAPE_T_ARC)
    c.SetLayer(item.GetLayer())
    c.SetWidth(item.GetWidth())
    cc, st = item.GetCenter(), item.GetStart()
    a = math.atan2(st.y - cc.y, st.x - cc.x) + math.radians(item.GetArcAngle().AsDegrees()) / 2
    r = item.GetRadius()
    mid = pcbnew.VECTOR2I(int(round(cc.x + r * math.cos(a))), int(round(cc.y + r * math.sin(a))))
    c.SetArcGeometry(item.GetStart(), mid, item.GetEnd())
    assert abs(c.GetArcAngle().AsDegrees() - item.GetArcAngle().AsDegrees()) < 1e-3
    return c


def assert_drc_shape_is_the_arc(item, d: int = 300):
    """KiCad's DRC shape of an arc: copper at radius r +- (w/2 - d), none at r +- (w/2 + d), across the sweep."""
    eff = item.GetEffectiveShape()
    c, r, hw = item.GetCenter(), item.GetRadius(), item.GetWidth() / 2
    st = item.GetStart()
    a0 = math.atan2(st.y - c.y, st.x - c.x)
    sw = math.radians(item.GetArcAngle().AsDegrees())
    for k in range(1, 40):
        a = a0 + sw * k / 40
        for rr, inside in ((r + hw - d, True), (r + hw + d, False), (r - hw + d, True), (r - hw - d, False)):
            p = pcbnew.VECTOR2I(int(round(c.x + rr * math.cos(a))), int(round(c.y + rr * math.sin(a))))
            assert eff.Collide(p, 0) == inside, (item.m_Uuid.AsString(), math.degrees(a), rr - r)


# ---------------------------------------------------------------- pcbnew -> kipy proto (what the IPC server sends)
def _v(m, p):
    m.x_nm, m.y_nm = int(p.x), int(p.y)


def kipy_shape(item):
    """kipy Board* shape built from a pcbnew PCB_SHAPE (absolute coordinates, as the server serialises them)."""
    g = bt.BoardGraphicShape()
    g.id.value = item.m_Uuid.AsString()
    g.layer = pcbnew_to_kipy_layer(int(item.GetLayer()))
    g.net.name = item.GetNetname() if hasattr(item, "GetNetname") else ""
    g.shape.attributes.stroke.width.value_nm = int(item.GetWidth())
    g.shape.attributes.fill.fill_type = bp.GraphicFillType.GFT_FILLED if item.IsSolidFill() else bp.GraphicFillType.GFT_UNFILLED
    s = item.GetShape()
    if s == pcbnew.SHAPE_T_SEGMENT:
        _v(g.shape.segment.start, item.GetStart())
        _v(g.shape.segment.end, item.GetEnd())
    elif s == pcbnew.SHAPE_T_ARC:
        _v(g.shape.arc.start, item.GetStart())
        _v(g.shape.arc.mid, item.GetArcMid())
        _v(g.shape.arc.end, item.GetEnd())
    elif s == pcbnew.SHAPE_T_CIRCLE:
        _v(g.shape.circle.center, item.GetCenter())
        _v(g.shape.circle.radius_point, item.GetEnd())
    elif s == pcbnew.SHAPE_T_RECTANGLE:
        _v(g.shape.rectangle.top_left, item.GetStart())
        _v(g.shape.rectangle.bottom_right, item.GetEnd())
        if item.GetCornerRadius():
            g.shape.rectangle.corner_radius.value_nm = int(item.GetCornerRadius())
    elif s == pcbnew.SHAPE_T_POLY:
        ps = item.GetPolyShape()
        for i in range(ps.OutlineCount()):
            pwh = bp.PolygonWithHoles()
            o = ps.COutline(i)
            assert o.ArcCount() == 0, "arc polygons: extend the converter"
            for k in range(o.PointCount()):
                _v(pwh.outline.nodes.add().point, o.CPoint(k))
            pwh.outline.closed = True
            g.shape.polygon.polygons.append(pwh)
    elif s == pcbnew.SHAPE_T_BEZIER:
        _v(g.shape.bezier.start, item.GetStart())
        _v(g.shape.bezier.control1, item.GetBezierC1())
        _v(g.shape.bezier.control2, item.GetBezierC2())
        _v(g.shape.bezier.end, item.GetEnd())
    else:
        raise AssertionError(f"unknown shape {item.ShowShape()}")
    return to_concrete_board_shape(BoardShape(g))


def shape_numbers(item, max_err: int) -> dict:
    """missing (pcbnew minus IPC+1um, board max_error and fine reference) and excess (IPC minus
    pcbnew+(max_error+1um)) areas in nm^2."""
    ipc = sps(M.shape_to_polys(kipy_shape(item), max_err))
    ref = item_poly(item, max_err)
    fine = item_poly(item, FINE_ERR)
    grown = inflated(ipc, UM)
    return {"missing": minus_area(ref, grown), "missing_fine": minus_area(fine, grown),
            "excess": minus_area(ipc, inflated(ref, max_err + UM)), "area": ref.Area(),
            "kind": item.ShowShape(), "w": int(item.GetWidth()), "filled": bool(item.IsSolidFill())}


def assert_conservative(nums: list[dict], label: str):
    bad = [n for n in nums if n["missing"] > EPS_AREA or n["missing_fine"] > EPS_AREA or n["excess"] > EPS_AREA]
    worst = {k: max((n[k] for n in nums), default=0.0) for k in ("missing", "missing_fine", "excess")}
    print(f"{label}: {len(nums)} shapes, worst missing {worst['missing']:.0f} nm^2, missing_fine "
          f"{worst['missing_fine']:.0f} nm^2, excess {worst['excess']:.0f} nm^2")
    assert not bad, f"{label}: {len(bad)} shapes not conservative / bounded, e.g. {bad[:3]}"


# ---------------------------------------------------------------- snapshot boards (copies)
def load_copy(board_name: str, tmp_path_factory):
    """(project copy, pcbnew BOARD) of a fresh tmp copy of the snapshot. Never cached across tests: other test
    modules load other projects into pcbnew's settings manager, after which a kept BOARD's zone filler crashes."""
    from harness.copy import copy_project
    from harness.model import load_board

    d = tmp_path_factory.mktemp(f"cons-{board_name}")
    pc = copy_project(board_pcb(board_name), d / board_name)
    return pc, load_board(pc)


def copper_items(b, classes):
    cu = {int(l) for l in b.GetEnabledLayers().CuStack()}
    items = list(b.GetDrawings())
    for fp in b.GetFootprints():
        items += list(fp.GraphicalItems())
    return [it for it in items if it.GetClass() in classes and int(it.GetLayer()) in cu]


@pytest.mark.parametrize("board_name", BOARDS)
def test_snapshot_copper_graphics_are_contained(board_name, tmp_path_factory):
    _, b = load_copy(board_name, tmp_path_factory)
    max_err = int(b.GetDesignSettings().m_MaxError) or 5000
    items = copper_items(b, ("PCB_SHAPE",))
    skewed = [it for it in items if arc_mid_skew(it) >= 1e-4]
    for it in skewed:
        assert_drc_shape_is_the_arc(it)
    print(f"{board_name}: {len(skewed)} arcs with a skewed mid point (reference = the DRC arc)")
    nums = [shape_numbers(it, max_err) for it in items]
    assert_conservative(nums, f"{board_name} (max_error {max_err})")


# ---------------------------------------------------------------- synthetic shapes
def _board():
    b = pcbnew.BOARD()
    return b


def _shape(b, kind, width, filled=False, layer=None):
    s = pcbnew.PCB_SHAPE(b, kind)
    s.SetLayer(pcbnew.F_Cu if layer is None else layer)
    s.SetWidth(int(width))
    if filled:
        s.SetFilled(True)
    return s


def _xy(x, y):
    return pcbnew.VECTOR2I(int(x), int(y))


def synthetic_shapes():
    """Segments, arcs (small / large / thick-on-tight), circles and rectangles (filled / unfilled, KiCad 10 corner
    radius, rotated -> polygon), a concave polygon, a Bezier, each in three widths; open shapes again rotated by
    37 degrees; plus a filled polygon without stroke."""
    b = _board()
    makers = []
    for w in (100_000, 250_000, 1_000_000):
        def seg(w=w):
            s = _shape(b, pcbnew.SHAPE_T_SEGMENT, w)
            s.SetStart(_xy(1 * MM, 2 * MM)), s.SetEnd(_xy(7.3 * MM, 4.1 * MM))
            return s
        makers.append((seg, True))
        # (centre, radius, start angle, sweep): small / tight (radius ~ width) / large > 180 degrees / clockwise.
        # The mid point is the true arc midpoint, as KiCad writes it (pcbnew's own polygon of an arc whose mid
        # point is not the midpoint is not the arc: a pcbnew quirk outside the IPC path).
        for (cx, cy, r, a0, sw) in ((3 * MM, -2 * MM, 3.6 * MM, 30, 110), (0, 0, 0.55 * MM, -20, 150),
                                    (8 * MM, 8 * MM, 11.8 * MM, -130, 260), (1 * MM, 1 * MM, 2 * MM, 200, -75)):
            def arc(w=w, cx=cx, cy=cy, r=r, a0=a0, sw=sw):
                pt = [(cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
                      for a in (a0, a0 + sw / 2, a0 + sw)]
                s = _shape(b, pcbnew.SHAPE_T_ARC, w)
                s.SetArcGeometry(*(_xy(*q) for q in pt))
                return s
            makers.append((arc, True))
        for filled in (False, True):
            for r in (0.3 * MM, 2.5 * MM):
                def circ(w=w, filled=filled, r=r):
                    s = _shape(b, pcbnew.SHAPE_T_CIRCLE, w, filled)
                    s.SetCenter(_xy(10 * MM, 10 * MM)), s.SetEnd(_xy(10 * MM + r, 10 * MM))
                    return s
                makers.append((circ, False))
            for rc in (0, 400_000):
                def rect(w=w, filled=filled, rc=rc):
                    s = _shape(b, pcbnew.SHAPE_T_RECTANGLE, w, filled)
                    s.SetStart(_xy(1 * MM, 1 * MM)), s.SetEnd(_xy(5 * MM, 3 * MM))
                    if rc:
                        s.SetCornerRadius(rc)
                    return s
                makers.append((rect, False))

            def rect_rot(w=w, filled=filled):  # rotated -> KiCad makes it a polygon
                s = _shape(b, pcbnew.SHAPE_T_RECTANGLE, w, filled)
                s.SetStart(_xy(1 * MM, 1 * MM)), s.SetEnd(_xy(5 * MM, 3 * MM))
                s.Rotate(_xy(0, 0), pcbnew.EDA_ANGLE(30, pcbnew.DEGREES_T))
                return s
            makers.append((rect_rot, False))

            def poly(w=w, filled=filled):  # concave
                s = _shape(b, pcbnew.SHAPE_T_POLY, w, filled)
                s.SetPolyPoints([_xy(0, 0), _xy(4 * MM, 0), _xy(4 * MM, 1 * MM), _xy(1.5 * MM, 1.2 * MM),
                                 _xy(2 * MM, 3 * MM), _xy(0, 2.5 * MM)])
                return s
            makers.append((poly, True))

        def bez(w=w):
            s = _shape(b, pcbnew.SHAPE_T_BEZIER, w)
            s.SetStart(_xy(0, 0)), s.SetBezierC1(_xy(0, 5 * MM)), s.SetBezierC2(_xy(5 * MM, -2 * MM))
            s.SetEnd(_xy(6 * MM, 4 * MM))
            s.RebuildBezierToSegmentsPointsList(s.GetMaxError())
            return s
        makers.append((bez, True))

    def bare_poly():
        s = _shape(b, pcbnew.SHAPE_T_POLY, 0, True)
        s.SetPolyPoints([_xy(0, 0), _xy(3 * MM, 0), _xy(1 * MM, 1 * MM), _xy(0, 3 * MM)])
        return s
    makers.append((bare_poly, False))
    out = []
    for make, rotate in makers:
        out.append(make())
        if rotate:
            c = make()
            c.Rotate(_xy(0, 0), pcbnew.EDA_ANGLE(37, pcbnew.DEGREES_T))
            if c.GetShape() == pcbnew.SHAPE_T_BEZIER:
                c.RebuildBezierToSegmentsPointsList(c.GetMaxError())
            out.append(c)
    return b, out


@pytest.mark.parametrize("max_err", [1500, 5000])
def test_synthetic_copper_graphics_are_contained(max_err):
    b, shapes = synthetic_shapes()
    b.GetDesignSettings().m_MaxError = max_err
    nums = [shape_numbers(s, max_err) for s in shapes]
    kinds = {(n["kind"], n["filled"]) for n in nums}
    assert {("Line", False), ("Arc", False), ("Circle", False), ("Circle", True), ("Rect", False), ("Rect", True),
            ("Polygon", False), ("Polygon", True), ("Bezier Curve", False)} <= kinds, kinds
    assert_conservative(nums, f"synthetic (max_error {max_err})")


# ---------------------------------------------------------------- copper text
class _P:
    def __init__(self, x, y):
        self.x, self.y = int(x), int(y)


class _Box:
    """kipy ``Box2`` look-alike (pos, size) of a pcbnew BOX2I (values copied: SWIG returns references)."""

    def __init__(self, bb):
        self.pos = _P(bb.GetX(), bb.GetY())
        self.size = _P(bb.GetWidth(), bb.GetHeight())


def text_copper(t, err: int):
    """The copper of a pcbnew text as KiCad's DRC sees it: the glyphs, or the knockout box minus the glyphs."""
    ps = pcbnew.SHAPE_POLY_SET()
    if t.IsKnockout():
        t.TransformShapeToPolygon(ps, t.GetLayer(), 0, int(err), pcbnew.ERROR_INSIDE)
    else:
        t.TransformTextToPolySet(ps, 0, int(err), pcbnew.ERROR_INSIDE)
    ps.Simplify()
    return ps


def kipy_text(t):
    """kipy BoardText with the attributes the IPC exporter reads (size)."""
    from kipy.board_types import BoardText

    p = bt.BoardText()
    p.id.value = t.m_Uuid.AsString()
    p.layer = pcbnew_to_kipy_layer(int(t.GetLayer()))
    sz = t.GetTextSize()
    p.text.attributes.size.x_nm, p.text.attributes.size.y_nm = int(sz.x), int(sz.y)
    p.text.attributes.stroke_width.value_nm = int(t.GetTextThickness())
    return BoardText(p)


def text_numbers(t, max_err: int) -> dict:
    """missing area of the text copper (fine and board max_error references) outside the IPC text obstacle
    (KiCad's GetBoundingBox grown by model_ipc.text_margin)."""
    ipc = sps([M.text_box_poly(_Box(t.GetBoundingBox()), M.text_margin(kipy_text(t)))])
    grown = inflated(ipc, UM)
    return {"text": t.GetText(), "missing": minus_area(text_copper(t, max_err), grown),
            "missing_fine": minus_area(text_copper(t, FINE_ERR), grown), "knockout": t.IsKnockout()}


def synthetic_texts():
    """Stroke and outline fonts, bold / italic, rotated, mirrored on B.Cu, knockout, thin / thick strokes,
    square / wide / tall sizes, descenders, accents, brackets, multi-line."""
    b = _board()
    out = []
    for font in ("", "Noto Sans"):
        for txt in ("Topor gjpqy 42 Wg_", "ÅÉ ÿ |[]{}()@", "Line1\ngjy\nQ_"):
            for (sx, sy, th) in ((1.5, 1.5, 0.15), (1.5, 1.5, 0.3), (0.5, 0.5, 0.02), (5, 5, 0.02), (3, 1.5, 0.8)):
                for bold, ital in ((False, False), (True, True)):
                    for ang, layer, mirror in ((0, pcbnew.F_Cu, False), (30, pcbnew.F_Cu, False),
                                               (90, pcbnew.B_Cu, True), (217, pcbnew.B_Cu, True)):
                        for knock in (False, True):
                            t = pcbnew.PCB_TEXT(b)
                            t.SetText(txt)
                            t.SetLayer(layer)
                            t.SetPosition(_xy(20 * MM, 20 * MM))
                            t.SetTextSize(pcbnew.VECTOR2I(int(sx * MM), int(sy * MM)))
                            t.SetTextThickness(int(th * MM))
                            t.SetBold(bold)
                            t.SetItalic(ital)
                            if font:
                                t.SetFontProp(font)
                            t.SetTextAngle(pcbnew.EDA_ANGLE(ang, pcbnew.DEGREES_T))
                            t.SetMirrored(mirror)
                            t.SetIsKnockout(knock)
                            out.append(t)
    return b, out


def test_kicad_text_bbox_alone_is_not_the_copper():
    """Why model_ipc grows the box: KiCad's GetBoundingBox() misses descenders / accents (documents the reason;
    if KiCad ever fixes it this fails and TEXT_BBOX_GROW can shrink)."""
    _, texts = synthetic_texts()
    t = texts[0]
    box = sps([M.text_box_poly(_Box(t.GetBoundingBox()), 0)])
    assert minus_area(text_copper(t, FINE_ERR), box) > 1e9


def test_synthetic_copper_text_bbox_contains_glyphs():
    _, texts = synthetic_texts()
    nums = [text_numbers(t, 5000) for t in texts]
    assert len(nums) >= 400
    bad = [n for n in nums if n["missing"] > EPS_AREA or n["missing_fine"] > EPS_AREA]
    print(f"synthetic texts: {len(nums)}, worst missing {max(n['missing'] for n in nums):.0f} nm^2 "
          f"(fine {max(n['missing_fine'] for n in nums):.0f})")
    assert not bad, bad[:3]


@pytest.mark.parametrize("board_name", BOARDS)
def test_snapshot_copper_text_bbox_contains_glyphs(board_name, tmp_path_factory):
    _, b = load_copy(board_name, tmp_path_factory)
    max_err = int(b.GetDesignSettings().m_MaxError) or 5000
    nums = [text_numbers(t, max_err) for t in copper_items(b, ("PCB_TEXT",))]
    print(f"{board_name}: {len(nums)} copper texts")
    assert not [n for n in nums if n["missing"] > EPS_AREA or n["missing_fine"] > EPS_AREA], nums


# ---------------------------------------------------------------- fills and pour reach (harness model = pcbnew oracle)
def harness_model(board_name: str, tmp_path_factory):
    """(pcbnew board, harness model) of a fresh tmp copy; export_model refills the zones and builds the
    connectivity (in memory, on the copy)."""
    from harness.model import export_model
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    pc, b = load_copy(board_name, tmp_path_factory)
    dru = pc.dru.read_text() if pc.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    return b, export_model(b, rules, name=board_name, dru_text=dru)


def _pwh(outline_chain, hole_chains):
    p = bp.PolygonWithHoles()
    for k in range(outline_chain.PointCount()):
        _v(p.outline.nodes.add().point, outline_chain.CPoint(k))
    p.outline.closed = True
    for h in hole_chains:
        hl = p.holes.add()
        for k in range(h.PointCount()):
            _v(hl.nodes.add().point, h.CPoint(k))
        hl.closed = True
    return p


def kipy_zone(z, layers):
    """kipy Zone (copper) with KiCad's filled polygons, as the IPC server serialises them after a refill."""
    from kipy.board_types import Zone

    p = bt.Zone()
    p.id.value = z.m_Uuid.AsString()
    p.type = bt.ZoneType.ZT_COPPER
    p.layers.extend(pcbnew_to_kipy_layer(l) for l in layers)
    p.copper_settings.net.name = z.GetNetname()
    for l in layers:
        ps = z.GetFilledPolysList(l)
        f = p.filled_polygons.add()
        f.layer = pcbnew_to_kipy_layer(l)
        for i in range(ps.OutlineCount()):
            f.shapes.polygons.append(_pwh(ps.Outline(i), [ps.Hole(i, h) for h in range(ps.HoleCount(i))]))
    return Zone(p)


def sps_fills(fills, layer):
    out = pcbnew.SHAPE_POLY_SET()
    for f in fills:
        if f["layer"] != layer or len(f["outer"]) < 3:
            continue
        one = pcbnew.SHAPE_POLY_SET()
        one.NewOutline()
        for x, y in f["outer"]:
            one.Append(int(x), int(y), -1, -1, True)
        for h in f["holes"]:
            one.NewHole()
            for x, y in h:
                one.Append(int(x), int(y), 0, -1, True)
        one.Simplify()
        out.BooleanAdd(one)
    return out


@pytest.mark.parametrize("board_name", BOARDS)
def test_fills_equivalent_and_pour_reach_subset(board_name, tmp_path_factory):
    """(1) zone_to_pour fills (from Zone.filled_polygons) == pcbnew's filled polygons: symmetric difference ≈ 0.
    (2) Ruling C26: model_ipc.pour_reach fed with the pcbnew-derived pads / vias / fills of the harness model ⊆
    pcbnew connectivity, per pour; 'pcbnew reached, IPC not' is allowed (conservative) and reported. One board
    load + refill for both (the refill is the slow part)."""
    b, model = harness_model(board_name, tmp_path_factory)
    cu = [int(l) for l in b.GetEnabledLayers().CuStack()]
    zones = [z for z in b.Zones() if not z.GetIsRuleArea() and not z.IsTeardropArea() and int(z.GetNetCode()) != 0]
    worst, total, n = 0.0, 0.0, 0
    for z in zones:
        layers = sorted(int(l) for l in z.GetLayerSet().Seq() if int(l) in cu)
        d = M.zone_to_pour(kipy_zone(z, layers), net_codes={z.GetNetname(): 1}, cu_ids=set(cu), pads=[], vias=[],
                           max_err=5000)
        for l in layers:
            ref = z.GetFilledPolysList(l).CloneDropTriangulation()
            ref.Simplify()
            ipc = sps_fills(d["fills"], l)
            sym = minus_area(ref, ipc) + minus_area(ipc, ref)
            worst, total, n = max(worst, sym), total + ref.Area(), n + 1
            assert sym <= EPS_AREA, (z.m_Uuid.AsString(), l, sym, ref.Area())
    print(f"FILLS {board_name}: {n} zone layers, fill area {total / 1e12:.1f} mm^2, worst symmetric difference "
          f"{worst:.0f} nm^2")

    extra, short_p, short_v, tot_p, tot_v = [], 0, 0, 0, 0
    for p in model["pours"]:
        rp, rv = M.pour_reach(p["net"], p["layers"], p["fills"], model["pads"], model["vias"])
        extra += [(p["uuid"], x) for x in sorted(set(rp) - set(p["pads"]))]
        extra += [(p["uuid"], x) for x in sorted(set(rv) - set(p["vias"]))]
        short_p += len(set(p["pads"]) - set(rp))
        short_v += len(set(p["vias"]) - set(rv))
        tot_p += len(p["pads"])
        tot_v += len(p["vias"])
    print(f"REACH {board_name}: pours {len(model['pours'])}, pcbnew reached pads {tot_p} vias {tot_v}; "
          f"pcbnew reached but IPC not: pads {short_p}, vias {short_v}; IPC-only: {len(extra)}")
    assert not extra, f"IPC reaches what pcbnew does not: {extra[:10]}"


# ---------------------------------------------------------------- via free flag (C23)
@pytest.mark.parametrize("board_name", BOARDS)
def test_via_free_flags_equal_pcbnew(board_name, tmp_path_factory, tmp_path):
    """toporlib.pcbfile.via_free_flags on the board re-saved through pcbnew (KiCad 10 text, like a save_as copy)
    == pcbnew GetIsFree() per via uuid; the original snapshot text (legacy ``(free)`` / ``tstamp`` on wiccon) too."""
    from harness.writer import save

    pc, b = load_copy(board_name, tmp_path_factory)
    want = {t.m_Uuid.AsString(): bool(t.GetIsFree()) for t in b.GetTracks() if t.GetClass() == "PCB_VIA"}
    out = save(b, tmp_path / "resaved.kicad_pcb")
    text = out.read_text()
    assert pcbfile.file_version(text) == pcbfile.KICAD10_VERSION
    got = pcbfile.via_free_flags(text)
    assert got == want
    assert pcbfile.via_free_flags(pc.pcb.read_text()) == want
    print(f"{board_name}: {len(want)} vias, {sum(want.values())} free")
    assert M.via_free("not-a-via-uuid", got) is True  # unsure -> fixed obstacle
