"""Synthetic fixture boards and project files (written to caller-supplied dirs)."""
import json
from pathlib import Path

import pcbnew

MM = 1_000_000


def _v(x, y):
    return pcbnew.VECTOR2I(int(round(x * MM)), int(round(y * MM)))


def _add_pad(fp, num, shape, size, pos, net, *, drill=None, angle=0.0, back=False):
    p = pcbnew.PAD(fp)
    p.SetNumber(num)
    if drill is None:
        p.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
        ls = pcbnew.LSET()
        ls.AddLayer(pcbnew.B_Cu if back else pcbnew.F_Cu)
        ls.AddLayer(pcbnew.B_Mask if back else pcbnew.F_Mask)
        ls.AddLayer(pcbnew.B_Paste if back else pcbnew.F_Paste)
        p.SetLayerSet(ls)
    else:
        p.SetAttribute(pcbnew.PAD_ATTRIB_PTH)
        p.SetLayerSet(p.PTHMask())
        p.SetDrillSize(_v(*drill))
    p.SetShape(shape)
    p.SetSize(_v(*size))
    p.SetPosition(_v(*pos))
    if angle:
        p.SetOrientation(pcbnew.EDA_ANGLE(angle, pcbnew.DEGREES_T))
    if net is not None:
        p.SetNet(net)
    fp.Add(p)
    return p


def make_fixture_board(
    path: Path,
    *,
    outline_mm=(50, 40),
    arc_corner=True,
    net_tie=False,
    copper_text=True,
    npth=True,
    locked_track=True,
    arc=True,
    half_track=False,
    zone=True,
    pads="all",
    free_via=True,
    transition_via=False,
    vcc_stub=False,
    wall=False,
    b_pads=False,
    pour_gap_pad=False,
    dangling_trace=False,
) -> Path:
    """Synthetic board. `b_pads`: SIG pads "2"/"3" are SMD on B.Cu (SIG then needs a via to reach
    the F.Cu pads). `pour_gap_pad`: GND SMD pad "9" on F.Cu at (10, 30) that the B.Cu GND pour
    cannot reach. `dangling_trace`: a hand-started SIG track from pad "5" (20, 28) up and to the right,
    ending at (22, 26.5) in the open (a dangling end heading ~37 degrees up; pad "2" is further up)."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = pcbnew.CreateEmptyBoard()
    b.SetCopperLayerCount(2)
    nets = {}
    for name in ("VCC", "SIG", "GND"):
        n = pcbnew.NETINFO_ITEM(b, name)
        b.Add(n)
        nets[name] = n

    W, H = outline_mm
    r = 5.0

    def edge(shape, a, bb, **kw):
        s = pcbnew.PCB_SHAPE(b)
        s.SetShape(shape)
        s.SetLayer(pcbnew.Edge_Cuts)
        s.SetWidth(int(0.1 * MM))
        if shape == pcbnew.SHAPE_T_ARC:
            s.SetCenter(_v(*kw["center"]))
            s.SetStart(_v(*a))
            s.SetArcAngleAndEnd(pcbnew.EDA_ANGLE(kw["angle"], pcbnew.DEGREES_T))
        else:
            s.SetStart(_v(*a))
            s.SetEnd(_v(*bb))
        if shape == pcbnew.SHAPE_T_BEZIER:
            s.SetBezierC1(_v(*kw["c1"]))
            s.SetBezierC2(_v(*kw["c2"]))
            s.RebuildBezierToSegmentsPointsList(int(0.005 * MM))
        b.Add(s)

    seg = pcbnew.SHAPE_T_SEGMENT
    edge(seg, (r if arc_corner else 0, 0), (W, 0))
    edge(seg, (W, 0), (W, H))
    # bottom edge is a (gently curved) Bezier
    edge(pcbnew.SHAPE_T_BEZIER, (W, H), (0, H), c1=(W * 2 / 3, H + 1.0), c2=(W / 3, H + 1.0))
    if arc_corner:
        edge(seg, (0, H), (0, r))
        # quarter arc from (0,r) to (r,0), centre (r,r)
        edge(pcbnew.SHAPE_T_ARC, (0, r), None, center=(r, r), angle=90.0)
    else:
        edge(seg, (0, H), (0, 0))

    if pads is not None:
        fp = pcbnew.FOOTPRINT(b)
        fp.SetReference("U1")
        fp.SetValue("U1")
        fp.SetPosition(_v(0, 0))
        b.Add(fp)
        spec = [
            ("1", pcbnew.PAD_SHAPE_CIRCLE, (1.6, 1.6), (20, 20), "VCC", dict(drill=(0.8, 0.8))),
            ("2", pcbnew.PAD_SHAPE_RECT, (1.0, 2.0), (25, 20), "SIG", {}),
            ("3", pcbnew.PAD_SHAPE_ROUNDRECT, (1.0, 2.0), (30, 20), "SIG", {}),
            ("4", pcbnew.PAD_SHAPE_OVAL, (1.0, 2.0), (35, 20), "GND", dict(drill=(0.6, 1.2))),
            ("5", pcbnew.PAD_SHAPE_TRAPEZOID, (1.0, 1.0), (20, 28), "SIG", {}),
            ("6", pcbnew.PAD_SHAPE_RECT, (1.0, 2.0), (35, 28), "VCC", dict(angle=30.0)),
        ]
        if pads != "all":
            spec = [s for s in spec if s[0] in set(pads)]
        for num, shp, size, pos, net, kw in spec:
            if b_pads and num in ("2", "3"):
                kw = {**kw, "back": True}
            p = _add_pad(fp, num, shp, size, pos, nets[net], **kw)
            if shp == pcbnew.PAD_SHAPE_ROUNDRECT:
                p.SetRoundRectRadiusRatio(0.25)  # 0.25 mm on the 1.0 mm short side
            if shp == pcbnew.PAD_SHAPE_TRAPEZOID:
                p.SetDelta(_v(0.0, 0.2))
        if pour_gap_pad:
            _add_pad(fp, "9", pcbnew.PAD_SHAPE_RECT, (1.0, 1.0), (10, 30), nets["GND"])
        if npth:
            p = pcbnew.PAD(fp)
            p.SetNumber("")
            p.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
            p.SetShape(pcbnew.PAD_SHAPE_CIRCLE)
            p.SetSize(_v(3.2, 3.2))
            p.SetDrillSize(_v(3.2, 3.2))
            p.SetLayerSet(p.UnplatedHoleMask())
            p.SetPosition(_v(45, 35))
            fp.Add(p)

    if net_tie:
        nt = pcbnew.FOOTPRINT(b)
        nt.SetReference("NT1")
        nt.SetValue("NT1")
        nt.SetPosition(_v(0, 0))
        nt.AddNetTiePadGroup("1,2")
        b.Add(nt)
        _add_pad(nt, "1", pcbnew.PAD_SHAPE_RECT, (1.0, 1.0), (10, 30), nets["VCC"])
        _add_pad(nt, "2", pcbnew.PAD_SHAPE_RECT, (1.0, 1.0), (10.5, 30), nets["SIG"])

    if copper_text:
        t = pcbnew.PCB_TEXT(b)
        t.SetText("TOPOR")
        t.SetLayer(pcbnew.F_Cu)
        t.SetPosition(_v(40, 30))
        b.Add(t)

    def track(a, c, net, locked=False, layer=pcbnew.F_Cu):
        t = pcbnew.PCB_TRACK(b)
        t.SetStart(_v(*a))
        t.SetEnd(_v(*c))
        t.SetWidth(int(0.25 * MM))
        t.SetLayer(layer)
        t.SetNet(nets[net])
        t.SetLocked(locked)
        b.Add(t)

    if locked_track:
        track((14, 10), (30, 10), "SIG", locked=True)
    if arc:
        a = pcbnew.PCB_ARC(b)
        a.SetStart(_v(10, 10))
        a.SetMid(_v(12, 11))
        a.SetEnd(_v(14, 10))
        a.SetWidth(int(0.25 * MM))
        a.SetLayer(pcbnew.F_Cu)
        a.SetNet(nets["SIG"])
        b.Add(a)
    if half_track:
        track((25, 20), (25, 28), "SIG")
    if dangling_trace:
        track((20, 28), (22, 26.5), "SIG")
    if vcc_stub:
        track((20, 20), (27, 24), "VCC", locked=True)
    if wall:
        # Locked GND wall across the board at y=24 on F.Cu and B.Cu: the 0.7 mm left to the outline
        # cannot take a 0.25 mm wire (0.2 + 0.25 + 0.5 = 0.95 mm), so pads above/below are walled off.
        wfp = pcbnew.FOOTPRINT(b)
        wfp.SetReference("W1")
        wfp.SetValue("W1")
        wfp.SetPosition(_v(0, 0))
        b.Add(wfp)
        for num, x in (("7", 1.2), ("8", 48.8)):
            _add_pad(wfp, num, pcbnew.PAD_SHAPE_CIRCLE, (1.0, 1.0), (x, 24), nets["GND"], drill=(0.5, 0.5))
        track((1.2, 24), (48.8, 24), "GND", locked=True)
        # M3: the board has two routable layers, so the wall closes B.Cu as well (the THT end
        # pads join both walls).
        track((1.2, 24), (48.8, 24), "GND", locked=True, layer=pcbnew.B_Cu)

    v = pcbnew.PCB_VIA(b)
    v.SetPosition(_v(5, 5))
    v.SetDrill(int(0.3 * MM))
    v.SetWidth(int(0.6 * MM))
    v.SetNet(nets["GND"])
    if free_via:
        v.SetIsFree(True)
    b.Add(v)

    if transition_via:
        # SIG layer change at (40,10): F.Cu track, via (not free), B.Cu track
        track((35, 10), (40, 10), "SIG")
        v2 = pcbnew.PCB_VIA(b)
        v2.SetPosition(_v(40, 10))
        v2.SetDrill(int(0.3 * MM))
        v2.SetWidth(int(0.6 * MM))
        v2.SetNet(nets["SIG"])
        b.Add(v2)
        t2 = pcbnew.PCB_TRACK(b)
        t2.SetStart(_v(40, 10))
        t2.SetEnd(_v(45, 10))
        t2.SetWidth(int(0.25 * MM))
        t2.SetLayer(pcbnew.B_Cu)
        t2.SetNet(nets["SIG"])
        b.Add(t2)

    if zone:
        z = pcbnew.ZONE(b)
        z.SetLayer(pcbnew.B_Cu)
        z.SetNet(nets["GND"])
        ol = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in ((1, 1), (W - 1, 1), (W - 1, H - 1), (1, H - 1)):
            ol.Append(_v(x, y))
        ol.SetClosed(True)
        z.AddPolygon(ol)
        b.Add(z)
        pcbnew.ZONE_FILLER(b).Fill(b.Zones())

    pcbnew.SaveBoard(str(path), b)
    return path


def add_teardrop_zone(pcb: Path, net: str, layer: str, poly_mm) -> None:
    """Append a KiCad-style teardrop zone (attr teardrop padvia) to a saved board file (text edit:
    pcbnew's python API does not expose the teardrop type enum)."""
    pcb = Path(pcb)
    text = pcb.read_text()
    pts = " ".join(f"(xy {x} {y})" for x, y in poly_mm)
    zone = (f'(zone (net "{net}") (layer "{layer}") (hatch none 0.1) (priority 30000) '
            f'(attr (teardrop (type padvia))) (connect_pads yes (clearance 0)) (min_thickness 0.0254) '
            f'(fill yes (island_removal_mode 1)) (polygon (pts {pts})))')
    i = text.rstrip().rfind(")")
    pcb.write_text(text[:i] + zone + "\n" + text[i:])


_DEF_RULES = {
    "min_clearance": 0.2,
    "min_track_width": 0.2,
    "min_copper_edge_clearance": 0.5,
    "min_hole_clearance": 0.25,
    "min_hole_to_hole": 0.25,
    "min_via_diameter": 0.4,
    "min_through_hole_diameter": 0.3,
    "min_via_annular_width": 0.05,
    # every Board Setup constraint KiCad writes (a key the file lacks takes topor's reported rule fallback)
    "min_connection": 0.0,
    "max_error": 0.005,
}
_DEF_CLASSES = [
    {"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.6,
     "via_drill": 0.3, "priority": 2147483647}
]


def make_pair_via_board(path: Path, *, wide: bool = True) -> Path:
    """Pair-via room fixture (pair-via site proposal 2026-11-13): a 20 x 10 mm 2-layer board, the diff pair
    USB+ / USB- from J1 (SMD pads on F.Cu at x 1) to J2 (SMD pads on B.Cu at x 19), so the pair changes layer once,
    in a channel between two GND copper walls on both layers (filled copper rectangles of net GND). The channel
    (y 5.275 +- 0.62 mm) holds the pair band (2 x 0.2 mm tracks at a 0.2 mm gap, 0.2 mm clearance) everywhere but
    not the two-via disc of the smallest via (0.4 / 0.3 mm, via gap 0.2: a 1.0 mm disc + 0.2 mm clearance); with
    ``wide`` the channel widens to +- 1.6 mm for x 5 .. 9 mm, the one place that holds the disc. Use with
    ``write_project_file(.., classes=PAIR_VIA_CLASSES)``."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = pcbnew.CreateEmptyBoard()
    b.SetCopperLayerCount(2)
    nets = {}
    for name in ("USB+", "USB-", "GND"):
        n = pcbnew.NETINFO_ITEM(b, name)
        b.Add(n)
        nets[name] = n
    W, H = 20.0, 10.0
    for a, c in (((0, 0), (W, 0)), ((W, 0), (W, H)), ((W, H), (0, H)), ((0, H), (0, 0))):
        s = pcbnew.PCB_SHAPE(b)
        s.SetShape(pcbnew.SHAPE_T_SEGMENT)
        s.SetLayer(pcbnew.Edge_Cuts)
        s.SetWidth(int(0.05 * MM))
        s.SetStart(_v(*a))
        s.SetEnd(_v(*c))
        b.Add(s)
    for ref, x, back in (("J1", 1.0, False), ("J2", 19.0, True)):
        fp = pcbnew.FOOTPRINT(b)
        fp.SetReference(ref)
        fp.SetValue(ref)
        fp.SetPosition(_v(0, 0))
        b.Add(fp)
        _add_pad(fp, "1", pcbnew.PAD_SHAPE_RECT, (0.25, 0.25), (x, 5.0), nets["USB+"], back=back)
        _add_pad(fp, "2", pcbnew.PAD_SHAPE_RECT, (0.25, 0.25), (x, 5.55), nets["USB-"], back=back)
    yc, h, hw = 5.275, 0.62, 1.6
    parts = [(0.6, 5.0, h), (5.0, 9.0, hw), (9.0, W - 0.6, h)] if wide else [(0.6, W - 0.6, h)]
    for x0, x1, hh in parts:
        for y0, y1 in ((0.6, yc - hh), (yc + hh, H - 0.6)):
            for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
                s = pcbnew.PCB_SHAPE(b)
                s.SetShape(pcbnew.SHAPE_T_RECT)
                s.SetLayer(layer)
                s.SetFilled(True)
                s.SetWidth(0)
                s.SetStart(_v(x0, y0))
                s.SetEnd(_v(x1, y1))
                s.SetNet(nets["GND"])
                b.Add(s)
    pcbnew.SaveBoard(str(path), b)
    return path


#: Net class of the pair-via fixture: 0.2 mm tracks and clearance, diff pair 0.2 / 0.2 mm, via gap 0.2 mm.
PAIR_VIA_CLASSES = [
    {"name": "Default", "clearance": 0.2, "track_width": 0.2, "via_diameter": 0.6, "via_drill": 0.3,
     "diff_pair_width": 0.2, "diff_pair_gap": 0.2, "diff_pair_via_gap": 0.2, "priority": 2147483647}
]


def write_project_file(pcb: Path, *, rules: dict, classes: list[dict], patterns=()) -> Path:
    pcb = Path(pcb)
    merged = {**_DEF_RULES, **rules}
    cls = list(classes) if classes else [dict(c) for c in _DEF_CLASSES]
    doc = {
        "board": {"design_settings": {"rules": merged}},
        "net_settings": {
            "classes": cls,
            "meta": {"version": 3},
            "netclass_assignments": {},
            "netclass_patterns": [dict(p) for p in patterns],
        },
        "meta": {"filename": pcb.with_suffix(".kicad_pro").name, "version": 3},
    }
    out = pcb.with_suffix(".kicad_pro")
    out.write_text(json.dumps(doc, indent=2))
    return out
