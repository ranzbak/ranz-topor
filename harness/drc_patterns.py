"""PROPOSED test patterns from the DRC galleries (branch drc-patterns): one tiny board per DRC failure cluster.

NOT in the test suite: the user approves each pattern's PNG first (AGENT_RULES "New test patterns need user approval").
Every pattern is solvable by construction: the reference copper (the expected answer) is laid out first and checked with
shapely (layer-aware, the pattern's own clearance) and with KiCad's DRC (`--drc`: 0 routing violations, 0 unconnected).
Geometry and names are generic; footprints are public KiCad library shapes (pad sizes and pitches only).

Rules on every board (Board Setup + one Default net class): clearance 0.15, track 0.20 (= the board minimum, so the
router cannot neck a wire), via 0.6 / 0.3, copper to edge 0.30, hole clearance 0.25. Kept copper (a hand-routed track
or arc already on the board) is on the problem board too; the router keeps it ([run] existing_copper = "fixed").

    python -m harness.drc_patterns [OUT] [--drc]       # OUT/<name>/{problem,reference}/<name>.kicad_pcb + expected.json
"""
from __future__ import annotations

import json
import math
import sys
from dataclasses import dataclass, field
from pathlib import Path

import harness  # noqa: F401  (KiCad isolated before pcbnew)
import pcbnew

from harness.fixtures import write_project_file

STATUS = "PROPOSED (awaiting approval)"
MM = 1_000_000
SQ2 = math.sqrt(2.0)
CL = 0.15  # clearance (Board Setup and the Default class)
W = 0.20  # signal width = board minimum
VIA_D, VIA_DR = 0.6, 0.3
EDGE = 0.30
RULES = {"min_clearance": CL, "min_track_width": W, "min_copper_edge_clearance": EDGE, "min_hole_clearance": 0.25,
         "min_hole_to_hole": 0.25, "min_via_diameter": 0.4, "min_through_hole_diameter": 0.3,
         "min_via_annular_width": 0.05, "min_connection": 0.0, "max_error": 0.005}
DEFAULT_CLASS = {"name": "Default", "clearance": CL, "track_width": W, "via_diameter": VIA_D, "via_drill": VIA_DR,
                 "priority": 2147483647}
LAYER = {"F.Cu": pcbnew.F_Cu, "B.Cu": pcbnew.B_Cu}
# Router settings compared per pattern: topor's defaults and the drawing settings of the routes the clusters come from.
CONFIGS = {"default": {},
           "narrow": {"drawing.candidates": 1, "drawing.any_angle": "forbidden", "drawing.acute": "forbidden"}}


@dataclass
class Pad:
    num: str
    net: str | None
    pos: tuple[float, float]
    size: tuple[float, float]
    shape: str = "rect"  # rect | roundrect | circle | oval
    drill: float | None = None  # PTH when set
    layer: str = "F.Cu"  # SMD layer
    rr: float = 0.25  # roundrect radius ratio (KiCad: radius = ratio x min(w, h))


@dataclass
class Fp:
    ref: str
    pads: list[Pad]
    lib: str = ""  # the public library footprint the pad sizes come from
    flip: bool = False  # a footprint flipped to B.Cu (KiCad's own mirror), pads given in board coordinates


@dataclass
class Trk:
    net: str
    layer: str
    width: float
    pts: list[tuple[float, float]]
    locked: bool = False


@dataclass
class Arc:
    net: str
    layer: str
    width: float
    start: tuple[float, float]
    mid: tuple[float, float]
    end: tuple[float, float]
    locked: bool = True


@dataclass
class Via:
    net: str
    pos: tuple[float, float]
    dia: float = VIA_D
    drill: float = VIA_DR


@dataclass
class Pat:
    name: str
    cluster: str  # cluster id of the dedup table (reports/drc-patterns-*.md)
    title: str
    purpose: str
    answer: str
    why: str
    size: tuple[float, float]
    nets: list[str]
    fps: list[Fp]
    connections: list[tuple[str, str, str]]  # (net, "REF.pad", "REF.pad") the router must make
    ref_tracks: list[Trk]
    ref_vias: list[Via] = field(default_factory=list)
    kept: list[Trk] = field(default_factory=list)
    kept_arcs: list[Arc] = field(default_factory=list)
    expect: dict = field(default_factory=dict)  # vias, len (new copper), layers per net, etc.
    marks: list[dict] = field(default_factory=list)  # PNG annotations: {"xy": [x, y], "text": str}


def _r(x: float) -> float:
    return round(x, 4)


def plen(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


# ------------------------------------------------------------------------------------------------ geometry (shapely)
def pad_geom(p: Pad):
    from shapely.geometry import Point, box

    x, y = p.pos
    w, h = p.size
    if p.shape == "circle":
        return Point(x, y).buffer(w / 2, quad_segs=64)
    if p.shape == "oval":
        r = min(w, h) / 2
        return box(x - w / 2 + r, y - h / 2 + r, x + w / 2 - r, y + h / 2 - r).buffer(r, quad_segs=64) if w != h else Point(x, y).buffer(r, quad_segs=64)
    if p.shape == "roundrect":
        r = p.rr * min(w, h)
        return box(x - w / 2 + r, y - h / 2 + r, x + w / 2 - r, y + h / 2 - r).buffer(r, quad_segs=64)
    return box(x - w / 2, y - h / 2, x + w / 2, y + h / 2)


def items(pat: Pat, *, ref: bool = True):
    """(net, label, layers, geometry) of every copper item: pads, kept copper and (ref) the reference copper."""
    from shapely.geometry import LineString, Point

    out = []
    for f in pat.fps:
        for p in f.pads:
            out.append((p.net, f"pad {f.ref}.{p.num}", {"F.Cu", "B.Cu"} if p.drill else {p.layer}, pad_geom(p)))
    tracks = list(pat.kept) + (list(pat.ref_tracks) if ref else [])
    for t in tracks:
        out.append((t.net, f"track {t.net} {t.layer}", {t.layer}, LineString(t.pts).buffer(t.width / 2, quad_segs=32)))
    for a in pat.kept_arcs:
        out.append((a.net, f"arc {a.net} {a.layer}", {a.layer}, LineString(arc_points(a)).buffer(a.width / 2, quad_segs=32)))
    if ref:
        for v in pat.ref_vias:
            out.append((v.net, f"via {v.net}", {"F.Cu", "B.Cu"}, Point(v.pos).buffer(v.dia / 2, quad_segs=64)))
    return out


def arc_points(a: Arc, n: int = 64) -> list[tuple[float, float]]:
    """The arc through start, mid, end as a polyline (circumcentre)."""
    (x1, y1), (x2, y2), (x3, y3) = a.start, a.mid, a.end
    d = 2 * (x1 * (y2 - y3) + x2 * (y3 - y1) + x3 * (y1 - y2))
    ux = ((x1 * x1 + y1 * y1) * (y2 - y3) + (x2 * x2 + y2 * y2) * (y3 - y1) + (x3 * x3 + y3 * y3) * (y1 - y2)) / d
    uy = ((x1 * x1 + y1 * y1) * (x3 - x2) + (x2 * x2 + y2 * y2) * (x1 - x3) + (x3 * x3 + y3 * y3) * (x2 - x1)) / d
    r = math.dist((ux, uy), a.start)
    t1, t2, t3 = (math.atan2(p[1] - uy, p[0] - ux) for p in (a.start, a.mid, a.end))

    def ccw(a0, b0):
        return (b0 - a0) % (2 * math.pi)

    sweep = ccw(t1, t3) if ccw(t1, t2) < ccw(t1, t3) else -ccw(t3, t1)
    return [(ux + r * math.cos(t1 + sweep * i / n), uy + r * math.sin(t1 + sweep * i / n)) for i in range(n + 1)]


def clearance_report(pat: Pat) -> list[dict]:
    """Layer-aware shapely check of reference + kept copper + pads (different nets >= CL apart, copper inside the
    edge line). KiCad's DRC (--drc) is the authority; this catches construction slips before it runs."""
    from shapely.geometry import box

    its = items(pat)
    bad = []
    for i in range(len(its)):
        for j in range(i + 1, len(its)):
            (na, a, la, ga), (nb, b, lb, gb) = its[i], its[j]
            if (na is not None and na == nb) or not (la & lb):
                continue
            if a.startswith("pad") and b.startswith("pad") and a.split(".")[0] == b.split(".")[0]:
                continue  # pads of one footprint: the library's own spacing
            d = ga.distance(gb)
            if d < CL - 1e-6:
                bad.append({"a": a, "b": b, "gap": round(d, 4)})
    Wd, Hd = pat.size
    inner = box(EDGE, EDGE, Wd - EDGE, Hd - EDGE)
    for _, a, _, g in its:
        if not inner.contains(g):
            bad.append({"a": a, "b": "edge", "gap": None})
    return bad


# ------------------------------------------------------------------------------------------------ the board file
def _v(x, y):
    return pcbnew.VECTOR2I(int(round(x * MM)), int(round(y * MM)))


def build_board(pat: Pat, *, copper: bool) -> pcbnew.BOARD:
    pcbnew.KIID.SeedGenerator(7)
    b = pcbnew.CreateEmptyBoard()
    b.SetCopperLayerCount(2)
    nets = {}
    for name in pat.nets:
        n = pcbnew.NETINFO_ITEM(b, name)
        b.Add(n)
        nets[name] = n
    Wd, Hd = pat.size
    for a, c in (((0, 0), (Wd, 0)), ((Wd, 0), (Wd, Hd)), ((Wd, Hd), (0, Hd)), ((0, Hd), (0, 0))):
        s = pcbnew.PCB_SHAPE(b)
        s.SetShape(pcbnew.SHAPE_T_SEGMENT)
        s.SetLayer(pcbnew.Edge_Cuts)
        s.SetWidth(int(0.05 * MM))
        s.SetStart(_v(*a))
        s.SetEnd(_v(*c))
        b.Add(s)
    for f in pat.fps:
        fp = pcbnew.FOOTPRINT(b)
        fp.SetReference(f.ref)
        fp.SetValue(f.ref)
        ox, oy = f.pads[0].pos
        fp.SetPosition(_v(ox, oy))
        fp.Reference().SetLayer(pcbnew.F_Fab)
        fp.Value().SetLayer(pcbnew.F_Fab)
        fp.Value().SetVisible(False)
        b.Add(fp)
        for ps in f.pads:
            p = pcbnew.PAD(fp)
            p.SetNumber(ps.num)
            x, y = ps.pos
            if f.flip:
                x = 2 * ox - x  # built on F.Cu mirrored, then flipped left-right onto B.Cu (KiCad's own flip)
            if ps.drill is None:
                p.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
                ls = pcbnew.LSET()
                back = ps.layer == "B.Cu" and not f.flip
                for ly in ((pcbnew.B_Cu, pcbnew.B_Mask, pcbnew.B_Paste) if back else (pcbnew.F_Cu, pcbnew.F_Mask, pcbnew.F_Paste)):
                    ls.AddLayer(ly)
                p.SetLayerSet(ls)
            else:
                p.SetAttribute(pcbnew.PAD_ATTRIB_PTH)
                p.SetLayerSet(p.PTHMask())
                p.SetDrillSize(_v(ps.drill, ps.drill))
            shape = {"rect": pcbnew.PAD_SHAPE_RECT, "roundrect": pcbnew.PAD_SHAPE_ROUNDRECT,
                     "circle": pcbnew.PAD_SHAPE_CIRCLE, "oval": pcbnew.PAD_SHAPE_OVAL}[ps.shape]
            p.SetShape(shape)
            p.SetSize(_v(*ps.size))
            if ps.shape == "roundrect":
                p.SetRoundRectRadiusRatio(ps.rr)
            p.SetPosition(_v(x, y))
            if ps.net is not None:
                p.SetNet(nets[ps.net])
            fp.Add(p)
        if f.flip:
            fp.Flip(_v(ox, oy), pcbnew.FLIP_DIRECTION_LEFT_RIGHT)

    def track(t: Trk):
        for a, c in zip(t.pts, t.pts[1:]):
            tr = pcbnew.PCB_TRACK(b)
            tr.SetStart(_v(*a))
            tr.SetEnd(_v(*c))
            tr.SetWidth(int(round(t.width * MM)))
            tr.SetLayer(LAYER[t.layer])
            tr.SetNet(nets[t.net])
            tr.SetLocked(t.locked)
            b.Add(tr)

    for t in pat.kept:
        track(t)
    for a in pat.kept_arcs:
        arc = pcbnew.PCB_ARC(b)
        arc.SetStart(_v(*a.start))
        arc.SetMid(_v(*a.mid))
        arc.SetEnd(_v(*a.end))
        arc.SetWidth(int(round(a.width * MM)))
        arc.SetLayer(LAYER[a.layer])
        arc.SetNet(nets[a.net])
        arc.SetLocked(a.locked)
        b.Add(arc)
    if copper:
        for t in pat.ref_tracks:
            track(t)
        for v in pat.ref_vias:
            via = pcbnew.PCB_VIA(b)
            via.SetPosition(_v(*v.pos))
            via.SetWidth(int(round(v.dia * MM)))
            via.SetDrill(int(round(v.drill * MM)))
            via.SetNet(nets[v.net])
            b.Add(via)
    return b


def write_board(pat: Pat, path: Path, *, copper: bool) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(path), build_board(pat, copper=copper))
    write_project_file(path, rules=dict(RULES), classes=[dict(DEFAULT_CLASS)])
    return path


# ------------------------------------------------------------------------------------------------ helpers for patterns
def header_2x3(ref, x0, y0, nets, *, drill=1.0, size=1.7):
    """2x3 pin header (Connector_PinHeader_2.54mm:PinHeader_2x03_P2.54mm_Vertical pad sizes), pin 1 square at (x0, y0),
    pins 1/2 in the first column (y0, y0 + 2.54), then 3/4 and 5/6 at +2.54 mm in x. `nets` = six net names or None."""
    pads = []
    for i in range(6):
        col, row = divmod(i, 2)
        pads.append(Pad(str(i + 1), nets[i], (_r(x0 + 2.54 * col), _r(y0 + 2.54 * row)), (size, size),
                        "rect" if i == 0 else "circle", drill=drill))
    return Fp(ref, pads, "Connector_PinHeader_2.54mm:PinHeader_2x03_P2.54mm_Vertical")


def jst_sh4(ref, x0, y0, nets, *, mp_dy=None, layer="B.Cu"):
    """JST SH 4-pin (Connector_JST:JST_SH_SM04B-SRSS-TB_1x04-1MP_P1.00mm_Horizontal): 0.6 x 1.55 pads at 1.0 mm pitch
    from (x0, y0) in +x, mounting pads 1.2 x 1.8 at 1.3 mm beyond the end pins and `mp_dy` from the pin row (None: the
    pin row only)."""
    pads = [Pad(str(i + 1), nets[i], (_r(x0 + i), y0), (0.6, 1.55), layer=layer) for i in range(4)]
    if mp_dy is not None:
        pads += [Pad("MP", None, (_r(x0 - 1.3), _r(y0 + mp_dy)), (1.2, 1.8), layer=layer),
                 Pad("MP", None, (_r(x0 + 4.3), _r(y0 + mp_dy)), (1.2, 1.8), layer=layer)]
    return Fp(ref, pads, "Connector_JST:JST_SH_SM04B-SRSS-TB_1x04-1MP_P1.00mm_Horizontal")


# ------------------------------------------------------------------------------------------------ K1 usbc_pair_esd
def usbc_pair_esd() -> Pat:
    Wd, Hd = 9.0, 9.0
    cx, yr = 4.5, 7.0  # receptacle signal row centre, pad centre y (0.3 x 1.45 pads, 0.5 mm pitch)
    top, bot = yr - 0.725, yr + 0.725
    # Connector_USB:USB_C_Receptacle_HRO_TYPE-C-31-M-12 signal row: (dx, width, net); DP = A6/B6, DN = A7/B7 interleaved
    row = [(-3.25, 0.6, None), (-2.45, 0.6, None), (-1.75, 0.3, None), (-1.25, 0.3, None), (-0.75, 0.3, "DP"),
           (-0.25, 0.3, "DN"), (0.25, 0.3, "DP"), (0.75, 0.3, "DN"), (1.25, 0.3, None), (1.75, 0.3, None),
           (2.45, 0.6, None), (3.25, 0.6, None)]
    names = ["A1", "A4", "B5", "A8", "B6", "A7", "A6", "B7", "A5", "B8", "B4", "B1"]
    j1 = Fp("J1", [Pad(n, net, (_r(cx + dx), yr), (w, 1.45)) for n, (dx, w, net) in zip(names, row)],
            "Connector_USB:USB_C_Receptacle_HRO_TYPE-C-31-M-12 (signal row)")
    # ESD array, flow-through (Package_DFN_QFN:Diodes_UDFN-10_1x2.5mm_P0.5mm turned 90 deg): pin k faces pin 11-k
    ex, ey = cx + 1.25, 3.902
    yb, yt = _r(ey + 0.388), _r(ey - 0.388)
    esd = []
    for i, (bn, tn, net) in enumerate(((6, 5, "DP"), (7, 4, "DN"), (8, 3, None), (9, 2, None), (10, 1, None))):
        x = _r(ex - 1.0 + 0.5 * i)
        wd = 0.45 if bn == 8 else 0.25
        esd += [Pad(str(bn), net, (x, yb), (wd, 0.62)), Pad(str(tn), net, (x, yt), (wd, 0.62))]
    u2 = Fp("U2", esd, "Package_DFN_QFN:Diodes_UDFN-10_1x2.5mm_P0.5mm")
    t = Fp("J2", [Pad("1", "DP", (8.0, 1.4), (0.8, 0.3)), Pad("2", "DN", (8.0, 2.0), (0.8, 0.3))], "generic 0.6 mm pitch pads")
    yl = _r(top - CL - W / 2 - 0.005)  # DP join above the row, just clear of A7's tip
    yd = _r(bot + CL + W / 2 + 0.005)  # DN join below the row
    xp, xn = _r(cx + 0.25), _r(cx + 0.75)
    ref = [Trk("DP", "F.Cu", W, [(_r(cx - 0.75), yr), (_r(cx - 0.75), yl), (xp, yl), (xp, yr)]),
           Trk("DP", "F.Cu", W, [(xp, yl), (xp, yb), (xp, yt), (xp, 2.0), (_r(xp + 0.6), 1.4), (8.0, 1.4)]),
           Trk("DN", "F.Cu", W, [(_r(cx - 0.25), yr), (_r(cx - 0.25), yd), (xn, yd), (xn, yr)]),
           Trk("DN", "F.Cu", W, [(xn, yr), (xn, yb), (xn, yt), (xn, 2.6), (_r(xn + 0.6), 2.0), (8.0, 2.0)])]
    return Pat(
        name="usbc_pair_esd", cluster="K1", title="USB pair: duplicated receptacle pins joined, flow-through ESD, 0 vias",
        purpose="A USB-C receptacle's signal row (0.3 x 1.45 pads at 0.5 mm pitch: no wire passes between pins) carries "
                "the data pair twice, interleaved: DP on B6 / A6, DN on A7 / B7. Above it a flow-through ESD array "
                "(UDFN-10, 0.5 mm pitch; pin 6 faces pin 5, pin 7 faces pin 4: the line runs straight under the part). "
                "The pair then turns right to two 0.6 mm pitch pads. Everything is on F.Cu; the other pins have no net.",
        answer="0 vias, all F.Cu. DP joins B6 and A6 just above the row (clear of A7's tip), DN joins A7 and B7 just "
               "below it; DP leaves A6 and DN leaves B7 straight up into ESD pins 6 / 7, straight through under the part "
               "to pins 5 / 4, then up and 45 deg right into J2 (DP above DN, no crossing).",
        why="The interleaved pins force one join above and one below the row. DP above / DN below is the only order that "
            "lets both lines leave the row on the side of the ESD without enclosing the other (DN above would trap A6 "
            "inside DN's loop). The ESD is flow-through, so the shortest path is straight through it. No crossing is "
            "needed anywhere, so no via. The failure class: wires drawn between the 0.5 mm pins (forced pad exits) and a "
            "planned DP/DN crossing drawn on one layer next to the pins.",
        size=(Wd, Hd), nets=["DP", "DN"], fps=[j1, u2, t],
        connections=[("DP", "J1.B6", "J1.A6"), ("DP", "J1.A6", "U2.6"), ("DP", "U2.5", "J2.1"),
                     ("DN", "J1.A7", "J1.B7"), ("DN", "J1.B7", "U2.7"), ("DN", "U2.4", "J2.2")],
        ref_tracks=ref, expect={"vias": 0, "layers": {"DP": ["F.Cu"], "DN": ["F.Cu"]}},
        marks=[{"xy": [cx - 0.25, top - 0.05], "text": "A7 tip: DP passes at clearance"}])


# ------------------------------------------------------------------------------------------------ K2 fpc_exit_fanout
def fpc_exit_fanout() -> Pat:
    Wd, Hd = 11.0, 11.0
    px, y0 = 1.5, 2.0
    ys = [_r(y0 + 0.5 * i) for i in range(8)]
    s1, s2 = 4, 5  # pins on y 4.0 (S1) and 4.5 (S2)
    j = Fp("J1", [Pad(str(i + 1), {s1: "S1", s2: "S2"}.get(i), (px, y), (1.1, 0.3)) for i, y in enumerate(ys)],
           "Connector_FFC-FPC:TE_2-1734839-2_1x22-1MP_P0.5mm_Horizontal (pad size and pitch)")
    tp = Fp("TP1", [Pad("1", "S2", (5.1, 7.5), (1.5, 1.5))], "TestPoint:TestPoint_Pad_1.5x1.5mm")
    tgt = Fp("TP2", [Pad("1", "S1", (8.954, 9.5), (1.0, 1.0))], "TestPoint:TestPoint_Pad_1.0x1.0mm")
    tip = _r(px + 0.55)  # pad tip x 2.05
    b2 = 2.1  # S2 bends 45 deg just past the tip
    b1 = 2.11  # S1 bends at the same x: its 45 deg line is 0.51 / sqrt 2 = 0.36 from S2's (pitch 0.35)
    c1 = b1 - ys[s1]  # S1 line x - y = c1
    yh = _r(7.5 - 0.75 - CL - W / 2)  # S1's horizontal over TP1 (6.5)
    xh0 = _r(c1 + yh)
    c2 = _r((5.85 - 6.75) + (CL + W / 2) * SQ2)  # 45 deg line past TP1's top-right corner at clearance
    xh1 = _r(c2 + yh)
    ref = [Trk("S2", "F.Cu", W, [(px, ys[s2]), (b2, ys[s2]), (5.1, 7.5)]),
           Trk("S1", "F.Cu", W, [(px, ys[s1]), (b1, ys[s1]), (xh0, yh), (xh1, yh), (8.954, _r(8.954 - c2))])]
    assert abs((8.954 - c2) - 9.5) < 0.01
    return Pat(
        name="fpc_exit_fanout", cluster="K2", title="Fine-pitch connector: exit straight past the pad tip, then fan out",
        purpose="A 0.5 mm pitch FPC connector (0.3 x 1.1 pads, the others without net). S2 (lower pin) goes to a 1.5 mm "
                "test pad down-right, S1 (upper pin) to a pad further down-right beyond it, so S1 passes the test pad on "
                "its outer side. F.Cu only.",
        answer=f"0 vias. Both leave their pads straight along the pad axis to just past the tip (x {b2}), then 45 deg "
               f"down-right, parallel at 0.36 mm (pitch 0.35); S2 ends in TP1, S1 runs over TP1 at clearance (y {yh}) and "
               "45 deg past its corner into TP2.",
        why="A diagonal exit from the pad centre passes the neighbour pin's corner below clearance (0.5 mm pitch leaves "
            "0.2 mm between pads); straight to the tip and then 45 deg keeps 0.36 mm between the parallel legs. Every "
            "monotone octilinear path has the same length, so the tight fan is also the shortest. The failure class: the "
            "drawing forcing the exit past the neighbour pin and the pass-by of the test pad.",
        size=(Wd, Hd), nets=["S1", "S2"], fps=[j, tp, tgt],
        connections=[("S1", "J1.5", "TP2.1"), ("S2", "J1.6", "TP1.1")], ref_tracks=ref,
        expect={"vias": 0, "layers": {"S1": ["F.Cu"], "S2": ["F.Cu"]}},
        marks=[{"xy": [tip, ys[s1] - 0.25], "text": "pad tip"}])


# ------------------------------------------------------------------------------------------------ K3 via_pair_cross
def via_pair_cross() -> Pat:
    Wd, Hd = 10.0, 8.0
    sq = (1.0, 1.0)
    fps = [Fp("A1", [Pad("1", "A", (1.2, 4.0), sq)]), Fp("A2", [Pad("1", "A", (8.8, 4.0), sq, layer="B.Cu")]),
           Fp("B1", [Pad("1", "B", (5.0, 1.2), sq)]), Fp("B2", [Pad("1", "B", (5.0, 6.8), sq, layer="B.Cu")]),
           Fp("C1", [Pad("1", "C", (1.2, 5.6), sq)]), Fp("C2", [Pad("1", "C", (8.8, 5.6), sq)])]
    va, vb = (6.5, 4.0), (5.0, 2.6)
    ref = [Trk("A", "F.Cu", W, [(1.2, 4.0), va]), Trk("A", "B.Cu", W, [va, (8.8, 4.0)]),
           Trk("B", "F.Cu", W, [(5.0, 1.2), vb]), Trk("B", "B.Cu", W, [vb, (5.0, 6.8)]),
           Trk("C", "F.Cu", W, [(1.2, 5.6), (8.8, 5.6)])]
    need = VIA_D / 2 + CL + W / 2
    return Pat(
        name="via_pair_cross", cluster="K3", title="Two layer-changing nets cross: vias apart, not in the crossing",
        purpose="A runs from an F.Cu pad (left) to a B.Cu pad (right), B from an F.Cu pad (top) to a B.Cu pad (bottom); "
                "their straight lines cross in the middle. C runs straight on F.Cu just below the crossing. Each of A and "
                "B needs one via; there is free room everywhere else.",
        answer=f"3 straight lines, 2 vias. B dives above the crossing (via at y <= {_r(4.0 - need)}, e.g. {list(vb)}), so "
               f"B is on B.Cu where A and C cross it; A stays on F.Cu across B and dives right of it (via at x >= "
               f"{_r(5.0 + need)}, e.g. {list(va)}). Every via keeps clearance to the other nets' wires on both layers.",
        why="Straight lines are the shortest; each net changes layer once because its pads are on different layers. The "
            "two vias cannot sit at the crossing: a via is on both layers, so it must keep clearance to every wire that "
            "passes there. (B diving between A and C, y 4.55 .. 5.05, with A diving left of x 4.45, is the other legal "
            "answer of the same length.) The failure class: several vias and wires of different nets bunched into one "
            "spot below clearance, with free room beside it (a via placed or moved after its check).",
        size=(Wd, Hd), nets=["A", "B", "C"], fps=fps,
        connections=[("A", "A1.1", "A2.1"), ("B", "B1.1", "B2.1"), ("C", "C1.1", "C2.1")],
        ref_tracks=ref, ref_vias=[Via("A", va), Via("B", vb)], expect={"vias": 2},
        marks=[{"xy": [5.0, 4.0], "text": "crossing: no via here"}])


# ------------------------------------------------------------------------------------------------ K4 kept_wide_gnd_arc
def kept_wide_gnd_arc() -> Pat:
    Wd, Hd = 21.5, 16.5
    hdr = header_2x3("J1", 2.5, 10.5, ["P3V3", "GND", None, None, "SDA", "SCL"])
    ty = 1.4  # target row at the top edge: the pads' outer ends are 0.33 mm from the edge line (no wire passes)
    jst = jst_sh4("J2", 15.96, ty, ["P3V3", "SDA", "SCL", "GND"])  # pin row only (the 3V3 diagonal comes in where its mounting pad would be)
    gw = 1.125
    cxa, cya, ra = 12.9, 10.5, 2.5
    mid = (_r(cxa - ra / SQ2), _r(cya - ra / SQ2))
    gnd = [Trk("GND", "B.Cu", gw, [(2.5, 13.04), (2.5, 15.0), (10.4, 15.0), (10.4, 10.5)], locked=True),
           Trk("GND", "B.Cu", gw, [(12.9, 8.0), (18.96, 8.0)], locked=True),
           Trk("GND", "B.Cu", 0.3, [(18.96, 8.0), (18.96, ty)], locked=True)]
    arc = Arc("GND", "B.Cu", gw, (10.4, 10.5), mid, (12.9, 8.0))
    c3 = 18.36  # 3V3 diagonal x + y = c3: (cxa + cya - c3) / sqrt 2 - (ra + gw / 2) - 0.15 = 0.35 mm free (< one wire)
    v3 = Trk("P3V3", "B.Cu", 0.3, [(2.5, 10.5), (2.5, 8.6), (_r(c3 - 8.6), 8.6), (15.96, _r(c3 - 15.96)), (15.96, ty)])
    room = (cxa + cya - c3) / SQ2 - (ra + gw / 2) - 0.15
    via_s, via_c = (16.96, 2.6), (17.96, 2.6)
    ref = [Trk("SDA", "F.Cu", W, [(7.58, 10.5), (_r(19.56 - 10.5), 10.5), via_s]), Trk("SDA", "B.Cu", W, [via_s, (16.96, ty)]),
           Trk("SCL", "F.Cu", W, [(7.58, 13.04), (17.96, _r(20.62 - 17.96)), via_c]), Trk("SCL", "B.Cu", W, [via_c, (17.96, ty)])]
    return Pat(
        name="kept_wide_gnd_arc", cluster="K4", title="Kept wide GND arc + 3V3 leave no B.Cu room: I2C on the other layer",
        purpose="Kept hand copper on B.Cu: a 1.125 mm GND track with a 2.5 mm radius arc hugging a 2x3 header (from the "
                "header's GND pin round its right side and on to the target connector's GND pin), and a 0.3 mm 3V3 "
                "track from header pin 1 over the header to the target's 3V3 pin. SDA / SCL go from the header (PTH) to a "
                "JST SH connector on B.Cu at the top edge, between its 3V3 and GND pins. Between the 3V3 diagonal and "
                f"the GND arc only {room:.2f} mm is free (one wire needs {W + 2 * CL:.2f}). F.Cu is empty.",
        answer="SDA and SCL leave the header on F.Cu (PTH pins), run 45 deg up-right over the kept B.Cu copper and drop "
               "with one via each just below their JST pads (short B.Cu stubs). 2 vias, nothing touches the GND arc.",
        why="On B.Cu the targets are only reachable through the channel between the 3V3 and GND tracks (the JST row sits "
            "at the board edge, its pins close the channel's top), and the channel's bottleneck at the arc is narrower "
            "than one wire. F.Cu has ample room; the B.Cu target pads need the via. The failure class: wires drawn "
            "through a kept wide arc / track (overlap -0.66 mm, and 0.03 mm grazes the engine did not see).",
        size=(Wd, Hd), nets=["P3V3", "GND", "SDA", "SCL"], fps=[hdr, jst],
        connections=[("SDA", "J1.5", "J2.2"), ("SCL", "J1.6", "J2.3")],
        ref_tracks=ref, ref_vias=[Via("SDA", via_s), Via("SCL", via_c)], kept=gnd + [v3], kept_arcs=[arc],
        expect={"vias": 2, "layers": {"SDA": ["F.Cu", "B.Cu"], "SCL": ["F.Cu", "B.Cu"]}, "b_cu_room": round(room, 3)},
        marks=[{"xy": [_r(cxa - (ra + gw / 2 + 0.25) / SQ2), _r(cya - (ra + gw / 2 + 0.25) / SQ2)],
                "text": f"B.Cu bottleneck {room:.2f} mm"}])


# ------------------------------------------------------------------------------------------------ K5 jst_fanout_45
def jst_fanout_45() -> Pat:
    Wd, Hd = 15.0, 13.2
    x0, pc = 11.0, 2.0  # upper JST: GND(nc), 3V3, SDA, SCL from x0 in +x, pins at y pc (B.Cu), pin row only
    j1 = jst_sh4("J1", x0, pc, [None, "P3V3", "SDA", "SCL"])
    xb, yb = 3.0, 11.8  # lower JST, same pin order, 8 mm to the left
    j2 = jst_sh4("J2", xb, yb, [None, "P3V3", "SDA", "SCL"])
    y1, y2 = _r(pc + 0.9), _r(yb - 0.9)  # 0.9 mm straight out of the pads (past the 0.775 mm pad tip), then 45 deg

    def run(net, i, w, locked=False):
        return Trk(net, "B.Cu", w, [(x0 + i, pc), (x0 + i, y1), (xb + i, y2), (xb + i, yb)], locked=locked)

    v3 = run("P3V3", 1, 0.3, locked=True)
    ref = [run("SDA", 2, W), run("SCL", 3, W)]
    return Pat(
        name="jst_fanout_45", cluster="K5", title="I2C from a 1 mm JST beside kept 3V3: a 45 deg parallel pair, no crossing",
        purpose="Two JST SH 4-pin connectors on B.Cu (1.0 mm pitch, pin order GND, 3V3, SDA, SCL), the lower one 8 mm "
                "to the left. A kept hand 3V3 track joins their 3V3 pins: straight out of the upper pad, 45 deg "
                "down-left, straight into the lower pad. SDA and SCL must follow it on B.Cu (both ends are B.Cu SMD pads).",
        answer="0 vias, B.Cu. SDA and SCL leave their pads straight down, turn 45 deg down-left as a pair parallel to the "
               "kept 3V3 (1 mm pitch, 0.71 mm between centre lines) and drop straight into the lower connector's pins. "
               "Only 0 / 45 / 90 degree segments; SDA and SCL never cross.",
        why="The pin order and pitch are the same on both connectors, so the three wires are one parallel bundle; every "
            "monotone octilinear path from pad to pad has the same length (8 mm across, 9.8 mm down), so the 45 deg "
            "pair is optimal and needs no free-angle stretch. The failure class: SDA / SCL drawn as free-angle segments "
            "that cross each other next to the connector (any_angle forbidden in that route; never-drop forced copper).",
        size=(Wd, Hd), nets=["P3V3", "SDA", "SCL"], fps=[j1, j2],
        connections=[("SDA", "J1.3", "J2.3"), ("SCL", "J1.4", "J2.4")], ref_tracks=ref, kept=[v3],
        expect={"vias": 0, "layers": {"SDA": ["B.Cu"], "SCL": ["B.Cu"]}, "octilinear": True})


# ------------------------------------------------------------------------------------------------ K6 pth_wrap_other_layer
def pth_wrap_other_layer() -> Pat:
    Wd, Hd = 15.0, 14.0
    hx, hy = 3.0, 10.0
    hdr = header_2x3("J1", hx, hy, [None, None, "SDA", "SCL", None, None])
    p4 = (_r(hx + 2.54), _r(hy + 2.54))
    xm = _r(hx + 2.54 + 1.27)  # between columns 2 and 3
    t2 = (_r(xm + 6.0), _r(hy - 2.0 - 6.0))  # SCL target up-right
    tp2 = Fp("TP2", [Pad("1", "SCL", t2, (1.7, 1.7), "circle", drill=1.0)], "TestPoint:TestPoint_THTPad_D1.7mm_Drill1.0mm")
    t1 = (12.5, 7.0)  # SDA target, on the other side of the kept SCL track
    tp1 = Fp("TP1", [Pad("1", "SDA", t1, (1.7, 1.7), "circle", drill=1.0)], "TestPoint:TestPoint_THTPad_D1.7mm_Drill1.0mm")
    scl = Trk("SCL", "B.Cu", W, [p4, (xm, _r(hy + 1.27)), (xm, _r(hy - 2.0)), t2])
    p3 = (_r(hx + 2.54), hy)
    d = _r(p3[1] - t1[1])
    ref = [Trk("SDA", "F.Cu", W, [p3, (_r(p3[0] + d), t1[1]), t1])]
    return Pat(
        name="pth_wrap_other_layer", cluster="K6", title="Kept B.Cu SCL blocks SDA's way: SDA on F.Cu, not wrapped round a pin",
        purpose="A 2x3 PTH header; a kept hand SCL track leaves pin 4 on B.Cu, runs up between the pin columns and "
                "45 deg up-right to a THT test point. SDA (pin 3) goes to a THT test point on the other side of that "
                "track. F.Cu is empty.",
        answer="SDA on F.Cu: 45 deg up-right from pin 3 and straight into TP1 (crossing the B.Cu SCL on the other "
               "layer). 0 vias.",
        why="Both SDA ends are PTH, so F.Cu costs nothing and its straight line is the shortest path. On B.Cu SDA would have "
            "to go round the kept SCL's end at pin 4 (between pins 4 / 2 / 6, a longer way squeezed past the pads). The "
            "failure class: a never-drop wire forced round a PTH pad at 0.01-0.11 mm instead of the free other layer.",
        size=(Wd, Hd), nets=["SDA", "SCL"], fps=[hdr, tp1, tp2],
        connections=[("SDA", "J1.3", "TP1.1")], ref_tracks=ref, kept=[scl],
        expect={"vias": 0, "layers": {"SDA": ["F.Cu"]}})


# ------------------------------------------------------------------------------------------------ K7 via_in_fanin
def via_in_fanin() -> Pat:
    Wd, Hd = 14.0, 11.0
    py, pp = 9.0, 1.27
    xs = [3.0 + pp * i for i in range(4)]
    row = Fp("U1", [Pad(str(i + 1), n, (_r(x), py), (0.8, 1.6), layer="B.Cu")
                    for i, (x, n) in enumerate(zip(xs, ["SDA", "SCL", "VX", None]))], "module pin row, 1.27 mm pitch, B.Cu")
    ta = Fp("T1", [Pad("1", "SDA", (12.6, 2.0), (1.0, 0.5), layer="B.Cu"), Pad("2", "SCL", (12.6, 2.8), (1.0, 0.5), layer="B.Cu")])
    tv = Fp("T2", [Pad("1", "VX", (_r(xs[2]), 1.2), (0.6, 1.0))])
    yk = 7.0  # SDA / SCL leave their pins straight up and bend 45 deg here
    cS, cC = xs[0] + yk, xs[1] + yk  # 45 deg lines x + y = c
    need = (VIA_D / 2 + CL + W / 2) * SQ2
    yv_min = _r(cC - xs[2] + need)
    vv = (_r(xs[2]), 7.4)
    ref = [Trk("SDA", "B.Cu", W, [(xs[0], py), (xs[0], yk), (_r(cS - 2.0), 2.0), (12.6, 2.0)]),
           Trk("SCL", "B.Cu", W, [(_r(xs[1]), py), (_r(xs[1]), yk), (_r(cC - 2.8), 2.8), (12.6, 2.8)]),
           Trk("VX", "B.Cu", W, [(_r(xs[2]), py), vv]), Trk("VX", "F.Cu", W, [vv, (_r(xs[2]), 1.2)])]
    return Pat(
        name="via_in_fanin", cluster="K7", title="A third net's via stays out of a bundle's fan-in corridor",
        purpose="A module pin row on B.Cu (1.27 mm pitch). SDA (outer pin) and SCL (middle pin) fan in from pads at the "
                "upper right on B.Cu; VX (the pin right of SCL) goes straight up to an F.Cu pad, so it needs one via and "
                "its column crosses the SDA / SCL bundle. Free room all round.",
        answer=f"1 via. VX drops to F.Cu right above its own pin (via at y >= {yv_min}, e.g. {list(vv)}), so it is on F.Cu "
               "where the bundle crosses its column; SDA and SCL run straight up from their pins, bend 45 deg and run "
               "parallel to their pads, every bend at clearance.",
        why="VX must be on F.Cu where the bundle crosses it, so its via goes below the crossing; anywhere between its pin "
            "and the bundle is equally short, and nothing makes it sit inside the corridor. The failure class: the via "
            "placed in the fan-in corridor, the bundle bent round it with stub segments at 0 mm gap (a knot), plus "
            "free-angle legs grazing at 0.12-0.14 mm the engine did not see.",
        size=(Wd, Hd), nets=["SDA", "SCL", "VX"], fps=[row, ta, tv],
        connections=[("SDA", "U1.1", "T1.1"), ("SCL", "U1.2", "T1.2"), ("VX", "U1.3", "T2.1")],
        ref_tracks=ref, ref_vias=[Via("VX", vv)], expect={"vias": 1, "via_y_min": yv_min},
        marks=[{"xy": [xs[2], _r(cC - xs[2])], "text": "corridor: no via"}])


# ------------------------------------------------------------------------------------------------ K8 pair_crossing_layered
def pair_crossing_layered() -> Pat:
    Wd, Hd = 12.0, 10.0
    ps = (0.3, 1.0)
    fps = [Fp("J1", [Pad("1", "P", (5.75, 0.9), ps), Pad("2", "N", (6.25, 0.9), ps)], "edge pads, 0.5 mm pitch"),
           Fp("J2", [Pad("1", "P", (5.75, 9.1), ps), Pad("2", "N", (6.25, 9.1), ps)], "edge pads, 0.5 mm pitch"),
           Fp("J3", [Pad("1", "X", (0.9, 5.0), (1.0, 0.5))]), Fp("J4", [Pad("1", "X", (11.1, 5.0), (1.0, 0.5))])]
    v1, v2 = (4.5, 5.0), (7.5, 5.0)
    ref = [Trk("P", "F.Cu", W, [(5.75, 0.9), (5.75, 9.1)]), Trk("N", "F.Cu", W, [(6.25, 0.9), (6.25, 9.1)]),
           Trk("X", "F.Cu", W, [(0.9, 5.0), v1]), Trk("X", "B.Cu", W, [v1, v2]), Trk("X", "F.Cu", W, [v2, (11.1, 5.0)])]
    return Pat(
        name="pair_crossing_layered", cluster="K8", title="A planned crossing gets its layer change (2 vias), never a same-layer cross",
        purpose="A pair (P, N) runs top edge to bottom edge, X runs left edge to right edge; all pads are F.Cu SMD at the "
                "board edges (the outline's copper clearance closes the way round every pad: X has to cross the pair).",
        answer="P and N straight on F.Cu; X straight, dives to B.Cu under the pair with one via on each side (vias at "
               "clearance from the pair). 2 vias.",
        why="The four ends alternate round the outline, so X and the pair must cross; the cheapest crossing moves the "
            "single wire (2 vias) rather than the pair (4). The failure class: the plan's crossing drawn on one layer "
            "(tracks_crossing and shorts between long F.Cu runs).",
        size=(Wd, Hd), nets=["P", "N", "X"], fps=fps,
        connections=[("P", "J1.1", "J2.1"), ("N", "J1.2", "J2.2"), ("X", "J3.1", "J4.1")],
        ref_tracks=ref, ref_vias=[Via("X", v1), Via("X", v2)], expect={"vias": 2})


# ------------------------------------------------------------------------------------------------ K9 roundrect_corner_pass
def roundrect_corner_pass() -> Pat:
    Wd, Hd = 13.0, 13.0
    # 0805 (Capacitor_SMD:C_0805_2012Metric: 1.0 x 1.45 roundrect, ratio 0.25) flipped to B.Cu; pad 2 at (6.95, 6)
    c = Fp("C1", [Pad("1", None, (5.05, 6.0), (1.0, 1.45), "roundrect", layer="B.Cu"),
                  Pad("2", None, (6.95, 6.0), (1.0, 1.45), "roundrect", layer="B.Cu")], "Capacitor_SMD:C_0805_2012Metric", flip=True)
    k = (7.2, 6.475)  # centre of pad 2's lower-right corner radius (0.25)
    gap = W + 2 * CL + 0.06  # 0.56: room for one wire + 0.06 mm when the corner is round
    pc = (_r(k[0] + (0.25 + gap + 0.5) / SQ2), _r(k[1] + (0.25 + gap + 0.5) / SQ2))
    fid = Fp("FD1", [Pad("1", None, pc, (1.0, 1.0), "circle", layer="B.Cu")], "Fiducial:Fiducial_1mm_Mask2mm (copper dot)")
    m = (k[0] + (0.25 + gap / 2) / SQ2, k[1] + (0.25 + gap / 2) / SQ2)
    s = m[0] + m[1]
    a = (_r(m[0] - 3.0), _r(s - (m[0] - 3.0)))
    b = (_r(m[0] + 3.0), _r(s - (m[0] + 3.0)))
    pa = Fp("TA", [Pad("1", "SIG", a, (0.6, 0.6), "circle", layer="B.Cu")])
    pb = Fp("TB", [Pad("1", "SIG", b, (0.6, 0.6), "circle", layer="B.Cu")])
    sharp = (s - (7.45 + 6.725)) / SQ2 - W / 2
    return Pat(
        name="roundrect_corner_pass", cluster="K9", title="A 45 deg wire through the gap at a roundrect pad's rounded corner",
        purpose="An 0805 footprint flipped to B.Cu (roundrect pads, radius 0.25 mm) and a 1 mm copper dot leave a "
                f"diagonal gap of {gap:.2f} mm between the dot and pad 2's rounded corner: one 0.2 mm wire at 0.15 mm "
                "clearance fits with 0.06 mm to spare. SIG runs from TA to TB straight through it, B.Cu only.",
        answer="SIG straight from TA to TB (one 45 deg segment) through the middle of the gap: 0.18 mm to the rounded "
               "corner and to the dot. 0 vias.",
        why=f"The straight line is the shortest path; it fits only because the corner is round (a sharp corner would "
            f"leave {sharp:.3f} mm). The failure class: a wire past a roundrect pad corner that KiCad measures at 0.077 "
            "mm while the engine saw no violation (corner model / flipped-footprint pad geometry).",
        size=(Wd, Hd), nets=["SIG"], fps=[c, fid, pa, pb],
        connections=[("SIG", "TA.1", "TB.1")], ref_tracks=[Trk("SIG", "B.Cu", W, [a, b])],
        expect={"vias": 0, "layers": {"SIG": ["B.Cu"]}, "sharp_corner_gap": round(sharp, 3)},
        marks=[{"xy": [_r(m[0]), _r(m[1])], "text": f"gap {gap:.2f}"}])


PATTERNS = {f.__name__: f for f in (usbc_pair_esd, fpc_exit_fanout, via_pair_cross, kept_wide_gnd_arc, jst_fanout_45,
                                    pth_wrap_other_layer, via_in_fanin, pair_crossing_layered, roundrect_corner_pass)}


def ref_length(pat: Pat) -> float:
    return round(sum(plen(t.pts) for t in pat.ref_tracks), 4)


def expected_doc(pat: Pat) -> dict:
    return {"name": pat.name, "cluster": pat.cluster, "title": pat.title, "status": STATUS, "purpose": pat.purpose,
            "expected": pat.answer, "why": pat.why, "size": list(pat.size),
            "footprints": {f.ref: f.lib for f in pat.fps if f.lib},
            "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
            "reference_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": [list(p) for p in t.pts]} for t in pat.ref_tracks],
            "reference_vias": [list(v.pos) for v in pat.ref_vias],
            "kept_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": [list(p) for p in t.pts]} for t in pat.kept],
            "kept_arcs": [{"net": a.net, "layer": a.layer, "width": a.width, "start": list(a.start), "mid": list(a.mid),
                           "end": list(a.end)} for a in pat.kept_arcs],
            "ref_len": ref_length(pat), **pat.expect,
            "rules_assumed": {"authority": "board"}}


def generate(name: str, out: Path, *, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = clearance_report(pat)
    if bad:
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = expected_doc(pat)
    if drc:
        from harness.drc import run_drc

        rep = run_drc(paths["reference"], d / "reference" / "drc.json")
        doc["reference_drc"] = {"violations": len(rep.violations), "unconnected": len(rep.unconnected),
                                "detail": [str(v) for v in rep.violations][:10]}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    return paths


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    drc = "--drc" in argv
    argv = [a for a in argv if a != "--drc"]
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "drc-patterns" / "boards"
    names = argv[1:] or list(PATTERNS)
    for name in names:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
