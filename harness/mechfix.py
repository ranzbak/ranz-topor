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

    def save(self, path: Path, *, rules: dict | None = None, classes=(), patterns=(), fill: bool = True) -> Path:
        """Write the board (zones filled unless `fill` is false: then they keep whatever fill they have)."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        if fill and any(True for _ in self.b.Zones()):
            pcbnew.ZONE_FILLER(self.b).Fill(self.b.Zones())
        pcbnew.SaveBoard(str(path), self.b)
        write_project_file(path, rules=rules or {}, classes=list(classes), patterns=patterns)
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
    routes around it, the drawing does not discover it (a 2-layer badge, D5 "clearance vs shape"
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
    a 2-layer badge: 8 "crossing ... cannot be layered: both stretches are fixed to the same layer",
    all inside the fan of a foreign pad on the other side, e.g. I2C_SDA x I2C_SCL over an LED pad).

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
    (a USB-C GND pin, D6b: no via site inside the fill, so a signal connection from an LED pad to it,
    16.4 mm with a via, ran on F.Cu inside the GND pour along the shield pad and left a 0.096 mm neck).

    GND: P (3, 10) 0.6 x 1.4 mm on B.Cu; Q (15, 10) inside a B.Cu GND pour 4.5..19 x 1..19."""
    f = Fx(20, 20, nets=("GND",))
    f.pad("P", "GND", (3.0, 10.0), (0.6, 1.4), layer=pcbnew.B_Cu)
    f.pad("Q", "GND", (15.0, 10.0), (1.0, 1.0), layer=pcbnew.B_Cu)
    f.zone("GND", pcbnew.B_Cu, [(4.5, 1), (19, 1), (19, 19), (4.5, 19)])
    f.keepout([pcbnew.F_Cu, pcbnew.B_Cu], [(0.2, 0.2), (19.8, 0.2), (19.8, 19.8), (0.2, 19.8)], vias=True)
    return f.save(Path(d) / "fx_same_layer_pour_stub.kicad_pcb")


def fx_coincident_pad_stub(d: Path) -> Path:
    """Two GND pads drawn on top of each other (a USB-C GND pin pair, A1/B12)
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
    reverse (D6b: wiccon 12, a 2-layer badge 14, qmtech 15, e.g. wiccon SCLK at (127.02, 142.00) B.Cu).

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


def fx_neck_bottleneck(d: Path) -> Path:
    """Width in P2 (M-C item 1): a wide net (0.4 mm track, 0.2 mm clearance, board minimum track
    0.2 mm) whose only way is a 0.7 mm gap between wall pads (both layers) at x = 15: 0.8 mm at
    the nominal width, 0.6 mm necked. The plan necks the stretch from the nearest pad end to the
    gap (TopoR 6.3 manual p. 56: the minimum width in bottlenecks; user rule: a neck starts at the
    footprint) and the drawing draws it so; before M-C the planner counted the full width, the
    drawing necked only the pad ends and the connection was drawn through the gap as a violation.

    WIDE: A (3, 10) and B (27, 10), SMD on F.Cu; the walls on F.Cu, B.Cu closed to tracks by a
    keepout over the whole board (coincident F and B wall pads would share CDT vertices)."""
    f = Fx(30, 20, nets=("WIDE",))
    f.pad("A1", "WIDE", (3.0, 10.0), (1.0, 1.0))
    f.pad("B1", "WIDE", (27.0, 10.0), (1.0, 1.0))
    f.pad("WN", None, (15.0, 4.825), (1.0, 9.65))
    f.pad("WS", None, (15.0, 15.175), (1.0, 9.65))
    f.keepout([pcbnew.B_Cu], [(0.2, 0.2), (29.8, 0.2), (29.8, 19.8), (0.2, 19.8)], tracks=True, vias=True)
    classes = [{"name": "Default", "clearance": 0.2, "track_width": 0.4, "via_diameter": 0.6, "via_drill": 0.3,
                "priority": 2147483647}]
    return f.save(Path(d) / "fx_neck_bottleneck.kicad_pcb", classes=classes)


def fx_fine_pitch_exit(d: Path) -> Path:
    """Pad exits on fine pitch (M-C item 2; qmtech ADV7511 pins 62-68 -> header U4): a row of
    seven 0.3 x 1.6 mm pads at 0.5 mm pitch (long axis along x, like a QFP's left side), the
    middle five connected to a header column up and to the left. A 0.25 mm track with 0.2 mm
    clearance cannot leave a pad through the 0.2 mm gap to its neighbour (it needs 0.125 + 0.2
    there): it has to leave through the pad's tip. Before M-C P2 counted no room at all for a
    wire next to its own pad, planned exits through the side gaps, and the drawing clipped the
    neighbour pads (or found no path from the pad and forced one)."""
    nets = tuple(f"D{i}" for i in range(5))
    f = Fx(16, 16, nets=nets)
    for i in range(7):
        y = 9.5 + 0.5 * i
        net = nets[i - 1] if 1 <= i <= 5 else None
        f.pad(f"U{i}", net, (11.0, y), (1.6, 0.3), shape=pcbnew.PAD_SHAPE_ROUNDRECT)
    for i, net in enumerate(nets):
        f.pad(f"H{i}", net, (3.0, 2.0 + 1.27 * i), (1.0, 1.0))
    return f.save(Path(d) / "fx_fine_pitch_exit.kicad_pcb")


def fx_octi_straight(d: Path) -> Path:
    """Octilinear drawing (M-45): a straight run between two pads 0.6 mm apart across 24 mm. The
    taut path is one segment at 1.4 degrees; the octilinear drawing keeps it straight with one
    short 45 degree jog (before M-45: the slanted segment).

    SIG: A1 (3, 10) and B1 (27, 10.6), SMD on F.Cu."""
    f = Fx(30, 20, nets=("SIG",))
    f.pad("A1", "SIG", (3.0, 10.0), (1.0, 1.0))
    f.pad("B1", "SIG", (27.0, 10.6), (1.0, 1.0))
    return f.save(Path(d) / "fx_octi_straight.kicad_pcb")


def fx_octi_bend(d: Path) -> Path:
    """Octilinear drawing (M-45): free board, the pads 17 mm apart in x and 7 mm in y. The
    octilinear path is an elbow: 10 mm straight and 7 sqrt(2) mm at 45 degrees, one 45 degree
    bend (before M-45: one segment at 22.4 degrees).

    SIG: A1 (3, 3) and B1 (20, 10), SMD on F.Cu."""
    f = Fx(24, 14, nets=("SIG",))
    f.pad("A1", "SIG", (3.0, 3.0), (1.0, 1.0))
    f.pad("B1", "SIG", (20.0, 10.0), (1.0, 1.0))
    return f.save(Path(d) / "fx_octi_bend.kicad_pcb")


def fx_octi_bundle(d: Path) -> Path:
    """Octilinear drawing (M-45): a bundle of four nets turning 90 degrees around a block. The
    nets leave a column of pads on the left (0.5 mm pitch) and end at a row of pads at the top
    right (0.5 mm pitch); an 11.5 x 11.5 mm unconnected pad (on F.Cu and B.Cu) fills the board's
    top left corner, the inside of the turn. Each wire hugs the wire inside it at the clearance, so the bundle keeps its pitch
    through the bend (hand style; before M-45 the taut chords fanned out at any angle).

    B0..B3: L<i> (3, 14 + 0.5 i) to T<i> (14 + 0.5 i, 3), SMD 0.3 x 0.3 mm on F.Cu; the block
    from (0.5, 0.5) to (12, 12)."""
    nets = tuple(f"B{i}" for i in range(4))
    f = Fx(20, 20, nets=nets)
    for i, net in enumerate(nets):
        f.pad(f"L{i}", net, (3.0, 14.0 + 0.5 * i), (0.3, 0.3))
        f.pad(f"T{i}", net, (14.0 + 0.5 * i, 3.0), (0.3, 0.3))
    f.pad("KF", None, (6.25, 6.25), (11.5, 11.5))
    f.pad("KB", None, (6.25, 6.25), (11.5, 11.5), layer=pcbnew.B_Cu)
    return f.save(Path(d) / "fx_octi_bundle.kicad_pcb")


def fx_octi_no_fit(d: Path) -> Path:
    """Octilinear drawing (M-45), where 45 degrees cannot fit: the only way is a channel at 30
    degrees, 0.70 mm wide between copper walls (no net, on F.Cu and B.Cu), where the 0.25 mm track
    with 0.2 mm clearance needs 0.65 mm. A staircase of 0/45/90 degree pieces does not fit
    there: that stretch stays at 30 degrees and is reported (any_angle), the connection is routed
    without a violation (never an open wire or a new violation because of the style).

    SIG: A1 (3, 16) and B1 (27, 16 - 24 tan 30), SMD on F.Cu; the walls cover the board between
    x 6 and 24 except the channel band."""
    import math

    f = Fx(30, 20, nets=("SIG",))
    t = math.tan(math.radians(30.0))
    y0 = 16.0
    a, b = (3.0, y0), (27.0, y0 - 24.0 * t)
    f.pad("A1", "SIG", a, (1.0, 1.0))
    f.pad("B1", "SIG", b, (1.0, 1.0))
    half = 0.35 / math.cos(math.radians(30.0))  # vertical half height of a 0.70 mm band at 30 deg
    yc = lambda x: y0 - (x - 3.0) * t  # noqa: E731
    for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
        # Above the band (smaller y in KiCad's frame) and below it.
        f.shape(layer, [(6.0, 0.3), (24.0, 0.3), (24.0, yc(24.0) - half), (6.0, yc(6.0) - half)])
        f.shape(layer, [(6.0, yc(6.0) + half), (24.0, yc(24.0) + half), (24.0, 19.7), (6.0, 19.7)])
    return f.save(Path(d) / "fx_octi_no_fit.kicad_pcb")


# ---- Inflated sketch boardlets (docs/dev/inflated-sketch.md §6.2): the per-layer cut test (U1 measures them).
# B.Cu is closed by a keepout (tracks and vias) beyond the board edge, so every connection runs on F.Cu through the
# one passage; the walls of unconnected pads reach 0.5 mm past the board edge (a gap at the edge, even one too narrow
# for a track, was taken by the plan at an overflow price: the cut test sees it, the per-edge model did not). The
# minimum track width is the net width, so the plan does not neck (the need is at the nominal width).

def _close_back(f: Fx) -> None:
    f.keepout([pcbnew.B_Cu], [(-1, -1), (f.w + 1, -1), (f.w + 1, f.h + 1), (-1, f.h + 1)], tracks=True, vias=True)


def fx_b1_corner_bundle(d: Path, gap: float = 2.0) -> Path:
    """B1 corner bundle: four nets from a column of pads on the left to one on the right through a
    Z-shaped passage between two walls of unconnected pads: the lower wall's top-right corner
    (15, 10) faces the upper wall's bottom-left corner (15, 10 - gap), the only cut of that length.
    Four 0.25 mm tracks at 0.2 mm clearance need 4 x 0.25 + 3 x 0.2 + 2 x 0.2 = 2.0 mm there.

    gap = 2.0 (variant a, exactly the need): no over-full cut; gap = 1.999 (variant b): exactly one,
    by 1 um. S1..S4: (3, 7 / 8.5 / 10 / 11.5) -> (27, same y), SMD on F.Cu."""
    f = Fx(30, 20, nets=("S1", "S2", "S3", "S4"))
    _close_back(f)
    f.pad("WL", None, (14.5, 15.25), (1.0, 10.5))
    top = 10 - gap
    f.pad("WU", None, (15.5, (top - 0.5) / 2), (1.0, top + 0.5))
    for i, y in enumerate((7.0, 8.5, 10.0, 11.5)):
        f.pad(f"A{i + 1}", f"S{i + 1}", (3.0, y), (0.6, 0.6))
        f.pad(f"B{i + 1}", f"S{i + 1}", (27.0, y), (0.6, 0.6))
    return f.save(Path(d) / f"fx_b1_corner_bundle_{'a' if gap >= 2.0 else 'b'}.kicad_pcb", rules={"min_track_width": 0.25})


def fx_b1b_corner_bundle(d: Path) -> Path:
    """B1 variant b: the passage 1 um narrower than the four tracks need."""
    return fx_b1_corner_bundle(d, gap=1.999)


def fx_b2_squeeze(d: Path) -> Path:
    """B2 non-neighbour squeeze: a wall along y = 10 with a 2.85 mm gap between its ends A (x 13.575)
    and C (x 16.425); small pads B and D at (15, 10 -/+ 0.95) above and below the gap (BD 1.8 mm is
    shorter than AC, so the projection keeps the cut BD and drops AC, which crosses it). Three
    vertical nets pass between A and the B-D column, three between the column and C; 0.25 mm tracks
    at 0.2 mm clearance: AB, AD, BC and CD each hold three (need 1.55 mm, room 1.573 in the
    octagonal norm), AC holds all six (need 2.9 mm, room 2.85): it overflows by 0.05 mm, which no
    projection edge sees. L1..L3 at x 12/13/14, R1..R3 at x 16/17/18, from y 3 to y 17."""
    f = Fx(30, 20, nets=("L1", "L2", "L3", "R1", "R2", "R3"))
    _close_back(f)
    f.pad("WA", None, ((13.575 - 0.5) / 2, 10.0), (14.075, 0.1))
    f.pad("WC", None, ((16.425 + 30.5) / 2, 10.0), (30.5 - 16.425, 0.1))
    f.pad("B", None, (15.0, 9.05), (0.1, 0.1))
    f.pad("D", None, (15.0, 10.95), (0.1, 0.1))
    for name, x in (("L1", 12.0), ("L2", 13.0), ("L3", 14.0), ("R1", 16.0), ("R2", 17.0), ("R3", 18.0)):
        f.pad(f"{name}a", name, (x, 3.0), (0.4, 0.4))
        f.pad(f"{name}b", name, (x, 17.0), (0.4, 0.4))
    return f.save(Path(d) / "fx_b2_squeeze.kicad_pcb", rules={"min_track_width": 0.25})


def fx_b5_other_layer_pad(d: Path) -> Path:
    """B5 other-layer pad: a wall of F.Cu pads along y = 10 with a 1.0 mm gap (x 14.5 to 15.5) and
    a B.Cu-only pad (0.4 mm) in the middle of the gap. The projection's edges across the gap end on
    the B.Cu pad, so on F.Cu they are no cut (room unlimited); the F.Cu sheet has the cut between the
    two F.Cu wall ends, 1.0 mm, which the three nets S1..S3 (x 14 / 15 / 16, y 3 -> 17) need
    3 x 0.25 + 2 x 0.2 + 2 x 0.2 = 1.55 mm of: over-full by 0.55 mm."""
    f = Fx(30, 20, nets=("S1", "S2", "S3"))
    _close_back(f)
    f.pad("WL", None, ((14.5 - 0.5) / 2, 10.0), (15.0, 0.2))
    f.pad("WR", None, ((15.5 + 30.5) / 2, 10.0), (30.5 - 15.5, 0.2))
    f.pad("M", None, (15.0, 10.0), (0.4, 0.4), layer=pcbnew.B_Cu)
    for i, x in enumerate((14.0, 15.0, 16.0)):
        f.pad(f"A{i + 1}", f"S{i + 1}", (x, 3.0), (0.6, 0.6))
        f.pad(f"B{i + 1}", f"S{i + 1}", (x, 17.0), (0.6, 0.6))
    return f.save(Path(d) / "fx_b5_other_layer_pad.kicad_pcb", rules={"min_track_width": 0.25})
# ---------------------------------------------------------------- D7 differential pairs
#: Default class of the pair fixtures: 0.25 mm tracks at 0.2 mm clearance, pairs 0.2 mm wide at a 0.2 mm gap
#: (centre pitch 0.4 mm), 0.6 / 0.3 mm vias (via pair pitch 0.6 + 0.2 = 0.8 mm).
DP_CLASS = {"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.6, "via_drill": 0.3,
            "diff_pair_width": 0.2, "diff_pair_gap": 0.2, "diff_pair_via_gap": 0.25, "priority": 2147483647}


def _dp_pads(f: Fx, ref: str, p_net: str, n_net: str, at, *, pitch=0.5, axis="y", size=(0.8, 0.3)):
    """A two-pad footprint-like pair of SMD pads: P at `at`, N `pitch` further along `axis` (KiCad frame)."""
    x, y = at
    q = (x, y + pitch) if axis == "y" else (x + pitch, y)
    f.pad(ref + "P", p_net, (x, y), size, shape=pcbnew.PAD_SHAPE_ROUNDRECT)
    f.pad(ref + "N", n_net, q, size, shape=pcbnew.PAD_SHAPE_ROUNDRECT)


def fx_dp_straight(d: Path) -> Path:
    """D7: a straight pair. DP+ / DP- leave two pads 0.5 mm apart at x = 3 and run 24 mm to two pads at x = 27
    (same rows): one coupled pair at the 0.2 mm gap, P above N at both ends (no polarity cross), skew 0, the two
    tracks joined to their pads by short fan-out stubs (the pad pitch 0.5 differs from the pair pitch 0.4)."""
    f = Fx(30, 20, nets=("DP+", "DP-"))
    _dp_pads(f, "A", "DP+", "DP-", (3.0, 9.75))
    _dp_pads(f, "B", "DP+", "DP-", (27.0, 9.75))
    return f.save(Path(d) / "fx_dp_straight.kicad_pcb", classes=[DP_CLASS])


def fx_dp_bend(d: Path) -> Path:
    """D7: a pair with one 45 degree bend: from pads at (3, 4) / (3, 4.5) to pads at (20, 11) / (20, 11.5); the
    octilinear elbow has one 45 degree bend, which makes the outer track 2 x pitch x tan 22.5 = 0.33 mm longer
    (skew reported, estimate and actual agree)."""
    f = Fx(24, 16, nets=("CLK_P", "CLK_N"))
    _dp_pads(f, "A", "CLK_P", "CLK_N", (3.0, 4.0))
    _dp_pads(f, "B", "CLK_P", "CLK_N", (20.0, 11.0))
    return f.save(Path(d) / "fx_dp_bend.kicad_pcb", classes=[DP_CLASS])


def fx_dp_via(d: Path) -> Path:
    """D7: a pair that must change layer: a copper wall (no net) across the whole board on F.Cu between x = 12 and
    18; the pads are on F.Cu at both ends. The pair goes down and up with a via pair each time: two vias side by
    side (pitch 0.6 + 0.2 = 0.8 mm), P and N vias on their own sides, no unpaired via."""
    f = Fx(30, 20, nets=("D0P", "D0N"))
    _dp_pads(f, "A", "D0P", "D0N", (3.0, 9.75))
    _dp_pads(f, "B", "D0P", "D0N", (27.0, 9.75))
    f.shape(pcbnew.F_Cu, [(12.0, 0.3), (18.0, 0.3), (18.0, 19.7), (12.0, 19.7)])
    return f.save(Path(d) / "fx_dp_via.kicad_pcb", classes=[DP_CLASS])


def fx_dp_qfn_fanout(d: Path) -> Path:
    """D7: fan-out from 0.5 mm pitch QFN-like pins into a coupled pair: a row of eight 0.8 x 0.25 mm pads at
    0.5 mm pitch (long axis along x, like a QFN's left side) at x = 12; the middle two are USB_D+ / USB_D-, their
    neighbours are pins of other nets (A0..A5, each going to its own test pad on the left). The pair leaves through
    the pad tips, zips at the gap once past the pins and runs to a connector at x = 26. The 0.2 mm tracks with
    0.2 mm clearance cannot leave between the pins: the fan-out is the uncoupled stretch at the pads."""
    others = tuple(f"A{i}" for i in range(6))
    f = Fx(30, 20, nets=("USB_D+", "USB_D-") + others)
    k = 0
    for i in range(8):
        y = 8.25 + 0.5 * i
        if i == 3:
            net = "USB_D+"
        elif i == 4:
            net = "USB_D-"
        else:
            net = others[k]
            k += 1
        f.pad(f"U{i}", net, (12.0, y), (0.8, 0.25), shape=pcbnew.PAD_SHAPE_ROUNDRECT)
    for i, net in enumerate(others):
        f.pad(f"T{i}", net, (3.0, 5.0 + 2.0 * i), (1.0, 1.0))
    _dp_pads(f, "J", "USB_D+", "USB_D-", (26.0, 9.5), pitch=1.0, size=(1.2, 0.6))
    return f.save(Path(d) / "fx_dp_qfn_fanout.kicad_pcb", classes=[DP_CLASS])


def fx_dp_narrow(d: Path) -> Path:
    """D7: a pair through a passage too narrow for the coupled pair: copper walls (no net) on F.Cu and B.Cu across
    the board leave one 0.85 mm channel; the coupled pair needs 0.2 + 0.2 + 0.2 + 2 x 0.2 clearance = 1.0 mm.
    Never drop (M-B1): both halves are routed through it with the shortfall priced and reported as violations,
    nothing is left open."""
    f = Fx(30, 20, nets=("TX+", "TX-"))
    _dp_pads(f, "A", "TX+", "TX-", (3.0, 9.75))
    _dp_pads(f, "B", "TX+", "TX-", (27.0, 9.75))
    for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
        f.shape(layer, [(12.0, 0.3), (18.0, 0.3), (18.0, 9.575), (12.0, 9.575)])
        f.shape(layer, [(12.0, 10.425), (18.0, 10.425), (18.0, 19.7), (12.0, 19.7)])
    return f.save(Path(d) / "fx_dp_narrow.kicad_pcb", classes=[DP_CLASS])


def fx_dp_two_pairs(d: Path) -> Path:
    """D7: two pairs side by side through a bend around a block (like fx_octi_bundle): pairs TX0 and TX1 leave a
    column of pads on the left (0.5 mm pitch, TX0 above TX1) and end at a row of pads at the top right; an
    11.5 x 11.5 mm unconnected pad fills the top left corner. Both pairs stay coupled through their bends and keep
    their clearance to each other."""
    nets = ("TX0+", "TX0-", "TX1+", "TX1-")
    f = Fx(22, 22, nets=nets)
    for i, net in enumerate(nets):
        f.pad(f"L{i}", net, (3.0, 14.5 + 0.6 * i + (0.6 if i >= 2 else 0.0)), (0.8, 0.3), shape=pcbnew.PAD_SHAPE_ROUNDRECT)
    for i, net in enumerate(nets):
        f.pad(f"T{i}", net, (14.5 + 0.6 * i + (0.6 if i >= 2 else 0.0), 3.0), (0.3, 0.8), shape=pcbnew.PAD_SHAPE_ROUNDRECT)
    f.pad("KF", None, (6.25, 6.25), (11.5, 11.5))
    f.pad("KB", None, (6.25, 6.25), (11.5, 11.5), layer=pcbnew.B_Cu)
    return f.save(Path(d) / "fx_dp_two_pairs.kicad_pcb", classes=[DP_CLASS])


def fx_dp_no_partner(d: Path) -> Path:
    """D7: names that end like a pair half but have no partner (EN, SIG_P1) are routed as normal nets; RX_P1 /
    RX_N1 (digits after the polarity, KiCad's rule) are a pair."""
    f = Fx(30, 20, nets=("EN", "SIG_P1", "RX_P1", "RX_N1"))
    f.pad("A1", "EN", (3.0, 4.0), (1.0, 1.0))
    f.pad("B1", "EN", (27.0, 4.0), (1.0, 1.0))
    f.pad("A2", "SIG_P1", (3.0, 16.0), (1.0, 1.0))
    f.pad("B2", "SIG_P1", (27.0, 16.0), (1.0, 1.0))
    _dp_pads(f, "R", "RX_P1", "RX_N1", (3.0, 9.75))
    _dp_pads(f, "S", "RX_P1", "RX_N1", (27.0, 9.75))
    return f.save(Path(d) / "fx_dp_no_partner.kicad_pcb", classes=[DP_CLASS])


# ---------------------------------------------------------------- cleanup-safe output (KiCad's Cleanup Tracks & Vias)


def fx_cleanup_in_pad(d: Path) -> Path:
    """Cleanup-safe output, segments wholly inside a pad: the octilinear drawing reaches a big pad on a 45 degree
    diagonal that ends 0.17 mm short of the pad centre and adds a 0.17 mm jog inside the pad (eprom-to-kernal U2
    pins 3..12, 1.6 mm round pads: 20 such jogs). KiCad's "Delete tracks fully inside pads"
    (tracks_cleaner.cpp:TRACKS_CLEANER::deleteTracksInPads @ 86a7a3f) removes the jog.

    SIG: A1 (3, 3) and B1 (13, 12.83), 1.6 mm round, on F.Cu."""
    f = Fx(16, 16, nets=("SIG",))
    f.pad("A1", "SIG", (3.0, 3.0), (1.6, 1.6), shape=pcbnew.PAD_SHAPE_CIRCLE)
    f.pad("B1", "SIG", (13.0, 12.83), (1.6, 1.6), shape=pcbnew.PAD_SHAPE_CIRCLE)
    return f.save(Path(d) / "fx_cleanup_in_pad.kicad_pcb")


def fx_cleanup_tee(d: Path) -> Path:
    """Cleanup-safe output, T junctions: a three-pad net whose third pad joins the trace between the other two
    (net_tree "free": a branch point on a routed trace). KiCad judges a track end on the middle of another track as
    connected only through that track's shape (connectivity_data.cpp:CONNECTIVITY_DATA::TestTrackEndpointDangling
    @ 86a7a3f: an item that reaches both ends of a short segment counts once), so the junction must be an exact
    vertex shared by three track ends (qmtech ADV_DVDD, ADV_CEC: a 43 / 114 µm segment next to a T was removed with
    the rest of its trace).

    SIG: A (5, 3), B (5, 17), C (15, 10), 1 x 1 mm on F.Cu (a T at (5, 10) is 10 mm, C to a pad 12.2 mm); a no-via
    keepout keeps it on F.Cu."""
    f = Fx(20, 20, nets=("SIG",))
    f.pad("A1", "SIG", (5.0, 3.0), (1.0, 1.0))
    f.pad("B1", "SIG", (5.0, 17.0), (1.0, 1.0))
    f.pad("C1", "SIG", (15.0, 10.0), (1.0, 1.0))
    f.keepout([pcbnew.F_Cu, pcbnew.B_Cu], [(0.2, 0.2), (19.8, 0.2), (19.8, 19.8), (0.2, 19.8)], vias=True)
    return f.save(Path(d) / "fx_cleanup_tee.kicad_pcb")


def fx_cleanup_collinear(d: Path) -> Path:
    """Cleanup-safe output, collinear segments: a trace that runs along a row of foreign pads touches each of them on
    one straight line, and the drawing keeps a vertex at every touching point (qmtech JS2_6 along a header row:
    0.55 / 1.45 mm pieces on y 128.55). KiCad's "Merge co-linear tracks"
    (tracks_cleaner.cpp:TRACKS_CLEANER::mergeCollinearSegments @ 86a7a3f) merges every such pair, so the output must
    already be one segment there.

    SIG: A1 (3, 12) to B1 (27, 12); K<i> (6 + 3 i, 12), i = 0..6, 1 x 1 mm, no net, on F.Cu, on the line between
    them; a no-via keepout keeps SIG on F.Cu."""
    f = Fx(30, 20, nets=("SIG",))
    f.pad("A1", "SIG", (3.0, 12.0), (1.0, 1.0))
    f.pad("B1", "SIG", (27.0, 12.0), (1.0, 1.0))
    for i in range(7):
        f.pad(f"K{i}", None, (6.0 + 3.0 * i, 12.0), (1.0, 1.0))
    f.keepout([pcbnew.F_Cu, pcbnew.B_Cu], [(0.2, 0.2), (29.8, 0.2), (29.8, 19.8), (0.2, 19.8)], vias=True)
    return f.save(Path(d) / "fx_cleanup_collinear.kicad_pcb")


def fx_cleanup_stale_fill(d: Path) -> Path:
    """Cleanup-safe output, zone fills: the board's stored B.Cu GND fill is out of date (it covers only the right
    end of the zone, as a fill made before the outline grew), the user's state after Accept when the plugin does
    not refill. The plugin refills a copy for the engine, so the GND pad's stub and via go into the real pour; KiCad's
    cleanup judges them by the stored fill (connectivity_data.cpp:CONNECTIVITY_DATA::TestTrackEndpointDangling
    @ 86a7a3f: a via connected on one layer only, a track end only in a zone) and removes them unless Accept writes
    the refilled zones too (run.commit_fills; qmtech: a 2.4 mm GND stub).

    GND: G (5, 10) on F.Cu; B.Cu GND zone over the board, its stored fill only x 16..19.5."""
    f = Fx(20, 20, nets=("GND",))
    f.pad("G1", "GND", (5.0, 10.0), (1.0, 1.0))
    f.pad("G2", "GND", (18.0, 10.0), (1.0, 1.0), layer=pcbnew.B_Cu)
    z = f.zone("GND", pcbnew.B_Cu, [(16, 0.5), (19.5, 0.5), (19.5, 19.5), (16, 19.5)])
    pcbnew.ZONE_FILLER(f.b).Fill(f.b.Zones())
    ol = pcbnew.SHAPE_LINE_CHAIN()
    for x, y in [(0.5, 0.5), (19.5, 0.5), (19.5, 19.5), (0.5, 19.5)]:
        ol.Append(_v(x, y))
    ol.SetClosed(True)
    z.Outline().RemoveAllContours()
    z.Outline().AddOutline(ol)
    return f.save(Path(d) / "fx_cleanup_stale_fill.kicad_pcb", fill=False)
