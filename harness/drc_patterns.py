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
    # ESD array, flow-through (Package_DFN_QFN:Diodes_UDFN-10_1x2.5mm_P0.5mm at 180 deg): two pin columns along y, pin k
    # (right column) faces pin 11 - k (left column); DP on 5 / 6 (the row nearest the receptacle), DN on 4 / 7
    ex, ey = cx + 0.5, _r(yr - 3.405)
    esd = []
    for k in range(1, 6):
        y = _r(ey - 1.0 + 0.5 * (k - 1))
        if k == 3:
            # GND pins 3 / 8 have no net here: pin 3 0.03 mm narrower than the library pad so the two keep 0.157 mm
            esd += [Pad("3", None, (_r(ex + 0.365), y), (0.67, 0.45)), Pad("8", None, (_r(ex - 0.412), y), (0.57, 0.45))]
        else:
            net = {5: "DP", 4: "DN"}.get(k)
            esd += [Pad(str(k), net, (_r(ex + 0.388), y), (0.62, 0.25)), Pad(str(11 - k), net, (_r(ex - 0.388), y), (0.62, 0.25))]
    u2 = Fp("U2", esd, "Package_DFN_QFN:Diodes_UDFN-10_1x2.5mm_P0.5mm")
    yp, yn = _r(ey + 1.0), _r(ey + 0.5)  # DP and DN pin rows of the ESD
    t = Fp("J2", [Pad("1", "DP", (0.9, yp), (0.8, 0.3)), Pad("2", "DN", (0.9, yn), (0.8, 0.3))], "generic 0.5 mm pitch pads")
    yl = _r(top - CL - W / 2 - 0.005)  # DP join above the row, just clear of A7's tip
    yd = _r(bot + CL + W / 2 + 0.005)  # DN join below the row
    xr = _r(ex + 0.388 + 0.31 + CL + W / 2 + 0.012)  # DN rises right of ESD pin 5's outer end
    yk = 5.6
    ref = [Trk("DP", "F.Cu", W, [(_r(cx - 0.75), yr), (_r(cx - 0.75), yl), (_r(cx + 0.25), yl), (_r(cx + 0.25), yr)]),
           Trk("DP", "F.Cu", W, [(_r(cx + 0.25), yl), (_r(cx + 0.25), yp)]),
           Trk("DP", "F.Cu", W, [(0.9, yp), (_r(ex + 0.388), yp)]),
           Trk("DN", "F.Cu", W, [(_r(cx - 0.25), yr), (_r(cx - 0.25), yd), (_r(cx + 0.75), yd), (_r(cx + 0.75), yr)]),
           Trk("DN", "F.Cu", W, [(_r(cx + 0.75), yr), (_r(cx + 0.75), yk), (xr, _r(yk - (xr - cx - 0.75))), (xr, yn), (0.9, yn)])]
    return Pat(
        name="usbc_pair_esd", cluster="K1", title="USB pair: duplicated receptacle pins joined, flow-through ESD, 0 vias",
        purpose="A USB-C receptacle's signal row (0.3 x 1.45 pads at 0.5 mm pitch: no wire passes between pins) carries "
                "the data pair twice, interleaved: DP on B6 / A6, DN on A7 / B7. Above it a flow-through ESD array "
                "(UDFN-10, 0.5 mm pitch) in the library orientation turned 180 deg: two pin columns, pin 6 faces pin 5 "
                "(DP, the row nearest the receptacle), pin 7 faces pin 4 (DN). The pair goes on to two pads at the left "
                "edge. Everything is on F.Cu; the other pins have no net.",
        answer="0 vias, all F.Cu. DP joins B6 and A6 just above the row (clear of A7's tip), DN joins A7 and B7 just "
               "below it. DP runs from A6 straight up into ESD pin 6 and on to the left along its pin row (pin 5 joined "
               "under the part); DN rises from B7 right of the ESD, enters pin 4, runs under the part through pin 7 "
               "and on to the left, 0.5 mm above DP. Regression check: topor solves it with this orientation.",
        why="The interleaved pins force one join above and one below the row, and both orders leave DP's riser left of "
            "DN's. DP's riser lands on the near ESD row; DN, coming up on the right, reaches its row from the outer end "
            "of pin 4, so the two never cross and both leave the part on the left. The failure class (with the ESD "
            "turned 90 deg in round 1): wires drawn between the 0.5 mm pins and a crossing drawn on one layer.",
        size=(Wd, Hd), nets=["DP", "DN"], fps=[j1, u2, t],
        connections=[("DP", "J1.B6", "J1.A6"), ("DP", "J1.A6", "U2.6"), ("DP", "U2.6", "U2.5"), ("DP", "U2.6", "J2.1"),
                     ("DN", "J1.A7", "J1.B7"), ("DN", "J1.B7", "U2.4"), ("DN", "U2.4", "U2.7"), ("DN", "U2.7", "J2.2")],
        ref_tracks=ref, expect={"vias": 0, "layers": {"DP": ["F.Cu"], "DN": ["F.Cu"]}},
        marks=[{"xy": [cx - 0.25, top - 0.05], "text": "A7 tip: DP passes at clearance"}])


# ------------------------------------------------------------------------------------------------ K2 fpc_exit_fanout
def fpc_exit_fanout() -> Pat:
    Wd, Hd = 9.5, 10.0
    px, y0, n = 1.5, 2.0, 10  # FPC column: pins along y at 0.5 mm pitch, pads 1.1 x 0.3 along x (tip at x 2.05)
    sig = {i: f"S{i}" for i in range(1, 9)}  # 8 signals on pins 2..9; pins 1 and 10 without net
    ys = [_r(y0 + 0.5 * i) for i in range(n)]
    j1 = Fp("J1", [Pad(str(i + 1), sig.get(i), (px, y), (1.1, 0.3)) for i, y in enumerate(ys)],
            "Connector_FFC-FPC:TE_2-1734839-2_1x22-1MP_P0.5mm_Horizontal (pad size and pitch)")
    yb = 9.0  # second FPC: a row along x at the bottom, pads 0.3 x 1.1 along y (tip at y 8.45)
    bx, y2 = 2.1, _r(yb - 0.6)  # every wire bends just past the pad tip on both connectors
    xs = [_r(bx - ys[i] + y2) for i in range(n)]  # pin i of J1 lands on pin i of J2: 45 deg lines x - y = bx - y_i
    j2 = Fp("J2", [Pad(str(i + 1), sig.get(i), (xs[i], yb), (0.3, 1.1)) for i in range(n)],
            "Connector_FFC-FPC:TE_2-1734839-2_1x22-1MP_P0.5mm_Horizontal (pad size and pitch)")
    ref = [Trk(sig[i], "F.Cu", W, [(px, ys[i]), (bx, ys[i]), (xs[i], y2), (xs[i], yb)]) for i in sig]
    pitch45 = 0.5 / SQ2
    return Pat(
        name="fpc_exit_fanout", cluster="K2", title="Eight wires between two 0.5 mm FPCs: exit past the pad tip, 45 deg bundle",
        purpose="Two 0.5 mm pitch FPC connectors (0.3 x 1.1 pads, end pins without net): a column at the left edge "
                "and a row at the bottom, 8 signals S1..S8 from one to the other in the same order. F.Cu only. The "
                f"45 deg bundle between them has a pitch of {pitch45:.3f} mm (one wire + clearance = {W + CL:.2f}).",
        answer=f"0 vias. Every wire leaves its pad straight along the pad axis to just past the tip (x {bx}), runs "
               f"45 deg down-right in a bundle at {pitch45:.3f} mm pitch and drops straight into the other connector's "
               "pad (bend at y {:.1f}). No wire leaves a pad at an angle.".format(y2),
        why="A wire that leaves a 0.5 mm pitch pad at 45 deg from its centre crosses the 0.2 mm gap into the next pad's "
            "clearance (0.04 mm left); straight to the tip and then 45 deg keeps the neighbour's corner at 0.18 mm. "
            "Every monotone octilinear path has the same length, and the bundle leaves no spare room (pitch 0.354 vs "
            "0.35), so the wires must leave their pads in order and in line. The failure class: the drawing forcing "
            "an exit at an angle into the next pad's space.",
        size=(Wd, Hd), nets=[sig[i] for i in sig], fps=[j1, j2],
        connections=[(sig[i], f"J1.{i + 1}", f"J2.{i + 1}") for i in sig], ref_tracks=ref,
        expect={"vias": 0, "layers": {sig[i]: ["F.Cu"] for i in sig}, "octilinear": True},
        marks=[{"xy": [_r(px + 0.55), ys[1] - 0.25], "text": "pad tip"}])


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


# ------------------------------------------------------------------------------------------------ K3 via_grid_10
def via_grid_10() -> Pat:
    Wd, Hd = 12.0, 12.0
    pp, g0, k = 1.0, 4.0, 5  # 5 x 5 crossings at 1 mm pitch
    sq = (0.6, 0.6)
    gs = [_r(g0 + pp * i) for i in range(k)]
    fps, ref, vias, nets, conns = [], [], [], [], []
    yv, xv = 3.0, 9.0  # the vertical nets dive above the grid, the horizontal ones right of it
    for i, g in enumerate(gs):
        h, v = f"H{i + 1}", f"V{i + 1}"
        nets += [h, v]
        fps += [Fp(f"HL{i + 1}", [Pad("1", h, (1.2, g), sq)]), Fp(f"HR{i + 1}", [Pad("1", h, (_r(Wd - 1.2), g), sq, layer="B.Cu")]),
                Fp(f"VT{i + 1}", [Pad("1", v, (g, 1.2), sq)]), Fp(f"VB{i + 1}", [Pad("1", v, (g, _r(Hd - 1.2)), sq, layer="B.Cu")])]
        conns += [(h, f"HL{i + 1}.1", f"HR{i + 1}.1"), (v, f"VT{i + 1}.1", f"VB{i + 1}.1")]
        ref += [Trk(h, "F.Cu", W, [(1.2, g), (xv, g)]), Trk(h, "B.Cu", W, [(xv, g), (_r(Wd - 1.2), g)]),
                Trk(v, "F.Cu", W, [(g, 1.2), (g, yv)]), Trk(v, "B.Cu", W, [(g, yv), (g, _r(Hd - 1.2))])]
        vias += [Via(h, (xv, g)), Via(v, (g, yv))]
    need = VIA_D / 2 + CL + W / 2
    return Pat(
        name="via_grid_10", cluster="K3", title="Ten layer-changing nets cross in a 5 x 5 grid: vias outside the crossings",
        purpose="Five nets run left to right (F.Cu pad left, B.Cu pad right) and five top to bottom (F.Cu pad top, B.Cu "
                "pad bottom), 1 mm apart: 25 crossings. Each of the ten nets needs exactly one via; free room on every "
                "side of the grid.",
        answer=f"10 straight lines, 10 vias. The vertical nets dive above the grid (vias in a row at y {yv}), the "
               f"horizontal ones right of it (a column at x {xv}): every crossing is F.Cu over B.Cu and no via is nearer "
               f"than {need:.2f} mm to another net's wire. (Mirror answers - verticals below, horizontals left, etc. - "
               "are equally short.)",
        why="Straight lines are the shortest; one via per net is the minimum because the pads are on different layers. "
            "Inside the grid there is no via site at all (1 mm pitch leaves 0.6 mm from a via centre to the next wire, "
            "the via's own crossing wires block both layers), so each via goes where its net leaves the grid on the "
            "correct layer. The failure class, under pressure: vias of several nets bunched into the crossings below "
            "clearance while free room is beside them.",
        size=(Wd, Hd), nets=nets, fps=fps, connections=conns, ref_tracks=ref, ref_vias=vias, expect={"vias": 2 * k},
        marks=[{"xy": [gs[2], gs[2]], "text": "no via in the grid"}])


# ------------------------------------------------------------------------------------------------ K4 kept_wide_gnd_arc
def kept_wide_gnd_arc() -> Pat:
    Wd, Hd = 21.5, 16.5
    hx = 3.5
    hdr = header_2x3("J1", hx, 10.5, ["P3V3", "GND", None, None, "SDA", "SCL"])
    ty = 1.4  # target row at the top edge: the pads' outer ends are 0.33 mm from the edge line (no wire passes)
    jst = jst_sh4("J2", 15.96, ty, ["P3V3", "SDA", "SCL", "GND"])  # pin row only (the 3V3 diagonal comes in where its mounting pad would be)
    tp = Fp("TP3", [Pad("1", "GND", (1.3, 5.0), (1.7, 1.7), "circle", drill=1.0)], "TestPoint:TestPoint_THTPad_D1.7mm_Drill1.0mm")
    gw = 1.125
    cxa, cya, ra = 12.9, 10.5, 2.5
    mid = (_r(cxa - ra / SQ2), _r(cya - ra / SQ2))
    ye = _r(Hd - EDGE - gw / 2 - 0.09)  # the edge trace's bottom run, 0.09 mm inside the copper-to-edge line
    xl, re = 1.3, 2.5  # its left run and the radius of its bottom-left corner
    ce = (_r(xl + re), _r(ye - re))
    gnd = [Trk("GND", "B.Cu", gw, [(xl, 5.0), (xl, ce[1])], locked=True),
           Trk("GND", "B.Cu", gw, [(ce[0], ye), (10.4, ye), (10.4, 10.5)], locked=True),
           Trk("GND", "B.Cu", gw, [(hx, _r(10.5 + 2.54)), (hx, ye)], locked=True),
           Trk("GND", "B.Cu", gw, [(12.9, 8.0), (18.96, 8.0)], locked=True),
           Trk("GND", "B.Cu", 0.3, [(18.96, 8.0), (18.96, ty)], locked=True)]
    arc = Arc("GND", "B.Cu", gw, (10.4, 10.5), mid, (12.9, 8.0))
    edge_arc = Arc("GND", "B.Cu", gw, (xl, ce[1]), (_r(ce[0] - re / SQ2), _r(ce[1] + re / SQ2)), (ce[0], ye))
    c3 = 18.36  # 3V3 diagonal x + y = c3: (cxa + cya - c3) / sqrt 2 - (ra + gw / 2) - 0.15 = 0.35 mm free (< one wire)
    v3 = Trk("P3V3", "B.Cu", 0.3, [(hx, 10.5), (hx, 8.6), (_r(c3 - 8.6), 8.6), (15.96, _r(c3 - 15.96)), (15.96, ty)])
    room = (cxa + cya - c3) / SQ2 - (ra + gw / 2) - 0.15
    via_s, via_c = (16.96, 2.6), (17.96, 2.6)
    p5, p6 = (_r(hx + 5.08), 10.5), (_r(hx + 5.08), 13.04)
    cs, cc = 19.56, _r(p6[0] + p6[1])  # SDA / SCL F.Cu diagonals x + y = c
    ref = [Trk("SDA", "F.Cu", W, [p5, (_r(cs - 10.5), 10.5), via_s]), Trk("SDA", "B.Cu", W, [via_s, (16.96, ty)]),
           Trk("SCL", "F.Cu", W, [p6, (17.96, _r(cc - 17.96)), via_c]), Trk("SCL", "B.Cu", W, [via_c, (17.96, ty)])]
    return Pat(
        name="kept_wide_gnd_arc", cluster="K4", title="Kept wide GND arcs + 3V3 leave no B.Cu room: I2C on the other layer",
        purpose="Kept hand copper on B.Cu: a 1.125 mm GND trace along the board edge (down the left edge, round the "
                "bottom-left corner on a 2.5 mm arc, along the bottom edge), up beside a 2x3 header and round it on a "
                "2.5 mm arc to the target connector's GND pin; the header's GND pin drops onto the edge trace. A 0.3 mm "
                "3V3 track runs from header pin 1 over the header to the target's 3V3 pin. SDA / SCL go from the header "
                "(PTH) to a JST SH connector on B.Cu at the top edge, between its 3V3 and GND pins. Between the 3V3 "
                f"diagonal and the GND arc only {room:.2f} mm is free (one wire needs {W + 2 * CL:.2f}). F.Cu is empty.",
        answer="SDA and SCL leave the header on F.Cu (PTH pins), run 45 deg up-right over the kept B.Cu copper and drop "
               "with one via each just below their JST pads (short B.Cu stubs). 2 vias, nothing touches either GND arc.",
        why="On B.Cu the targets are only reachable through the channel between the 3V3 and GND tracks (the JST row sits "
            "at the board edge, its pins close the channel's top; the GND edge trace closes the bottom), and the "
            "channel's bottleneck at the arc is narrower than one wire. F.Cu has ample room; the B.Cu target pads need "
            "the via. The failure class: wires drawn through a kept wide arc / track (overlap -0.66 mm, and 0.03 mm "
            "grazes the engine did not see).",
        size=(Wd, Hd), nets=["P3V3", "GND", "SDA", "SCL"], fps=[hdr, jst, tp],
        connections=[("SDA", "J1.5", "J2.2"), ("SCL", "J1.6", "J2.3")],
        ref_tracks=ref, ref_vias=[Via("SDA", via_s), Via("SCL", via_c)], kept=gnd + [v3], kept_arcs=[arc, edge_arc],
        expect={"vias": 2, "layers": {"SDA": ["F.Cu", "B.Cu"], "SCL": ["F.Cu", "B.Cu"]}, "b_cu_room": round(room, 3)},
        marks=[{"xy": [_r(cxa - (ra + gw / 2 + 0.25) / SQ2), _r(cya - (ra + gw / 2 + 0.25) / SQ2)],
                "text": f"B.Cu bottleneck {room:.2f} mm"},
               {"xy": [_r(ce[0] - re / SQ2), _r(ce[1] + re / SQ2)], "text": "kept edge arc"}])


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


# ------------------------------------------------------------------------------------------------ K5 jst_fanout_45_close
def jst_fanout_45_close() -> Pat:
    Wd, Hd = 15.0, 9.0
    x0, pc = 11.0, 2.0  # upper JST, pin row only (GND(nc), 3V3, SDA, SCL)
    xb, yb = 3.0, 7.8  # lower JST, 8 mm to the left but only 5.8 mm lower: the bundle needs a horizontal stretch
    j1 = jst_sh4("J1", x0, pc, [None, "P3V3", "SDA", "SCL"])
    j2 = jst_sh4("J2", xb, yb, [None, "P3V3", "SDA", "SCL"])
    y1, y2 = _r(pc + 0.9), _r(yb - 0.9)
    hm, off = _r((y1 + y2) / 2), 1 / SQ2  # SDA's horizontal halfway; the bundle keeps a 0.71 mm pitch on every stretch

    def run(net, i, w, locked=False):  # i = 1 (3V3), 2 (SDA), 3 (SCL)
        h = _r(hm + (i - 2) * off)
        c, e = x0 + i + y1, xb + i + y2  # first and second 45 deg lines x + y = c / e
        return Trk(net, "B.Cu", w, [(x0 + i, pc), (x0 + i, y1), (_r(c - h), h), (_r(e - h), h), (xb + i, y2), (xb + i, yb)],
                   locked=locked)

    v3 = run("P3V3", 1, 0.3, locked=True)
    ref = [run("SDA", 2, W), run("SCL", 3, W)]
    return Pat(
        name="jst_fanout_45_close", cluster="K5", title="JST to JST closer together: four 45 deg bends per wire, parallel pair",
        purpose="The jst_fanout_45 pair with the lower connector only 5.8 mm below the upper one (still 8 mm to the "
                "left): the kept hand 3V3 track runs straight out of its pad, 45 deg down-left, left, 45 deg down-left "
                "and straight into the lower pad. SDA and SCL must follow it on B.Cu.",
        answer="0 vias, B.Cu. SDA and SCL follow the kept 3V3 as a parallel bundle (0.71 mm pitch on the 45 deg and "
               "horizontal stretches, 1 mm at the pads): out of the pad, 45 deg, horizontal, 45 deg, into the pad. Four "
               "45 deg bends per wire, no 90 deg corner, no free-angle segment, no crossing.",
        why="The pads need a straight exit (0.9 mm) at both ends; the 8 mm across is more than the 4 mm left for the "
            "diagonals, so each wire needs a horizontal stretch: two 45 deg turns into it and two out. Every monotone "
            "octilinear path has the same length, so the parallel bundle is optimal. The failure class: SDA / SCL "
            "drawn as free-angle segments that cross each other next to the connector.",
        size=(Wd, Hd), nets=["P3V3", "SDA", "SCL"], fps=[j1, j2],
        connections=[("SDA", "J1.3", "J2.3"), ("SCL", "J1.4", "J2.4")], ref_tracks=ref, kept=[v3],
        expect={"vias": 0, "layers": {"SDA": ["B.Cu"], "SCL": ["B.Cu"]}, "octilinear": True, "bends": {"SDA": 4, "SCL": 4}})


# ------------------------------------------------------------------------------------------------ K6 pth_wrap_pin
def pth_wrap_pin() -> Pat:
    Wd, Hd = 15.0, 16.0
    hx, hy = 3.0, 10.0
    hdr = header_2x3("J1", hx, hy, [None, None, "SDA", "SCL", None, None])
    p3, p4 = (_r(hx + 2.54), hy), (_r(hx + 2.54), _r(hy + 2.54))
    xm, xr = _r(hx + 1.27), _r(hx + 2.54 + 1.27)  # between the pin columns
    t2 = (_r(xr + 6.0), _r(hy - 2.0 - 6.0))
    tp2 = Fp("TP2", [Pad("1", "SCL", t2, (1.7, 1.7), "circle", drill=1.0)], "TestPoint:TestPoint_THTPad_D1.7mm_Drill1.0mm")
    scl = Trk("SCL", "B.Cu", W, [p4, (xr, _r(hy + 1.27)), (xr, _r(hy - 2.0)), t2], locked=True)
    dmin = 0.85 + CL + W / 2 + 0.02  # SDA's last 45 deg leg passes pin 4 at clearance + 0.02 mm
    c = _r(p4[0] - p4[1] - dmin * SQ2)  # that leg: x - y = c
    t1 = (p4[0], _r(p4[0] - c))  # SDA target right below pin 4, on B.Cu
    tp1 = Fp("TP1", [Pad("1", "SDA", t1, (0.8, 0.8), "circle", layer="B.Cu")], "SMD test pad D0.8 mm")
    ref = [Trk("SDA", "B.Cu", W, [p3, (xm, _r(hy + 1.27)), (xm, _r(xm - c)), t1])]
    return Pat(
        name="pth_wrap_pin", cluster="K6", title="SDA wraps round a PTH pin on B.Cu at clearance (kept SCL blocks the other side)",
        purpose="A 2x3 PTH header; a kept hand SCL track leaves pin 4 on B.Cu up-right between the pin columns to a THT "
                "test point. SDA runs from pin 3 to an SMD pad on B.Cu right below pin 4 (0.33 mm from its ring): the "
                "straight line goes through pin 4, the right side is closed by SCL, so SDA must wrap round pin 4's left side.",
        answer="0 vias, B.Cu. SDA 45 deg down-left from pin 3 into the gap between the pin columns, straight down past "
               "pin 4 (0.32 mm each side), then 45 deg down-right round pin 4's lower-left into the pad, passing the "
               "ring at 0.17 mm (clearance 0.15). Octilinear only.",
        why="The wrap is the shortest path: going right of pin 4 crosses the kept SCL, and F.Cu would need a via at the "
            "B.Cu pad for no gain in length. The last leg passes a round PTH ring at clearance + 0.02 mm, so it tests "
            "the clearance of a 45 deg leg round a pin. The failure class: a wire wrapped round a PTH pad at 0.01-0.11 mm.",
        size=(Wd, Hd), nets=["SDA", "SCL"], fps=[hdr, tp1, tp2],
        connections=[("SDA", "J1.3", "TP1.1")], ref_tracks=ref, kept=[scl],
        expect={"vias": 0, "layers": {"SDA": ["B.Cu"]}, "octilinear": True},
        marks=[{"xy": [_r(p4[0] - 0.85 / SQ2 - 0.15), _r(p4[1] + 0.85 / SQ2 + 0.15)], "text": "0.17 mm to the ring"}])


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


# ------------------------------------------------------------------------------------------------ K9 via_slot_30 / via_slot_60
def _via_slot(angle: int) -> Pat:
    """A slot between two rows of via-sized PTH pads at `angle` deg; walls of the same pads close it off on both ends
    (open at the far ends, so going round is possible but long). Built for 30 deg in math coordinates (y up), mirrored
    in x = y for 60 deg, then turned into board coordinates (y down)."""
    s = 0.5  # half-width of the band the wire centre may use (via centre to wire centre >= 0.55, + 0.02)
    h = _r(s + VIA_D / 2 + CL + W / 2 + 0.02)  # row offset from the slot axis
    L = 25.5 * s  # slot length along its axis: 3 pivots carry the wire 21.9 s, 4 carry it 29.3 s (band model)
    sp, wall = 0.75, 12.0  # pad pitch (0.15 mm gap: no wire between) and wall length
    t30 = math.radians(30)
    u, n = (math.cos(t30), math.sin(t30)), (-math.sin(t30), math.cos(t30))

    def xy(a, b):
        return (a * u[0] + b * n[0], a * u[1] + b * n[1])

    k = math.ceil(L / sp)
    up = [xy(L * i / k, h) for i in range(k + 1)]
    lo = [xy(L * i / k, -h) for i in range(k + 1)]
    m = math.ceil(wall / sp)
    up += [(up[0][0], up[0][1] + sp * j) for j in range(1, m + 1)]  # upper wall: up from the slot's start
    lo += [(lo[-1][0], lo[-1][1] - sp * j) for j in range(1, m + 1)]  # lower wall: down from the slot's end
    # the expected path in slot coordinates (a along, b across): 45 deg legs climb at 15 deg to the axis, 0 deg legs
    # fall at 30 deg; legs bounce between b = +s and -s
    ta15, ta30 = math.tan(math.radians(15)), math.tan(t30)
    da45, da0 = 2 * s / ta15, 2 * s / ta30
    a1 = (L - (2 * da0 + da45)) / 2
    piv = [(a1, s), (a1 + da0, -s), (a1 + da0 + da45, s), (a1 + 2 * da0 + da45, -s)]
    d45 = (1 / SQ2, 1 / SQ2)
    p_in, p_out = xy(*piv[0]), xy(*piv[-1])
    lead = 3.0 + a1 / math.cos(math.radians(15))  # TA / TB 3 mm outside the slot on the first / last 45 deg leg
    ta = (p_in[0] - lead * d45[0], p_in[1] - lead * d45[1])
    tb = (p_out[0] + lead * d45[0], p_out[1] + lead * d45[1])
    path = [ta] + [xy(*p) for p in piv] + [tb]
    pts = up + lo
    if angle == 60:  # mirror in x = y: 30 deg -> 60 deg, 0 deg legs -> 90 deg, 45 deg stays
        pts = [(y, x) for x, y in pts]
        path = [(y, x) for x, y in path]
    else:
        assert angle == 30
    allp = pts + path
    mg = 1.6
    x0, y1 = min(p[0] for p in allp) - mg, max(p[1] for p in allp) + mg
    Wd = _r(max(p[0] for p in allp) + mg - x0)
    Hd = _r(y1 - (min(p[1] for p in allp) - mg))

    def bd(p):
        return (_r(p[0] - x0), _r(y1 - p[1]))

    vias = Fp("VW", [Pad(str(i + 1), None, bd(p), (VIA_D, VIA_D), "circle", drill=VIA_DR) for i, p in enumerate(pts)],
              "via-sized PTH pads 0.6 / 0.3, no net (stitching vias)")
    pa = Fp("TA", [Pad("1", "SIG", bd(path[0]), (0.6, 0.6), "circle", layer="B.Cu")])
    pb = Fp("TB", [Pad("1", "SIG", bd(path[-1]), (0.6, 0.6), "circle", layer="B.Cu")])
    leg = "0 deg" if angle == 30 else "90 deg"
    return Pat(
        name=f"via_slot_{angle}", cluster="K9", title=f"Through a slot between via rows at {angle} deg: 45 deg + {leg} legs, four pivots",
        purpose=f"Two rows of via-sized PTH pads (0.6 / 0.3, 0.75 mm pitch, no net) form a slot at {angle} deg, "
                f"{L:.2f} mm long, {2 * h - VIA_D:.2f} mm between the rows; walls of the same pads close the board on "
                "both sides of it (open at the far ends, so a wire can go round, a long way). SIG runs from TA before "
                "the slot to TB after it, B.Cu SMD pads; the pads block both layers.",
        answer=f"SIG through the slot, octilinear only: 45 deg in, then {leg} / 45 deg / {leg} / 45 deg legs touching "
               "alternately the two band edges (4 pivots in the slot, each 0.57 mm from the via row), 45 deg out to "
               "TB. 0 vias; no free-angle segment, not round the walls.",
        why=f"The straight {angle} deg line is not octilinear. In a band {2 * s:.1f} mm wide a 45 deg leg drifts across "
            f"it in {da45:.2f} mm along the slot and a {leg} leg in {da0:.2f} mm, so with three pivots the wire gets "
            f"at most {(2 * da45 + 2 * da0):.2f} mm through the {L:.2f} mm slot; four pivots carry it "
            f"{(3 * da45 + 2 * da0):.2f} mm. Going round the walls is about twice as long. The failure class: a "
            "wire past obstacles off the 45 deg grid drawn at a free angle or too close to them.",
        size=(Wd, Hd), nets=["SIG"], fps=[vias, pa, pb], connections=[("SIG", "TA.1", "TB.1")],
        ref_tracks=[Trk("SIG", "B.Cu", W, [bd(p) for p in path])],
        expect={"vias": 0, "layers": {"SIG": ["B.Cu"]}, "octilinear": True, "bends": {"SIG": 4}},
        marks=[{"xy": list(bd(xy(L / 2, 0) if angle == 30 else xy(L / 2, 0)[::-1])), "text": f"slot {angle} deg"}])


def via_slot_30() -> Pat:
    return _via_slot(30)


def via_slot_60() -> Pat:
    return _via_slot(60)


PATTERNS = {f.__name__: f for f in (usbc_pair_esd, fpc_exit_fanout, via_pair_cross, via_grid_10, kept_wide_gnd_arc,
                                    jst_fanout_45, jst_fanout_45_close, pth_wrap_pin, via_in_fanin, pair_crossing_layered,
                                    roundrect_corner_pass, via_slot_30, via_slot_60)}
# round 2 (user review of the round 1 PNGs): patterns new or changed in it
ROUND2 = {"usbc_pair_esd": "changed", "fpc_exit_fanout": "changed", "via_grid_10": "new", "kept_wide_gnd_arc": "changed",
          "jst_fanout_45_close": "new", "pth_wrap_pin": "new (replaces pth_wrap_other_layer)", "via_slot_30": "new",
          "via_slot_60": "new"}


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
