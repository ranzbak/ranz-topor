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
    fp_clearance_mm=None,
    pad_clearance_mm=None,
    shield_pads=0,
) -> Path:
    """Synthetic board. `b_pads`: SIG pads "2"/"3" are SMD on B.Cu (SIG then needs a via to reach
    the F.Cu pads). `pour_gap_pad`: GND SMD pad "9" on F.Cu at (10, 30) that the B.Cu GND pour
    cannot reach. `dangling_trace`: a hand-started SIG track from pad "5" (20, 28) up and to the right,
    ending at (22, 26.5) in the open (a dangling end heading ~37 degrees up; pad "2" is further up).
    `shield_pads` (pour-pad-join): a connector J1 with that many THT GND pads all numbered "SH" (shield /
    mounting pads), 1.6 mm round, 0.8 mm drill, at (8 + 8 k, 34) mm, inside the B.Cu GND pour (`zone`)."""
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
        if fp_clearance_mm is not None:
            fp.SetLocalClearance(int(round(fp_clearance_mm * 1e6)))  # footprint-level override (KiCad inherits it into every pad without its own)
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
            if pad_clearance_mm is not None and num == "3":
                p.SetLocalClearance(int(round(pad_clearance_mm * 1e6)))  # pad override wins over the footprint's
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

    if shield_pads:
        jf = pcbnew.FOOTPRINT(b)
        jf.SetReference("J1")
        jf.SetValue("J1")
        jf.SetPosition(_v(0, 0))
        b.Add(jf)
        for k in range(shield_pads):
            _add_pad(jf, "SH", pcbnew.PAD_SHAPE_CIRCLE, (1.6, 1.6), (8 + 8 * k, 34), nets["GND"], drill=(0.8, 0.8))

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


def make_dense_board(path: Path, *, n_pairs: int = 90, seed: int = 1, pitch_mm: float = 1.0, reach: float = 5.0) -> Path:
    """limit-p3p4: a dense 2-layer board whose layer assignment and rip-up (P3/P4) take far longer than its
    topological search (P2): `n_pairs` two-pad nets N1.. on a `pitch_mm` grid of 0.6 mm round SMD pads, each pad paired
    with a seeded random free cell within `reach` cells; every second net ends on a B.Cu pad (a via each). The pairs
    cross a lot (the same model as engine/tests/topo_limit_p3p4.rs `dense`). Outline: the pad bbox plus 2 mm.
    No zones, no other copper. Pair with ``write_project_file(rules={}, classes=DENSE_CLASSES)``."""
    import math
    import random

    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = pcbnew.CreateEmptyBoard()
    b.SetCopperLayerCount(2)
    cols = int(math.sqrt(2 * n_pairs)) + 1
    cells = [(i % cols, i // cols) for i in range(2 * n_pairs)]
    rng = random.Random(seed)
    order = list(range(len(cells)))
    rng.shuffle(order)
    free, pairs = set(order), []
    for i in order:
        if i not in free:
            continue
        free.discard(i)
        near = sorted(j for j in free if math.hypot(cells[i][0] - cells[j][0], cells[i][1] - cells[j][1]) <= reach)
        if not near:
            continue
        j = rng.choice(near)
        free.discard(j)
        pairs.append((i, j))
    for k, (i, j) in enumerate(pairs, start=1):
        net = pcbnew.NETINFO_ITEM(b, f"N{k}")
        b.Add(net)
        fp = pcbnew.FOOTPRINT(b)
        fp.SetReference(f"P{k}")
        fp.SetValue(f"P{k}")
        fp.SetPosition(_v(0, 0))
        b.Add(fp)
        for num, c in (("1", i), ("2", j)):
            _add_pad(fp, num, pcbnew.PAD_SHAPE_CIRCLE, (0.6, 0.6), (5 + cells[c][0] * pitch_mm, 5 + cells[c][1] * pitch_mm), net,
                     back=(num == "2" and k % 2 == 0))
    w = 5 + (cols - 1) * pitch_mm + 3
    h = 5 + (len(cells) // cols) * pitch_mm + 3
    for a, c in (((2, 2), (w, 2)), ((w, 2), (w, h)), ((w, h), (2, h)), ((2, h), (2, 2))):
        e = pcbnew.PCB_SHAPE(b)
        e.SetShape(pcbnew.SHAPE_T_SEGMENT)
        e.SetLayer(pcbnew.Edge_Cuts)
        e.SetWidth(int(0.1 * MM))
        e.SetStart(_v(*a))
        e.SetEnd(_v(*c))
        b.Add(e)
    pcbnew.SaveBoard(str(path), b)
    return path


#: limit-p3p4: the dense board's net class (thin tracks, small vias: the grid is tight on purpose).
DENSE_CLASSES = [
    {"name": "Default", "clearance": 0.15, "track_width": 0.15, "via_diameter": 0.5,
     "via_drill": 0.25, "priority": 2147483647}
]


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
