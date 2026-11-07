"""Mechanism fixtures: tiny synthetic boards, one per routing mechanism a ladder board showed (D6a).

Each `fx_*` function writes a board (and its .kicad_pro) under a caller-supplied directory and
returns its path; `tests/test_mechfix.py` routes it end to end (tools/route_board.sh: strip, route,
KiCad DRC with refill) and checks the result. Coordinates in mm, KiCad frame (y down).
"""
from __future__ import annotations

from pathlib import Path

import pcbnew

from harness.fixtures import write_project_file

MM = 1_000_000


def _v(x, y):
    return pcbnew.VECTOR2I(int(round(x * MM)), int(round(y * MM)))


class Fx:
    """A rectangular 2-layer board under construction (outline `w` x `h` mm at the origin)."""

    def __init__(self, w: float, h: float, nets=("GND", "SIG"), layers: int = 2):
        # Fixed item uuids: the exports sort by uuid, so a fixture is the same board every run.
        pcbnew.KIID.SeedGenerator(1)
        self.b = pcbnew.CreateEmptyBoard()
        self.b.SetCopperLayerCount(layers)
        self.nets = {}
        for name in nets:
            n = pcbnew.NETINFO_ITEM(self.b, name)
            self.b.Add(n)
            self.nets[name] = n
        for a, c in (((0, 0), (w, 0)), ((w, 0), (w, h)), ((w, h), (0, h)), ((0, h), (0, 0))):
            s = pcbnew.PCB_SHAPE(self.b)
            s.SetShape(pcbnew.SHAPE_T_SEGMENT)
            s.SetLayer(pcbnew.Edge_Cuts)
            s.SetWidth(int(0.1 * MM))
            s.SetStart(_v(*a))
            s.SetEnd(_v(*c))
            self.b.Add(s)
        self.w, self.h = w, h

    def pad(self, ref: str, net: str | None, pos, size=(1.0, 1.0), *, layer=pcbnew.F_Cu, drill=None,
            shape=pcbnew.PAD_SHAPE_RECT, num="1"):
        """A one-pad footprint `ref` (SMD on `layer`, or PTH when `drill` is given)."""
        fp = pcbnew.FOOTPRINT(self.b)
        fp.SetReference(ref)
        fp.SetValue(ref)
        fp.SetPosition(_v(*pos))
        self.b.Add(fp)
        p = pcbnew.PAD(fp)
        p.SetNumber(num)
        if drill is None:
            p.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
            ls = pcbnew.LSET()
            back = layer == pcbnew.B_Cu
            for l in (layer, pcbnew.B_Mask if back else pcbnew.F_Mask, pcbnew.B_Paste if back else pcbnew.F_Paste):
                ls.AddLayer(l)
            p.SetLayerSet(ls)
        else:
            p.SetAttribute(pcbnew.PAD_ATTRIB_PTH)
            p.SetLayerSet(p.PTHMask())
            p.SetDrillSize(_v(drill, drill))
        p.SetShape(shape)
        p.SetSize(_v(*size))
        p.SetPosition(_v(*pos))
        if net is not None:
            p.SetNet(self.nets[net])
        fp.Add(p)
        return fp

    def zone(self, net: str, layer, poly=None, *, min_thickness=0.25, clearance=0.2):
        """A copper pour of `net` on `layer` (default: the board inset by 0.5 mm)."""
        poly = poly or [(0.5, 0.5), (self.w - 0.5, 0.5), (self.w - 0.5, self.h - 0.5), (0.5, self.h - 0.5)]
        z = pcbnew.ZONE(self.b)
        z.SetLayer(layer)
        z.SetNet(self.nets[net])
        z.SetMinThickness(int(min_thickness * MM))
        z.SetLocalClearance(int(clearance * MM))
        ol = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in poly:
            ol.Append(_v(x, y))
        ol.SetClosed(True)
        z.AddPolygon(ol)
        self.b.Add(z)
        return z

    def shape(self, layer, poly, net: str | None = None):
        """A filled copper polygon (gr_poly) on `layer`."""
        sh = pcbnew.PCB_SHAPE(self.b)
        sh.SetShape(pcbnew.SHAPE_T_POLY)
        sh.SetLayer(layer)
        sh.SetFilled(True)
        sh.SetWidth(0)
        ch = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in poly:
            ch.Append(_v(x, y))
        ch.SetClosed(True)
        sh.SetPolyShape(pcbnew.SHAPE_POLY_SET(ch))
        if net is not None:
            sh.SetNet(self.nets[net])
        self.b.Add(sh)
        return sh

    def keepout(self, layers, poly, *, tracks=False, vias=False, pour=False, pads=False, footprints=False):
        """A rule area (keepout) on the copper `layers` (pcbnew layer ids) forbidding the listed items."""
        z = pcbnew.ZONE(self.b)
        z.SetIsRuleArea(True)
        ls = pcbnew.LSET()
        for l in layers:
            ls.AddLayer(l)
        z.SetLayerSet(ls)
        z.SetDoNotAllowTracks(tracks)
        z.SetDoNotAllowVias(vias)
        z.SetDoNotAllowZoneFills(pour)
        z.SetDoNotAllowPads(pads)
        z.SetDoNotAllowFootprints(footprints)
        ol = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in poly:
            ol.Append(_v(x, y))
        ol.SetClosed(True)
        z.AddPolygon(ol)
        self.b.Add(z)
        return z

    def save(self, path: Path, *, rules: dict | None = None, classes=()) -> Path:
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        if any(True for _ in self.b.Zones()):
            pcbnew.ZONE_FILLER(self.b).Fill(self.b.Zones())
        pcbnew.SaveBoard(str(path), self.b)
        write_project_file(path, rules=rules or {}, classes=list(classes))
        return path


def fx_plane_stub_squeeze(d: Path) -> Path:
    """A signal past a plane pad: the pad's stub via must not take the signal's corridor
    (wiccon-2023 GPIO0 FLASH1.1 -> TP3.1 vs the MODE1.2 GND stub at (109.89, 120.61) mm, D5).

    GND pad G (F.Cu, B.Cu GND pour: it needs a stub and a via) at (10, 10); a 1.1 mm corridor
    between G and a foreign pad K east of it; SIG runs from (11.05, 4) to (11.05, 16) through that
    corridor, the straight line. The stub via may go anywhere around G but into the corridor."""
    f = Fx(22, 20, nets=("GND", "SIG", "K"))
    f.pad("G1", "GND", (10, 10), (1.0, 1.0))
    f.pad("K1", "K", (12.4, 10), (1.6, 6.0))
    f.pad("K2", "K", (18.0, 10), (1.0, 1.0))
    f.pad("S1", "SIG", (11.05, 4), (1.0, 1.0))
    f.pad("S2", "SIG", (11.05, 16), (1.0, 1.0))
    f.zone("GND", pcbnew.B_Cu)
    return f.save(Path(d) / "fx_plane_stub_squeeze.kicad_pcb")


def fx_two_sided_overlap(d: Path) -> Path:
    """A back-side net whose middle pad lies under a front-side pad of another net: the route on
    to the third pad leaves from the near pad R, through R if need be, not from the far pad L
    (wiccon-2023 RST: RESET1.1 L (108.53, 117.14) / R (115.37, 117.14) on B.Cu under MODE1.1 GPIO04
    (114.44, 117.24) on F.Cu, on to U1.1 (135.25, 98.55) on B.Cu; D5 left from L and ran past R).

    SIG: L (8.53, 17.14) and R (15.37, 17.14), 2.6 x 1.8 mm on B.Cu, and U (35.25, 3.55) on B.Cu.
    K: M (14.44, 17.24) 2.4 x 1.4 mm at 45 degrees on F.Cu over R, N (9.5, 12.5) and P (22, 6) on F.Cu."""
    f = Fx(40, 25, nets=("SIG", "K"))
    f.pad("L1", "SIG", (8.53, 17.14), (2.6, 1.8), layer=pcbnew.B_Cu)
    f.pad("R1", "SIG", (15.37, 17.14), (2.6, 1.8), layer=pcbnew.B_Cu)
    f.pad("U1", "SIG", (35.25, 3.55), (1.0, 1.0), layer=pcbnew.B_Cu)
    m = f.pad("M1", "K", (14.44, 17.24), (2.4, 1.4), shape=pcbnew.PAD_SHAPE_ROUNDRECT)
    for p in m.Pads():
        p.SetOrientation(pcbnew.EDA_ANGLE(45.0, pcbnew.DEGREES_T))
    f.pad("N1", "K", (9.5, 12.5), (1.0, 1.0))
    f.pad("P1", "K", (22.0, 6.0), (1.0, 1.0))
    return f.save(Path(d) / "fx_two_sided_overlap.kicad_pcb")


def fx_copper_graphic_corridor(d: Path) -> Path:
    """A copper graphic (gr_poly, no net) across the straight line of a two-pad net: the planner
    routes around it, the drawing does not discover it (dcsgonebadge D5 "clearance vs shape"
    failures, 1 -> 6: board.rs had no obstacles for copper graphics).

    SIG: (6, 10) and (24, 10) SMD on F.Cu; a 2 x 6 mm copper rectangle on F.Cu at x 14..16,
    y 7..13."""
    f = Fx(30, 20, nets=("SIG",))
    f.pad("A1", "SIG", (6, 10), (1.0, 1.0))
    f.pad("B1", "SIG", (24, 10), (1.0, 1.0))
    f.shape(pcbnew.F_Cu, [(14, 7), (16, 7), (16, 13), (14, 13)])
    return f.save(Path(d) / "fx_copper_graphic_corridor.kicad_pcb")


def _crossing(f: Fx) -> None:
    """Two nets that must cross (each runs from board edge to board edge on F.Cu): one of them
    needs two vias near the crossing at (15, 10)."""
    f.pad("A1", "A", (1.5, 10), (1.0, 1.0))
    f.pad("A2", "A", (28.5, 10), (1.0, 1.0))
    f.pad("B1", "B", (15, 1.5), (1.0, 1.0))
    f.pad("B2", "B", (15, 18.5), (1.0, 1.0))
    # Walls of foreign pads along the edges, so neither net can go round the other's ends.
    # 1.2 mm gaps at the terminals: room for one wire leaving on F.Cu.
    for i, (x, y) in enumerate([(1.5, 5.9), (1.5, 14.1), (28.5, 5.9), (28.5, 14.1), (10.9, 1.5), (19.1, 1.5), (10.9, 18.5), (19.1, 18.5)]):
        f.pad(f"W{i}", None, (x, y), (2.2, 7.0) if x in (1.5, 28.5) else (7.0, 2.2), layer=pcbnew.F_Cu, drill=None)


def fx_keepout_no_via_site(d: Path) -> Path:
    """No-via keepouts (both layers) where the natural via sites are: two nets that must cross
    at (15, 10); one of them changes layer twice, and the planner puts those vias right next to
    its pads (HEAD 2bfab0b). Keepouts 4.4 x 6 mm in front of every terminal; tracks may pass,
    vias may not (D6a item 2b case b)."""
    f = Fx(30, 20, nets=("A", "B"))
    _crossing(f)
    for poly in ([(2.6, 7), (7, 7), (7, 13), (2.6, 13)], [(23, 7), (27.4, 7), (27.4, 13), (23, 13)],
                 [(12, 2.6), (18, 2.6), (18, 7), (12, 7)], [(12, 13), (18, 13), (18, 17.4), (12, 17.4)]):
        f.keepout([pcbnew.F_Cu, pcbnew.B_Cu], poly, vias=True)
    return f.save(Path(d) / "fx_keepout_no_via_site.kicad_pcb")



def fx_keepout_no_via_inner(d: Path) -> Path:
    """A no-via keepout on In1.Cu only, on a 4-layer board: a through via spans In1, so it may not
    stand there either (D6a item 2b case h). The crossing of fx_keepout_no_via_site, keepouts on
    In1.Cu alone in front of every terminal."""
    f = Fx(30, 20, nets=("A", "B"), layers=4)
    _crossing(f)
    for poly in ([(2.6, 7), (7, 7), (7, 13), (2.6, 13)], [(23, 7), (27.4, 7), (27.4, 13), (23, 13)],
                 [(12, 2.6), (18, 2.6), (18, 7), (12, 7)], [(12, 13), (18, 13), (18, 17.4), (12, 17.4)]):
        f.keepout([pcbnew.In1_Cu], poly, vias=True)
    return f.save(Path(d) / "fx_keepout_no_via_inner.kicad_pcb")


def fx_pinned_crossing(d: Path) -> Path:
    """Two F.Cu nets that must cross in a cross-shaped corridor whose middle a foreign B.Cu pad
    covers: under it both wires are held to F.Cu, so they may not cross there (D6a layer pin:
    dcsgonebadge 8 "crossing ... cannot be layered: both stretches are fixed to the same layer",
    all inside the fan of a foreign pad on the other side, e.g. I2C_SDA x I2C_SCL over D7.3).

    A (2, 10) - (28, 10) and B (15.3, 2) - (15.3, 18) on F.Cu; walls (both layers, no net) leave a
    corridor y 7..13 / x 12..18.5 with end caps; K: a 4 x 4 mm B.Cu pad at (15.6, 10.2)."""
    f = Fx(30, 20, nets=("A", "B", "K"))
    f.pad("A1", "A", (2.0, 10), (1.0, 1.0))
    f.pad("A2", "A", (28.0, 10), (1.0, 1.0))
    f.pad("B1", "B", (15.3, 2.0), (1.0, 1.0))
    f.pad("B2", "B", (15.3, 18.0), (1.0, 1.0))
    walls = [(6.25, 3.75, 11.5, 6.5), (24.0, 3.75, 11.0, 6.5), (6.25, 16.25, 11.5, 6.5), (24.0, 16.25, 11.0, 6.5),
             (15.25, 0.85, 6.5, 0.7), (15.25, 19.15, 6.5, 0.7), (0.85, 10.0, 0.7, 6.0), (29.15, 10.0, 0.7, 6.0)]
    for i, (x, y, w, h) in enumerate(walls):
        for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
            f.pad(f"W{i}{'F' if layer == pcbnew.F_Cu else 'B'}", None, (x, y), (w, h), layer=layer)
    f.pad("K1", "K", (15.6, 10.2), (4.0, 4.0), layer=pcbnew.B_Cu)
    f.pad("K2", "K", (25.0, 10.0), (1.0, 1.0), layer=pcbnew.B_Cu)
    return f.save(Path(d) / "fx_pinned_crossing.kicad_pcb")


def fx_local_pour_far_pads(d: Path) -> Path:
    """A small local pour around one pad and a row of same-net pads outside it (qmtech
    /HDMI transmitter/1V8, D6a: a 6 x 6 mm F.Cu-only pour around U8; C7.1 and L1..L4.2 outside
    it each got a plane stub and a via that reaches no 1V8 copper on any layer: 5 open pads).

    P: U1 (8, 10) inside an F.Cu pour of P (5..11 x 7..13); C1 (8, 14.5) just below the pour;
    L1..L3 (18, 15), (22, 15), (26, 15) far from it. All pads SMD on F.Cu."""
    f = Fx(32, 20, nets=("P",))
    f.pad("U1", "P", (8, 10), (1.0, 1.0))
    f.pad("C1", "P", (8, 14.5), (1.0, 1.0))
    for i, x in enumerate((18, 22, 26)):
        f.pad(f"L{i + 1}", "P", (x, 15), (1.0, 1.0))
    f.zone("P", pcbnew.F_Cu, [(5, 7), (11, 7), (11, 13), (5, 13)])
    return f.save(Path(d) / "fx_local_pour_far_pads.kicad_pcb")


def fx_same_layer_pour_stub(d: Path) -> Path:
    """A B.Cu pad of a pour net next to (not in) that net's B.Cu pour, with no via site anywhere
    (a no-via keepout over the board): a track on B.Cu from the pad into the fill joins it
    (dcsgonebadge J1.A1, D6b: no via site inside the fill, so a signal connection D11.3 -> J1.A1,
    16.4 mm with a via, ran on F.Cu inside the GND pour along J1.S1 and left a 0.096 mm neck).

    GND: P (3, 10) 0.6 x 1.4 mm on B.Cu; Q (15, 10) inside a B.Cu GND pour 4.5..19 x 1..19."""
    f = Fx(20, 20, nets=("GND",))
    f.pad("P", "GND", (3.0, 10.0), (0.6, 1.4), layer=pcbnew.B_Cu)
    f.pad("Q", "GND", (15.0, 10.0), (1.0, 1.0), layer=pcbnew.B_Cu)
    f.zone("GND", pcbnew.B_Cu, [(4.5, 1), (19, 1), (19, 19), (4.5, 19)])
    f.keepout([pcbnew.F_Cu, pcbnew.B_Cu], [(0.2, 0.2), (19.8, 0.2), (19.8, 19.8), (0.2, 19.8)], vias=True)
    return f.save(Path(d) / "fx_same_layer_pour_stub.kicad_pcb")


def fx_coincident_pad_stub(d: Path) -> Path:
    """Two GND pads drawn on top of each other (a USB-C GND pin pair, dcsgonebadge J1.A1/J1.B12)
    outside the B.Cu GND pour: the plane stub and via of one starts inside the other, which is
    the same copper (D6b: every stub was refused as grazing another pad of its net, so J1.A1 became
    a 16 mm signal connection that ran inside the F.Cu pour and left a 0.096 mm neck).

    GND: P1 and P2 0.6 x 1.4 mm on F.Cu at (5, 10); Q (15, 10) on B.Cu inside a B.Cu GND pour
    9..19 x 1..19 (its edge 3.7 mm from the pads: the via site is inside the pour)."""
    f = Fx(20, 20, nets=("GND",))
    f.pad("P1", "GND", (5.0, 10.0), (0.6, 1.4))
    f.pad("P2", "GND", (5.0, 10.0), (0.6, 1.4))
    f.pad("Q", "GND", (15.0, 10.0), (1.0, 1.0), layer=pcbnew.B_Cu)
    f.zone("GND", pcbnew.B_Cu, [(6.5, 1), (19, 1), (19, 19), (6.5, 19)])
    return f.save(Path(d) / "fx_coincident_pad_stub.kicad_pcb", rules={"min_connection": 0.127})


def fx_corner_wrap(d: Path) -> Path:
    """A signal that bends around the corners of a field of foreign pads: the drawing's exact
    path repairs the funnel stretch by stretch, and two repairs that wrap one corner from either
    side of a funnel vertex left that vertex as an out-and-back spike, a segment and its exact
    reverse (D6b: wiccon 12, dcsgonebadge 14, qmtech 15, e.g. wiccon SCLK at (127.02, 142.00) B.Cu).

    SIG: (2.5, 3) and (27.5, 17) on F.Cu; K: eight F.Cu pads between them."""
    f = Fx(30, 20, nets=("SIG", "K"))
    f.pad("A1", "SIG", (2.5, 3), (1.0, 1.0))
    f.pad("B1", "SIG", (27.5, 17), (1.0, 1.0))
    ks = [((23.21, 16.27), (0.92, 0.99)), ((21.04, 13.3), (2.27, 1.48)), ((16.91, 11.5), (2.08, 1.15)),
          ((13.75, 8.51), (2.39, 2.99)), ((23.09, 10.62), (1.78, 1.39)), ((6.65, 3.38), (1.82, 1.5)),
          ((12.84, 15.49), (1.96, 2.03)), ((10.25, 3.33), (1.52, 1.1))]
    for i, (pos, size) in enumerate(ks):
        f.pad(f"K{i}", "K", pos, size)
    return f.save(Path(d) / "fx_corner_wrap.kicad_pcb")


def fx_narrow_gap_forced(d: Path) -> Path:
    """Never drop (M-B1): a signal whose only way is a gap narrower than its track plus the two
    clearances, on both layers. The drawing finds no legal geometry; the connection is drawn
    anyway through the gap, its two clearance violations (one per wall pad) priced and reported
    (before M-B1 it was dropped: "drawing: clearance gap ... against pad:W..").

    SIG: A (3, 10) and B (27, 10) SMD on F.Cu; walls of unconnected pads on F.Cu and B.Cu across
    the board at x = 15 (1 mm wide, from the board edge to y 9.75 and from y 10.25 to the edge):
    a 0.5 mm gap where the 0.25 mm track needs 0.65 mm (0.2 mm clearance each side)."""
    f = Fx(30, 20, nets=("SIG",))
    f.pad("A1", "SIG", (3.0, 10.0), (1.0, 1.0))
    f.pad("B1", "SIG", (27.0, 10.0), (1.0, 1.0))
    for layer, t in ((pcbnew.F_Cu, "F"), (pcbnew.B_Cu, "B")):
        f.pad(f"WN{t}", None, (15.0, 4.875), (1.0, 9.75), layer=layer)
        f.pad(f"WS{t}", None, (15.0, 15.125), (1.0, 9.75), layer=layer)
    return f.save(Path(d) / "fx_narrow_gap_forced.kicad_pcb")


def _terminal_clinch(f: Fx) -> None:
    """B's straight line passes between A's terminal A1 and A's other end A2 (A1 sits 1 mm south
    of B's line): B, the shorter connection, is routed first and wraps A1 on the north, so A has
    to cross it right at its terminal (Luzin & Popov 2012 sign 1, an ordinary clinch; the
    half-loop around a header pin of qmtech ADV_D -> U4). All four pads SMD on F.Cu."""
    f.pad("A1", "A", (10.0, 9.2), (1.0, 1.0))
    f.pad("A2", "A", (10.0, 2.0), (1.0, 1.0))
    f.pad("B1", "B", (7.5, 8.2), (1.0, 1.0))
    f.pad("B2", "B", (12.5, 8.2), (1.0, 1.0))


def fx_terminal_clinch(d: Path) -> Path:
    """Clinch at a terminal (M-B2 crossing rules): routed in P2's order, A crosses B next to A1
    and one of them takes a via pair; the rule routes A first and lays B on the other (south)
    side of A1: no crossing, no via (Luzin & Popov 2012, p. 118, ordinary clinch removal)."""
    f = Fx(20, 14, nets=("A", "B"))
    _terminal_clinch(f)
    return f.save(Path(d) / "fx_terminal_clinch.kicad_pcb")


def fx_terminal_clinch_no_via(d: Path) -> Path:
    """fx_terminal_clinch where A cannot go around B: walls of unconnected pads (both layers)
    beside B1 and B2 from the top edge down to y 8.9 leave A's only way north across B's line,
    and a no-via keepout (both layers) over the whole area leaves the crossing no via site. Before
    the crossing rules A crosses B on one layer and is drawn through it (never drop: a short);
    with them B passes A1 on the south and nothing crosses."""
    f = Fx(20, 14, nets=("A", "B"))
    _terminal_clinch(f)
    for i, x in enumerate((3.45, 16.55)):
        for layer, t in ((pcbnew.F_Cu, "F"), (pcbnew.B_Cu, "B")):
            f.pad(f"W{i}{t}", None, (x, 4.7), (5.9, 8.4), layer=layer)
    f.keepout([pcbnew.F_Cu, pcbnew.B_Cu], [(5.5, 3.5), (14.5, 3.5), (14.5, 12.5), (5.5, 12.5)], vias=True)
    return f.save(Path(d) / "fx_terminal_clinch_no_via.kicad_pcb")
