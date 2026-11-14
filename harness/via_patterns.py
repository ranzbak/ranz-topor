"""Via and junction test patterns (shopping list 5.4): redundant via pair, via-pair merge, via relocation by force
balance, branch-point relocation (Steiner point), via size rule.

Six small 2-layer boards (`via_pair_redundant`, `via_pair_slide`, `via_force_relocate`, `branch_point_relocate`,
`branch_point_into_pad`, `via_size_rule`). Every board is SOLVABLE BY CONSTRUCTION: the reference copper is drawn first
from centre lines, the pads are the fixed given, and `check()` verifies with shapely (clearance, copper to edge,
connectivity) before KiCad DRC measures it (0 violations / 0 unconnected). Nothing comes from the engine.

Rules shared by all boards (written to every `.kicad_pro`)
  Default class        clearance 0.20  track 0.25  via 0.80 / 0.40  (the NET CLASS via: deliberately NOT the smallest)
  Board Setup minimums track 0.20, clearance 0.20, via diameter 0.45, through-hole drill 0.25, annular ring 0.10,
                       hole to hole 0.25, copper to edge 0.50
  Vias in every expected answer: the smallest the supplier allows (Board Setup minimums) with pi * drill >= the width of
  the trace it carries (user 2026-11-10): 0.45 / 0.25 for 0.25 mm (pi * 0.25 = 0.785), larger only in via_size_rule.
Price (user rule, all numbers are settings): length in mm + 25 mm per via.

Obstacles are real fixed copper: a STRIP is one long thin SMD pad (0.8 mm wide, from copper-to-edge band to copper-to-
edge band, so nothing can pass between its end and the edge) on ONE layer; a short strip ends inside the board. They are
pads of single-pad nets: no connection of their own, so a router cannot move them (no tie between "the wire hops" and
"the obstacle hops"). The layer of a strip is drawn solid (F.Cu) or hatched (B.Cu); no hidden keep-outs anywhere.

`python -m harness.via_patterns [OUT]` writes every pattern under OUT (default `scratch/via-patterns`): problem/ (pads
only), reference/ (the expected copper), careless/ (the ruled-out copper, real and DRC-checked), expected.json,
`<name>.png` (problem | expected | careless), `<name>_zoom.png`, and README.md. Coordinates in mm, KiCad frame (y down).
Deterministic.
"""
from __future__ import annotations

import json
import math
import sys
import textwrap
from dataclasses import dataclass, field
from pathlib import Path

import pcbnew
from shapely.geometry import LineString, Point, box
from shapely.ops import unary_union

from harness.fit_patterns import EDGE, HOLE_CLEAR, RULES, Fp, Pad, Track, Via, build_board
from harness.fixtures import write_project_file

# ----------------------------------------------------------------------------------------------- rule values
CLEAR = 0.20
W_NORMAL = 0.25
VIA_CLASS = (0.80, 0.40)  # net class via: NEVER the expected one
MIN_ANNULAR, MIN_DRILL, MIN_VIA_D = 0.10, 0.25, 0.45
VIA_PRICE = 25.0
SQ2 = math.sqrt(2.0)
#: The rules every expected answer here is built for (harness.validity ``rules_assumed``): the board's own Board
#: Setup and net classes (no fab profile: ``[rules] fab_profile = "board"``, the default) and the via rule of the
#: answers, ``[vias] size = "board_min"`` (the smallest Board Setup via with pi x drill >= the trace width).
RULES_ASSUMED = {"authority": "board", "margin": 0.0, "options": {"vias.size": "board_min"}}
RULES_VP = dict(RULES, min_track_width=0.20, min_via_diameter=MIN_VIA_D, min_through_hole_diameter=MIN_DRILL,
                min_via_annular_width=MIN_ANNULAR, min_hole_to_hole=0.25, min_hole_clearance=HOLE_CLEAR,
                min_clearance=CLEAR)
STRIP_W = 0.8
PAD = 0.8  # terminal pad size
BAD, OK = "#d62728", "#138a36"
PALETTE = ["#1f77b4", "#ff7f0e", "#2ca02c", "#9467bd", "#8c564b", "#17becf", "#bcbd22", "#e377c2"]
GREY = "#8a8a8a"
LIT = {
    "luzin_popov2013": "Luzin, Popov, Popov 2013, 'Local minimisation of the number of vias in the topology of a printed "
                       "circuit' (docs/literature/luzin-popov2013-local-via-minimization.pdf), p. 2 algorithm steps 1-10 "
                       "(adjacent vias v, u joined by a wire shorter than e.g. 10 mm: copy that wire onto the layers of the "
                       "wires at v, re-lay wires that block it behind u, delete v), p. 2-3 fig. 2-3 (non-adjacent vias are "
                       "made adjacent), p. 3: 'afterwards the via positions are corrected by a force algorithm [6]'",
    "dayan_dai1991": "Dayan, Dai 1991, UCSC-CRL-91-40 (docs/literature/dayan-dai1991-ucsc-crl-91-40-force-driven.pdf): "
                     "force-driven wire/via placement, vias and wires repel foreign copper and the wires pull straight",
    "topor63": "TopoR 6.3 User Manual (docs/literature/eremex2017-topor63-user-manual.pdf) pp. 175-183: FreeStyle, move "
               "vias and T-points, delete extra vias",
    "popov2013": "Popov, Popov 2013, EDN 'Flexible routing' (docs/literature/popov2013-edn-flexible-routing-full.txt): "
                 "Steiner junctions at < 120 degrees, a pad takes over the junction when an angle is >= 120",
    "topor50_e8": "TopoR 5.0 black box E8c / E8d (docs/literature/topor50-blackbox-results.md): Auto move puts a free "
                  "T at the Fermat point (120 / 120 / 120); at an angle >= 125 the junction is deleted into the pad",
    "power_via": "memory power-via-rules rule 0 (user 2026-11-10): smallest via of the Board Setup minimums with "
                 "pi * drill >= the trace width, not the net class via",
}


def r4(x):
    return round(x, 4)


def rp(p):
    return [r4(p[0]), r4(p[1])]


def plen(pts):
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def via_for(width: float, grid: float = 0.01) -> tuple[float, float]:
    """(diameter, drill) of the smallest via the Board Setup allows that carries `width`: pi * drill >= width."""
    drill = max(MIN_DRILL, math.ceil(width / math.pi / grid - 1e-9) * grid)
    return r4(max(MIN_VIA_D, drill + 2 * MIN_ANNULAR)), r4(drill)


MIN_VIA = via_for(W_NORMAL)  # 0.45 / 0.25


# ----------------------------------------------------------------------------------------------- model
@dataclass
class Sol:
    label: str
    tracks: list[Track]
    vias: list[Via]
    text: str = ""
    notes: list[dict] = field(default_factory=list)  # drawn annotations: {"pt":, "text":, "color":} / {"p":, "q":, "text":}


@dataclass
class VPat:
    name: str
    title: str
    size: tuple[float, float]
    fps: list[Fp]
    connections: list[tuple[str, str, str]]
    ref: Sol
    careless: Sol
    answer: str
    why: str
    expected: dict
    classes: list[dict]
    net_patterns: list[dict] = field(default_factory=list)
    zooms: list[tuple[tuple, str]] = field(default_factory=list)
    rules: dict = field(default_factory=lambda: dict(RULES_VP))
    layers: int = 2
    keepouts: list = field(default_factory=list)
    colors: dict = field(default_factory=dict)

    @property
    def nets(self):
        out = []
        for f in self.fps:
            for p in f.pads:
                if p.net and p.net not in out:
                    out.append(p.net)
        return out

    @property
    def ref_tracks(self):
        return self.ref.tracks

    @property
    def ref_vias(self):
        return self.ref.vias


class _View:
    """A VPat seen with another solution's copper (duck-typed for fit_patterns.build_board)."""

    def __init__(self, pat: VPat, sol: Sol):
        self._p, self.ref_tracks, self.ref_vias = pat, sol.tracks, sol.vias

    def __getattr__(self, k):
        return getattr(self._p, k)


def smd(num, net, x, y, w=PAD, h=PAD, layer="F.Cu") -> Pad:
    return Pad(num, net, (x, y), (w, h), layer=layer)


def terminal(ref, net, x, y, w=PAD, h=PAD, layer="F.Cu") -> Fp:
    return Fp(ref, [smd("1", net, x, y, w, h, layer)], "terminal")


def strip(ref, net, x, y0, y1, w=STRIP_W, layer="F.Cu") -> Fp:
    return Fp(ref, [smd("1", net, x, (y0 + y1) / 2, w, y1 - y0, layer)], "wall")


def mkvia(net, pos, dia_drill=None):
    d, dr = dia_drill or MIN_VIA
    return Via(net, pos, d, dr)


def classes(extra=()):
    return [{"name": "Default", "clearance": CLEAR, "track_width": W_NORMAL, "via_diameter": VIA_CLASS[0],
             "via_drill": VIA_CLASS[1], "priority": 2147483647}] + [dict(c) for c in extra]


def tl(sol: Sol, net: str) -> float:
    return sum(plen(t.pts) for t in sol.tracks if t.net == net)


def price(sol: Sol, nets: list[str]) -> dict:
    ln = {n: r4(tl(sol, n)) for n in nets}
    return {"length_mm": r4(sum(ln.values())), "per_net": ln, "vias": len(sol.vias),
            "price": r4(sum(ln.values()) + VIA_PRICE * len(sol.vias))}


# ----------------------------------------------------------------------------------------------- verification
def pad_geo(p: Pad):
    return box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2, p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2)


def copper(pat: VPat, sol: Sol | None):
    """[(net, layer, geom, kind)] of every copper item (pads of the given, tracks, vias on both layers)."""
    out = []
    for f in pat.fps:
        for p in f.pads:
            out.append((p.net, p.layer, pad_geo(p), "pad"))
    if sol:
        for t in sol.tracks:
            out.append((t.net, t.layer, LineString(t.pts).buffer(t.width / 2, quad_segs=16), "track"))
        for v in sol.vias:
            for ly in ("F.Cu", "B.Cu"):
                out.append((v.net, ly, Point(v.pos).buffer(v.dia / 2, quad_segs=16), "via"))
    return out


def clearance_report(pat: VPat, sol: Sol) -> dict:
    items = copper(pat, sol)
    worst = 9e9
    bad = []
    for i, (n1, l1, g1, k1) in enumerate(items):
        for n2, l2, g2, k2 in items[i + 1:]:
            if l1 != l2 or n1 == n2:
                continue
            d = g1.distance(g2)
            worst = min(worst, d)
            if d < CLEAR - 1e-6:
                bad.append((n1, n2, l1, round(d, 4)))
    W, H = pat.size
    edge = min(min(g.bounds[0], g.bounds[1], W - g.bounds[2], H - g.bounds[3]) for _, _, g, _ in items)
    return {"min_clearance": r4(worst), "violations": bad, "min_edge": r4(edge), "edge_ok": edge >= EDGE - 1e-6}


def connected(pat: VPat, sol: Sol, net: str) -> bool:
    items = [(l, g) for n, l, g, k in copper(pat, sol) if n == net]
    n = len(items)
    parent = list(range(n))

    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    for i in range(n):
        for j in range(i + 1, n):
            if items[i][0] == items[j][0] and items[i][1].intersects(items[j][1]):
                parent[find(i)] = find(j)
    # a via joins its two layer discs (same geometry on both layers) -> already joined by overlap of identical discs? no:
    # the two discs are on different layers, so join them explicitly
    for v in sol.vias:
        if v.net != net:
            continue
        ids = [i for i, (_, g) in enumerate(items) if g.equals(Point(v.pos).buffer(v.dia / 2, quad_segs=16))]
        for i in ids[1:]:
            parent[find(i)] = find(ids[0])
    pads = [i for i, (n_, l, g, k) in enumerate([c for c in copper(pat, sol) if c[0] == net]) if k == "pad"]
    return len({find(i) for i in pads}) == 1


def check(pat: VPat) -> dict:
    out = {}
    for key, sol in (("reference", pat.ref), ("careless", pat.careless)):
        rep = clearance_report(pat, sol)
        assert not rep["violations"], (pat.name, key, rep["violations"])
        assert rep["edge_ok"], (pat.name, key, rep)
        for net, _, _ in pat.connections:
            assert connected(pat, sol, net), (pat.name, key, net, "not connected")
        out[key] = rep
    return out


# ----------------------------------------------------------------------------------------------- 1 via_pair_redundant
def p_redundant() -> VPat:
    """A on F.Cu (pads on F.Cu) with a short F.Cu strip across its straight line: hopping under it costs two vias
    (50 mm); going round the strip end costs 1.95 mm."""
    W, H = 20.0, 14.0
    y0 = 7.0
    A1, A2 = (3.0, y0), (17.0, y0)
    sx, s_y0, s_y1 = 10.0, 4.0, 9.0
    fps = [terminal("A1", "A", *A1), terminal("A2", "A", *A2), strip("S1", "OBST", sx, s_y0, s_y1)]
    w = W_NORMAL
    # reference: below the strip end: the line 0.2 + w/2 + 0.025 (grid) clear of the strip's lower end
    yb = r4(s_y1 + CLEAR + w / 2 + 0.025 + 0.0)  # 9.35
    dy = yb - y0
    ref_pts = [A1, (A1[0] + dy, yb), (A2[0] - dy, yb), A2]
    ref = Sol("A round the strip end on F.Cu, 0 vias", [Track("A", "F.Cu", w, ref_pts)], [],
              "straight on the pad row, 45 degrees down to the clearance line below the strip end, along it, 45 degrees up")
    # careless: dive under the strip: via before, via after
    vx = 1.15  # via centre to strip axis: 0.4 + 0.2 + 0.225 + slack
    v1, v2 = (sx - vx, y0), (sx + vx, y0)
    car = Sol("A hops under the strip: F.Cu, via, B.Cu, via, F.Cu", [
        Track("A", "F.Cu", w, [A1, v1]), Track("A", "B.Cu", w, [v1, v2]), Track("A", "F.Cu", w, [v2, A2])],
        [mkvia("A", v1), mkvia("A", v2)], "two layer changes around something that does not need them")
    pr, pc = price(ref, ["A"]), price(car, ["A"])
    straight = plen([A1, A2])
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 0, "via_positions": [],
        "tolerance_mm": None, "reference": pr, "careless": pc, "straight_mm": r4(straight),
        "break_even": {"via_pair_price_mm": 2 * VIA_PRICE, "detour_mm": r4(pr["length_mm"] - straight),
                       "note": "going round wins for every detour up to 2 x 25 = 50 mm"},
        "interpretation": {"what": "a layer-change pair (F->B->F) that exists only to pass an obstacle which can be "
                                   "passed on the wire's own layer for less than the vias cost",
                           "minimiser_move": "Luzin-Popov step 8: copy the wire between the two vias onto the layer of "
                                             "the wires at the first via and delete the via; the second via then joins "
                                             "two wires of one layer and goes too"},
        "judgement_calls": [
            "the strip is a foreign pad on F.Cu only (single-pad net OBST, no connection): 'something that does not need it "
            "on that layer' is read as an F.Cu obstacle that has an end inside the board; with a copper-to-edge-to-edge "
            "strip the two vias would be needed (that is via_pair_slide)",
            "price 25 mm per via is the user rule; the expected answer holds for every via price above 1.0 mm"],
        "literature": [LIT["luzin_popov2013"], LIT["topor63"]],
    }
    return VPat("via_pair_redundant", "A wire hops down and up around a strip it can walk round: 0 vias", (W, H), fps,
                [("A", "A1.1", "A2.1")], ref, car,
                f"0 vias. A runs on F.Cu round the lower end of the F.Cu strip: {pr['length_mm']:.3f} mm (straight "
                f"{straight:.1f}, detour {pr['length_mm'] - straight:.2f}); price {pr['price']:.1f}. Careless: hop under the "
                f"strip, 2 vias, {pc['length_mm']:.1f} mm, price {pc['price']:.1f}.",
                "Two vias cost 50 mm; the detour round the strip end costs 1.95 mm. Going round the upper end costs 2.6 mm "
                "more, so the lower end is the optimum. The strip does not reach the board edge, so the way round is open.",
                exp, classes(), zooms=[((7.0, 5.5, 13.0, 10.5), "A round the strip end (clearance line y = 9.35)")],
                colors={"A": PALETTE[0], "OBST": GREY})


# ----------------------------------------------------------------------------------------------- 2 via_pair_slide
def p_slide() -> VPat:
    """Two F.Cu strips from edge to edge across A's line: A must be on B.Cu under both. Careless: dive and rise at
    each strip (4 vias); the two vias between the strips are adjacent, 6 mm apart: they merge (2 vias)."""
    W, H = 22.0, 14.0
    y0 = 7.0
    A1, A2 = (2.0, y0), (20.0, y0)
    x1, x2 = 8.0, 14.0
    fps = [terminal("A1", "A", *A1), terminal("A2", "A", *A2),
           strip("S1", "WALL1", x1, EDGE, H - EDGE), strip("S2", "WALL2", x2, EDGE, H - EDGE)]
    w = W_NORMAL
    d = MIN_VIA[0] / 2
    lo1 = A1[0] + PAD / 2 + CLEAR + d  # via must clear pad A1 ...
    hi1 = x1 - STRIP_W / 2 - CLEAR - d  # ... and strip 1
    lo2 = x2 + STRIP_W / 2 + CLEAR + d
    hi2 = A2[0] - PAD / 2 - CLEAR - d
    v1x, v2x = r4((lo1 + hi1) / 2), r4((lo2 + hi2) / 2)  # 5.0 and 17.0: the middle of the free interval
    v1, v2 = (v1x, y0), (v2x, y0)
    ref = Sol("F.Cu, via, ONE B.Cu stretch under both strips, via, F.Cu: 2 vias",
              [Track("A", "F.Cu", w, [A1, v1]), Track("A", "B.Cu", w, [v1, v2]), Track("A", "F.Cu", w, [v2, A2])],
              [mkvia("A", v1), mkvia("A", v2)])
    # careless: dive/rise at each strip; between the strips two vias, 3 mm apart
    c = [(5.0, y0), (9.5, y0), (12.5, y0), (17.0, y0)]
    car = Sol("A hops under each strip separately: 4 vias", [
        Track("A", "F.Cu", w, [A1, c[0]]), Track("A", "B.Cu", w, [c[0], c[1]]), Track("A", "F.Cu", w, [c[1], c[2]]),
        Track("A", "B.Cu", w, [c[2], c[3]]), Track("A", "F.Cu", w, [c[3], A2])],
        [mkvia("A", p) for p in c],
        "the F.Cu stretch between the strips is the wire between two ADJACENT vias (3.0 mm < 10 mm): Luzin-Popov copies it "
        "to B.Cu, where nothing crosses it, and deletes both vias")
    pr, pc = price(ref, ["A"]), price(car, ["A"])
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 2,
        "via_positions": [{"id": "V1", "x_range": [r4(lo1), r4(hi1)], "y": y0, "recommended": rp(v1),
                           "role": "dive, before strip 1"},
                          {"id": "V2", "x_range": [r4(lo2), r4(hi2)], "y": y0, "recommended": rp(v2),
                           "role": "rise, after strip 2"}],
        "tolerance_mm": "any x inside the range (the wire is straight, so the length does not change); recommended = "
                        "middle of the free interval (equal clearance, see via_force_relocate)",
        "reference": pr, "careless": pc,
        "minimum_vias_proof": "both strips run from copper-to-edge band to copper-to-edge band on F.Cu: no way round, "
                              "every route from A1 to A2 changes layer; one B.Cu stretch under both needs 2 vias",
        "interpretation": {"what": "Luzin-Popov local via-pair minimisation applied to the adjacent vias between the "
                                   "strips; the vias 'slide' outward to the dive and rise sites",
                           "moves": "V2, V3 (between the strips) are deleted; V1 and V4 stay"},
        "judgement_calls": [
            "strips span the full board height so 2 vias is the proved minimum; 4 vias is the careless per-obstacle answer",
            "length is the same for both answers (straight 18.0 mm on the pad row); the saving is 2 vias = 50 mm"],
        "literature": [LIT["luzin_popov2013"], LIT["topor63"]],
    }
    return VPat("via_pair_slide", "Two walls in a row: one B.Cu stretch under both, two vias fewer", (W, H), fps,
                [("A", "A1.1", "A2.1")], ref, car,
                f"2 vias: A dives on F.Cu before strip 1 (x in [{lo1:.2f}, {hi1:.2f}], recommended 5.0) and rises after strip "
                f"2 (x in [{lo2:.2f}, {hi2:.2f}], recommended 17.0); one straight B.Cu stretch under both; {pr['length_mm']:.1f} "
                f"mm, price {pr['price']:.1f}. Careless: 4 vias, same length, price {pc['price']:.1f}.",
                "Both strips are F.Cu only and reach the copper-to-edge band at both ends: no route stays on F.Cu, so 2 vias "
                "is the minimum. The two inner vias are adjacent and joined by a 3 mm F.Cu wire; the B.Cu stretch under the "
                "strips is free, so the wire moves to B.Cu and both vias vanish (Luzin-Popov 2013 steps 4-10).",
                exp, classes(), zooms=[((2.0, 4.5, 11.0, 9.5), "dive site V1 and strip 1"),
                                       ((11.0, 4.5, 20.0, 9.5), "strip 2 and rise site V2")],
                colors={"A": PALETTE[0], "WALL1": GREY, "WALL2": "#b5b5b5"})


# ----------------------------------------------------------------------------------------------- 3 via_force_relocate
def p_force() -> VPat:
    """Same single wire; the free interval for each via is bounded by foreign copper on the dive layer; the cramped
    answer sits at the clearance limit, the expected one in the middle (equal clearance)."""
    W, H = 22.0, 14.0
    y0 = 7.0
    A1, A2 = (2.0, y0), (20.0, y0)
    xn1, xw, xn3 = 6.0, 12.0, 16.0
    fps = [terminal("A1", "A", *A1), terminal("A2", "A", *A2),
           strip("N1", "NB1", xn1, EDGE, H - EDGE, layer="B.Cu"),
           strip("W", "WALL", xw, EDGE, H - EDGE),
           strip("N3", "NB3", xn3, EDGE, H - EDGE, layer="B.Cu")]
    w = W_NORMAL
    d = MIN_VIA[0] / 2
    h = STRIP_W / 2
    lo1, hi1 = xn1 + h + CLEAR + d, xw - h - CLEAR - d
    lo2, hi2 = xw + h + CLEAR + d, xn3 - h - CLEAR - d
    m1, m2 = r4((lo1 + hi1) / 2), r4((lo2 + hi2) / 2)
    ref = Sol("vias in the middle of their free intervals (equal clearance)",
              [Track("A", "F.Cu", w, [A1, (m1, y0)]), Track("A", "B.Cu", w, [(m1, y0), (m2, y0)]),
               Track("A", "F.Cu", w, [(m2, y0), A2])], [mkvia("A", (m1, y0)), mkvia("A", (m2, y0))])
    c1, c2 = r4(lo1 + 0.05), r4(hi2 - 0.05 if False else lo2 + 0.05)  # first positions: squeezed against the neighbours
    # V1 squeezed against the B.Cu strip N1 (left), V2 squeezed against the F.Cu strip W (left of it is V1's side;
    # V2 sits right after W)
    car = Sol("vias placed at the first free spot: squeezed against the neighbours",
              [Track("A", "F.Cu", w, [A1, (c1, y0)]), Track("A", "B.Cu", w, [(c1, y0), (c2, y0)]),
               Track("A", "F.Cu", w, [(c2, y0), A2])], [mkvia("A", (c1, y0)), mkvia("A", (c2, y0))],
              "same length, same 2 vias; the vias leave 0.25 mm to the nearest foreign copper instead of 2.4 mm")

    def gaps(x_v, left_edge, right_edge):
        return r4(x_v - d - left_edge), r4(right_edge - (x_v + d))

    g_ref1 = gaps(m1, xn1 + h, xw - h)
    g_ref2 = gaps(m2, xw + h, xn3 - h)
    g_car1 = gaps(c1, xn1 + h, xw - h)
    g_car2 = gaps(c2, xw + h, xn3 - h)
    pr, pc = price(ref, ["A"]), price(car, ["A"])
    car.notes = [{"p": (xn1 + h, 8.6), "q": (c1 - d, 8.6), "text": f"{g_car1[0]:.2f}"},
                 {"p": (c2 - d - 0.0, 8.6), "q": (xw + h, 8.6), "text": f"{g_car2[0]:.2f}"}]
    ref.notes = [{"p": (xn1 + h, 8.6), "q": (m1 - d, 8.6), "text": f"{g_ref1[0]:.2f}"},
                 {"p": (m1 + d, 8.6), "q": (xw - h, 8.6), "text": f"{g_ref1[1]:.2f}"},
                 {"p": (xw + h, 8.6), "q": (m2 - d, 8.6), "text": f"{g_ref2[0]:.2f}"},
                 {"p": (m2 + d, 8.6), "q": (xn3 - h, 8.6), "text": f"{g_ref2[1]:.2f}"}]
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 2,
        "via_positions": [{"id": "V1", "recommended": [m1, y0], "tolerance_mm": 0.25,
                           "free_interval_x": [r4(lo1), r4(hi1)], "role": "dive, between B.Cu strip N1 and F.Cu strip W"},
                          {"id": "V2", "recommended": [m2, y0], "tolerance_mm": 0.25,
                           "free_interval_x": [r4(lo2), r4(hi2)], "role": "rise, between F.Cu strip W and B.Cu strip N3"}],
        "tolerance_mm": 0.25, "reference": pr, "careless": pc,
        "edge_gaps_mm": {"reference": {"V1": g_ref1, "V2": g_ref2}, "careless": {"V1": g_car1, "V2": g_car2}},
        "min_gap_mm": {"reference": min(g_ref1 + g_ref2), "careless": min(g_car1 + g_car2)},
        "interpretation": {
            "objective": "total wire length first (flat here: the wire is straight, any via position inside the interval "
                         "gives the same length), then equal clearance to the foreign copper on both sides of the via "
                         "(the force balance: foreign copper repels the via with the same law, the wires only pull it "
                         "back onto the straight line)",
            "bounds": "V1 must not cross B.Cu strip N1 (its B.Cu stretch would cross it) nor touch F.Cu strip W; V2 "
                      "likewise between W and N3. Strips are edge-to-edge, so there is no way round them."},
        "judgement_calls": [
            "the neighbours are strips (pads) of single-pad nets, not wires, so they cannot move; the force law (inverse "
            "distance, equal weights) is not fixed here - only the symmetric equilibrium is claimed",
            "tolerance 0.25 mm around the middle of each free interval; clearance gap asymmetry beyond that = not relaxed",
            "the length does not change between careless and expected (price equal): the gain is clearance, not mm"],
        "literature": [LIT["dayan_dai1991"], LIT["luzin_popov2013"]],
    }
    return VPat("via_force_relocate", "Vias in tight spots slide to equal clearance (same length)", (W, H), fps,
                [("A", "A1.1", "A2.1")], ref, car,
                f"2 vias, same copper length {pr['length_mm']:.1f} mm as the careless answer, but V1 at x = {m1:.3f} and V2 at "
                f"x = {m2:.3f} (+-0.25) instead of squeezed against the neighbours: gaps {g_ref1[0]:.2f}/{g_ref1[1]:.2f} and "
                f"{g_ref2[0]:.2f}/{g_ref2[1]:.2f} mm instead of {g_car1[0]:.2f}/{g_car1[1]:.2f} and {g_car2[0]:.2f}/"
                f"{g_car2[1]:.2f}.",
                "A has to be on B.Cu under the F.Cu strip W (edge to edge). V1 cannot sit left of the B.Cu strip N1 (the B.Cu "
                "stretch would cross it), V2 cannot sit right of N3. Inside the free intervals the length is the same, so "
                "the force balance puts each via where the gaps to the two neighbours are equal.",
                exp, classes(), zooms=[((4.5, 5.0, 13.5, 9.5), "V1 between N1 (B.Cu) and W (F.Cu)"),
                                       ((10.5, 5.0, 17.5, 9.5), "V2 between W and N3 (B.Cu)")],
                colors={"A": PALETTE[0], "NB1": GREY, "WALL": "#b5b5b5", "NB3": "#6e6e6e"})


# ----------------------------------------------------------------------------------------------- 4 branch point
def fermat(P, Q, R, it=400):
    x = [(P[0] + Q[0] + R[0]) / 3, (P[1] + Q[1] + R[1]) / 3]
    for _ in range(it):
        ws = [1 / max(math.dist(x, p), 1e-9) for p in (P, Q, R)]
        x = [sum(w * p[i] for w, p in zip(ws, (P, Q, R))) / sum(ws) for i in (0, 1)]
    return tuple(x)


def angle(a, o, b):
    v1, v2 = (a[0] - o[0], a[1] - o[1]), (b[0] - o[0], b[1] - o[1])
    return math.degrees(math.acos((v1[0] * v2[0] + v1[1] * v2[1]) / (math.hypot(*v1) * math.hypot(*v2))))


def p_branch() -> VPat:
    W, H = 20.0, 14.0
    P, Q, R = (3.0, 2.0), (3.0, 12.0), (17.0, 7.0)
    fps = [terminal("P", "T", *P), terminal("Q", "T", *Q), terminal("R", "T", *R)]
    w = W_NORMAL
    F = fermat(P, Q, R)
    F = (r4(F[0]), r4(F[1]))
    ref = Sol("T junction at the Steiner (Fermat) point", [Track("T", "F.Cu", w, [P, F]), Track("T", "F.Cu", w, [Q, F]),
                                                          Track("T", "F.Cu", w, [F, R])], [],
              "three straight wires meeting at 120 / 120 / 120 degrees; any-angle (smooth) style")
    J = (3.0, 7.0)
    car = Sol("T junction on the P-Q wire (R joined to the nearest point of the existing wire)",
              [Track("T", "F.Cu", w, [P, J, Q]), Track("T", "F.Cu", w, [J, R])], [],
              "what a router that attaches the third pad to the nearest point of the first wire produces")
    pr, pc = price(ref, ["T"]), price(car, ["T"])
    angs = [r4(angle(P, F, Q)), r4(angle(P, F, R)), r4(angle(Q, F, R))]
    mst = plen([P, Q]) + min(math.dist(R, P), math.dist(R, Q))
    # octilinear optimum for the same pads (junction on the pad row y = 7 reached by 45-degree legs): informational
    # Steiner point on the axis: legs from P and Q at 45 degrees meeting the horizontal trunk at (3 + 5, 7) = (8, 7)
    oct_len = 2 * 5 * SQ2 + (R[0] - 8.0)
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 0, "via_positions": [],
        "junction": {"expected": rp(F), "tolerance_mm": 0.15, "angles_deg": angs, "careless": rp(J)},
        "reference": pr, "careless": pc, "mst_pad_chain_mm": r4(mst),
        "gain_vs_careless_pct": r4(100 * (pc["length_mm"] - pr["length_mm"]) / pc["length_mm"]),
        "octilinear_note": {"steiner_junction": [8.0, 7.0], "length_mm": r4(oct_len),
                            "text": "in the 45-degree style the 120 degree junction does not exist; the octilinear optimum "
                                    "(two 45-degree legs from P and Q to a trunk on y = 7, trunk to R) is the same family "
                                    "of answers: junction x = 8.0, length informational, not drawn"},
        "interpretation": {"rule": "all three angles < 120: a free branch point at the Fermat point, 120 / 120 / 120 "
                                   "(Popov 2013 EDN; TopoR 5.0 E8c Auto move)"},
        "judgement_calls": [
            "any-angle (smooth) style is drawn; an octilinear board would use the 45-degree variant (length in the note)",
            "TopoR 5.0 autorouting builds pad-to-pad chains (E3b); its Auto move in FreeStyle moves a pre-routed T to the "
            "Fermat point (E8c). The TopoR run therefore pre-routes the careless T and applies Auto move"],
        "literature": [LIT["popov2013"], LIT["topor50_e8"], LIT["topor63"]],
    }
    return VPat("branch_point_relocate", "3-terminal net: the T junction belongs at the Steiner point", (W, H), fps,
                [("T", "P.1", "Q.1"), ("T", "Q.1", "R.1")], ref, car,
                f"0 vias, junction at the Fermat point {rp(F)} (angles {angs}), total {pr['length_mm']:.3f} mm. Careless "
                f"T on the P-Q wire at (3, 7): {pc['length_mm']:.1f} mm (+{exp['gain_vs_careless_pct']:.1f} %); pad chain {mst:.2f}.",
                "All three angles of the pad triangle are below 120 degrees (P 72.6, Q 72.6, R 39.8), so the shortest tree "
                "has a free branch point where the three wires meet at 120 degrees. No other copper is on the board.",
                exp, classes(), zooms=[((1.5, 3.5, 9.5, 10.5), "junction: expected F vs careless J")],
                colors={"T": PALETTE[0]})


def p_branch_pad() -> VPat:
    W, H = 20.0, 12.0
    A, M, V = (3.0, 3.0), (10.0, 6.0), (17.0, 3.0)
    fps = [terminal("A", "T", *A), terminal("M", "T", *M), terminal("V", "T", *V)]
    w = W_NORMAL
    ref = Sol("chain A-M-V: the junction is the pad M", [Track("T", "F.Cu", w, [A, M]), Track("T", "F.Cu", w, [M, V])], [],
              "two wires end in pad M; no free branch point")
    J = (10.0, 3.0)
    car = Sol("free T on the A-V line above M", [Track("T", "F.Cu", w, [A, J, V]), Track("T", "F.Cu", w, [J, M])], [],
              "M joined to the nearest point of the A-V wire")
    pr, pc = price(ref, ["T"]), price(car, ["T"])
    ang_m = r4(angle(A, M, V))
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 0, "via_positions": [],
        "junction": {"expected": "pad M (no free branch point)", "angle_at_M_deg": ang_m, "careless": rp(J)},
        "reference": pr, "careless": pc,
        "gain_vs_careless_pct": r4(100 * (pc["length_mm"] - pr["length_mm"]) / pc["length_mm"]),
        "interpretation": {"rule": "one angle >= 120 degrees: the Steiner tree degenerates, the pad at the wide angle is the "
                                   "junction (TopoR 5.0 E8d: junction deleted into the pad)"},
        "judgement_calls": ["a wire pair through a pad is a planar contact; whether a router counts that as a 'branch' is its "
                            "own bookkeeping, the expected answer only says: no free T, total length minimal"],
        "literature": [LIT["popov2013"], LIT["topor50_e8"]],
    }
    return VPat("branch_point_into_pad", "Wide angle at a pad: the junction is the pad, not a free T", (W, H), fps,
                [("T", "A.1", "M.1"), ("T", "M.1", "V.1")], ref, car,
                f"0 vias, no free junction: the wires meet in pad M (angle {ang_m} >= 120), total {pr['length_mm']:.3f} mm. "
                f"Careless free T at (10, 3): {pc['length_mm']:.1f} mm (+{exp['gain_vs_careless_pct']:.1f} %).",
                f"The angle at M between the wires to A and V is {ang_m} degrees (>= 120): the Fermat point would lie "
                "outside the triangle's interior, so the shortest tree is the chain through M.",
                exp, classes(), zooms=[((6.0, 1.5, 14.0, 7.5), "pad M and the careless T")],
                colors={"T": PALETTE[1]})


# ----------------------------------------------------------------------------------------------- 5 via_size_rule
def p_size() -> VPat:
    W, H = 26.0, 21.0
    rows = [("SIG", 0.2, 3.0, 0.8), ("PWR", 1.0, 9.5, 2.0), ("HEAVY", 2.0, 16.5, 3.0)]
    x1, x2 = 4.5, 21.5
    xv = 13.0
    fps, tracks, ctracks, vias, cvias = [], [], [], [], []
    cls_extra, pats, conns, vinfo = [], [], [], {}
    for i, (net, wd, y, ps) in enumerate(rows):
        fps += [terminal(f"{net}_1", net, x1, y, ps, ps, "F.Cu"), terminal(f"{net}_2", net, x2, y, ps, ps, "B.Cu")]
        conns.append((net, f"{net}_1.1", f"{net}_2.1"))
        cls_extra.append({"name": net, "clearance": CLEAR, "track_width": wd, "via_diameter": VIA_CLASS[0],
                          "via_drill": VIA_CLASS[1], "priority": i})
        pats.append({"netclass": net, "pattern": net})
        dia, dr = via_for(wd)
        vinfo[net] = {"width": wd, "need_circumference_drill": r4(wd / math.pi), "drill": dr, "dia": dia,
                      "annular": r4((dia - dr) / 2), "pi_drill": r4(math.pi * dr), "class_via": list(VIA_CLASS)}
        for (tr, vs, v) in ((tracks, vias, (dia, dr)), (ctracks, cvias, VIA_CLASS)):
            tr += [Track(net, "F.Cu", wd, [(x1, y), (xv, y)]), Track(net, "B.Cu", wd, [(xv, y), (x2, y)])]
            vs.append(Via(net, (xv, y), v[0], v[1]))
    ref = Sol("smallest via the Board Setup allows with pi x drill >= width", tracks, vias)
    car = Sol("net class via 0.80 / 0.40 on every net", ctracks, cvias,
              "the class via is bigger than needed for SIG and PWR (pi x 0.40 = 1.26) and the wrong rule")
    nets = [r[0] for r in rows]
    pr, pc = price(ref, nets), price(car, nets)
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 3,
        "via_positions": [{"net": n, "x_range": "anywhere on the straight row clear of both pads", "recommended": [xv, y],
                           "via": f"{vinfo[n]['dia']} / {vinfo[n]['drill']}"} for n, _, y, _ in rows],
        "via_sizes": vinfo, "reference": pr, "careless": pc,
        "board_setup": {"min_via_diameter": MIN_VIA_D, "min_through_hole": MIN_DRILL, "min_annular": MIN_ANNULAR,
                        "class_via": list(VIA_CLASS)},
        "interpretation": {
            "rule": "drill = smallest value >= Board Setup minimum drill with pi x drill >= the trace width it carries; "
                    "diameter = drill + 2 x minimum annular ring, but not below the minimum via diameter",
            "grid": "drill rounded UP to 0.01 mm (0.3183 -> 0.32, 0.6366 -> 0.64)"},
        "judgement_calls": [
            "drill grid 0.01 mm is mine (a fab usually offers 0.05 steps: 0.35 and 0.65); configurable",
            "one bigger via per net is expected; several minimum vias in parallel (sum of circumferences, power-via rule 2) "
            "would also carry 1.0 mm (2 x 0.785 = 1.57) - not drawn, the 5.4 text asks for 'the smallest drill with pi x "
            "drill >= width'",
            "each net has one pad on F.Cu and one on B.Cu, so exactly one via is topologically required per net; the via "
            "sits mid-row (any x clear of the pads gives the same length)",
            "the 2.0 mm HEAVY trace is an extreme to show the rule scales; a via annulus narrower than the trace is accepted "
            "(the circumference criterion, not the diameter, is the rule)"],
        "literature": [LIT["power_via"]],
    }
    return VPat("via_size_rule", "Via size from the Board Setup, not from the net class", (W, H), fps, conns, ref, car,
                "3 vias, one per net, sizes dia/drill: SIG (0.2 mm) 0.45/0.25, PWR (1.0 mm) 0.52/0.32, HEAVY (2.0 mm) 0.84/0.64. "
                f"Same copper length {pr['length_mm']:.1f} mm as the careless answer; the careless vias are all the class 0.80/0.40.",
                "Board Setup minimum 0.45/0.25 (annular 0.10): pi x 0.25 = 0.785 carries 0.2 mm; 1.0 mm needs drill >= 0.318 -> "
                "0.32 (dia 0.52); 2.0 mm needs >= 0.637 -> 0.64 (dia 0.84). The net class via 0.80/0.40 is deliberately "
                "different: a router that uses it is wrong for SIG (too big) and for HEAVY (too small: pi x 0.40 = 1.26 < 2.0).",
                exp, classes(cls_extra), net_patterns=pats,
                zooms=[((10.5, 1.0, 15.5, 5.0), "SIG via"), ((10.5, 7.5, 15.5, 11.5), "PWR via"),
                       ((10.5, 14.0, 15.5, 19.0), "HEAVY via")],
                colors={"SIG": PALETTE[0], "PWR": PALETTE[1], "HEAVY": PALETTE[2]})


BUILDERS = {"via_pair_redundant": p_redundant, "via_pair_slide": p_slide, "via_force_relocate": p_force,
            "branch_point_relocate": p_branch, "branch_point_into_pad": p_branch_pad, "via_size_rule": p_size}
NAMES = list(BUILDERS)


def build(name: str) -> VPat:
    pat = BUILDERS[name]()
    pat.expected["built"] = check(pat)
    return pat


# ----------------------------------------------------------------------------------------------- board files
def write_pat(view, path: Path, *, copper: bool) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = build_board(view, copper=copper)
    for p in b.GetPads():  # back-side SMD pads: copper, mask and paste on B
        if p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD and p.IsOnLayer(pcbnew.B_Cu):
            ls = pcbnew.LSET()
            for ly in (pcbnew.B_Cu, pcbnew.B_Mask, pcbnew.B_Paste):
                ls.AddLayer(ly)
            p.SetLayerSet(ls)
    pcbnew.SaveBoard(str(path), b)
    write_project_file(path, rules=view.rules, classes=view.classes, patterns=view.net_patterns)
    return path


def drc_of(path: Path) -> dict:
    from harness.drc import run_drc
    from harness.paths import kicad_cli

    if not kicad_cli().exists():
        return {"available": False}
    r = run_drc(path, path.with_name("drc.json"))
    return {"available": True, "violations": len(r.violations), "unconnected": len(r.unconnected),
            "types": sorted({v.type for v in r.violations})}


# ----------------------------------------------------------------------------------------------- png
def _poly(geom):
    return list(geom.exterior.coords)


def draw_board(ax, pat: VPat, sol: Sol | None, *, ratsnest=False, window=None, title="", labels=True):
    from matplotlib.patches import Circle, Polygon as MPoly, Rectangle

    W, H = pat.size
    ax.add_patch(Rectangle((0, 0), W, H, fc="#fbfbf6", ec="k", lw=1.0, zorder=0))
    for x0, y0, x1, y1 in ((0, 0, W, EDGE), (0, H - EDGE, W, H), (0, EDGE, EDGE, H - EDGE), (W - EDGE, EDGE, W, H - EDGE)):
        ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="#f3d9d9", ec="#c06060", hatch="xxxx", lw=0.3, zorder=0.7))
    col = lambda n: pat.colors.get(n, GREY)
    for f in pat.fps:
        for p in f.pads:
            back = p.layer == "B.Cu"
            g = pad_geo(p)
            ax.add_patch(MPoly(_poly(g), closed=True, fc=col(p.net), ec="k" if not back else col(p.net), lw=0.4 if not back else 0.8,
                               ls="-" if not back else (0, (2, 1)), alpha=0.85 if not back else 0.45,
                               hatch="////" if back else None, zorder=2 if not back else 1.5))
            if labels:
                long = p.size[1] > 3 * p.size[0]
                ref = f"{f.ref}  {p.net}" + (" (B.Cu)" if back else " (F.Cu)")
                if long:
                    ax.text(p.pos[0], p.pos[1] - p.size[1] / 2 + 0.7, ref, fontsize=5.5, ha="center", va="top", rotation=90,
                            color="#222222", zorder=6)
                else:
                    ax.text(p.pos[0], p.pos[1] + p.size[1] / 2 + 0.2, ref, fontsize=5.5, ha="center", va="top",
                            color="#222222", zorder=6)
    if sol is not None:
        for layer in ("B.Cu", "F.Cu"):
            for t in sol.tracks:
                if t.layer != layer:
                    continue
                g = LineString(t.pts).buffer(t.width / 2, cap_style="round", join_style="round", quad_segs=8)
                top = layer == "F.Cu"
                for geom in getattr(g, "geoms", [g]):
                    ax.add_patch(MPoly(_poly(geom), closed=True, fc=col(t.net), ec="k" if top else col(t.net),
                                       lw=0.25 if top else 0.7, ls="-" if top else (0, (2, 1)), alpha=0.85 if top else 0.45,
                                       hatch=None if top else "////", zorder=4.5 if top else 4))
        for i, v in enumerate(sol.vias, 1):
            ax.add_patch(Circle(v.pos, v.dia / 2, fc=col(v.net), ec="k", lw=0.5, zorder=5))
            ax.add_patch(Circle(v.pos, v.drill / 2, fc="white", ec="k", lw=0.25, zorder=5.1))
            if labels:
                ax.text(v.pos[0] + v.dia / 2 + 0.12, v.pos[1] - v.dia / 2 - 0.1, f"V{i} {v.dia:g}/{v.drill:g}", fontsize=5.5,
                        color="k", ha="left", va="bottom", zorder=9, bbox=dict(fc="#fff6c8", ec="none", pad=0.1, alpha=0.9))
        for nt in sol.notes:
            if "p" in nt:
                ax.annotate("", xy=nt["q"], xytext=nt["p"], arrowprops=dict(arrowstyle="<->", lw=0.6, color="#7a3db8"), zorder=8)
                ax.text((nt["p"][0] + nt["q"][0]) / 2, nt["p"][1] + 0.1, nt["text"], fontsize=5.5, color="#7a3db8", ha="center",
                        va="top", zorder=9)
            else:
                ax.text(nt["pt"][0], nt["pt"][1], nt["text"], fontsize=6, color=nt.get("color", BAD), zorder=9)
    if ratsnest:
        pos = {f"{f.ref}.{p.num}": p.pos for f in pat.fps for p in f.pads}
        for net, a, b in pat.connections:
            ax.plot(*zip(pos[a], pos[b]), ls=(0, (2, 2)), lw=0.9, color=col(net), zorder=3)
    if window:
        ax.set_xlim(window[0], window[2])
        ax.set_ylim(window[3], window[1])
    else:
        ax.set_xlim(-0.4, W + 0.4)
        ax.set_ylim(H + 0.4, -0.5)
    ax.set_aspect("equal")
    ax.grid(True, lw=0.3, color="#dddddd")
    ax.tick_params(labelsize=6)
    ax.set_title(title, fontsize=8.5, loc="left")


def _legend(fig, rect_y, pat):
    from matplotlib.lines import Line2D
    from matplotlib.patches import Patch

    h = [Patch(fc=c, ec="k", label=f"net {n}") for n, c in pat.colors.items()]
    h += [Patch(fc="#888888", ec="k", alpha=0.85, label="F.Cu copper (solid, black outline)"),
          Patch(fc="#888888", ec="#888888", alpha=0.45, hatch="////", label="B.Cu copper (hatched, dashed outline)"),
          Line2D([], [], marker="o", ls="", mfc="#888888", mec="k", label="via, dia/drill (white drill), Vn"),
          Line2D([], [], ls=(0, (2, 2)), color="#444444", lw=0.9, label="ratsnest (problem)"),
          Patch(fc="#f3d9d9", ec="#c06060", hatch="xxxx", label="copper-to-edge band 0.50: no copper"),
          Line2D([], [], ls="", marker="", label="strip = one long fixed pad, no connection of its own")]
    fig.legend(handles=h, loc="lower center", ncol=3, fontsize=6.3, frameon=True, bbox_to_anchor=(0.5, rect_y))


def render(pat: VPat, doc: dict, out: Path) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    W, H = pat.size
    pw = 9.4
    ph = pw * (H + 1.0) / (W + 0.8)
    top, leg_h, foot_h, gap = 0.5, 1.1, 1.3, 0.15
    fh = top + 3 * (ph + gap) + leg_h + foot_h
    fig = plt.figure(figsize=(pw + 0.4, fh))
    axs = [fig.add_axes([0.05, 1 - (top + (k + 1) * (ph + gap) - gap * 0.5) / fh, 0.9, (ph - 0.45) / fh]) for k in range(3)]
    draw_board(axs[0], pat, None, ratsnest=True, title="problem: fixed pads, ratsnest (dashed), nothing routed")
    pr, pc = doc["expected"]["reference"], doc["expected"]["careless"]
    dr = doc["kicad"]["reference_drc"]
    draw_board(axs[1], pat, pat.ref, title=f"EXPECTED = reference copper (KiCad DRC {dr['violations']} violations / {dr['unconnected']} "
               f"unconnected): {pat.ref.label}\n{pr['vias']} vias, {pr['length_mm']:.2f} mm, price {pr['price']:.1f} mm")
    dc = doc["kicad"]["careless_drc"]
    draw_board(axs[2], pat, pat.careless, title=f"CARELESS (ruled out; DRC {dc['violations']} / {dc['unconnected']}): {pat.careless.label}\n"
               f"{pc['vias']} vias, {pc['length_mm']:.2f} mm, price {pc['price']:.1f} mm")
    _legend(fig, (foot_h + 0.05) / fh, pat)
    fig.suptitle(f"{pat.name}: {pat.title}", fontsize=11, y=1 - 0.15 / fh)
    fig.text(0.02, 0.005, "\n".join(textwrap.wrap("Expected: " + pat.answer, 175)[:9]), fontsize=6.6, ha="left", va="bottom")
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=120)
    plt.close(fig)
    return out


def render_zoom(pat: VPat, out: Path) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    n = len(pat.zooms)
    fig, axs = plt.subplots(2, n, figsize=(6.2 * n, 11.0), squeeze=False)
    for k, (win, label) in enumerate(pat.zooms):
        draw_board(axs[0][k], pat, pat.ref, window=win, title="EXPECTED: " + label)
        draw_board(axs[1][k], pat, pat.careless, window=win, title="careless: " + label)
    fig.suptitle(f"{pat.name}: zoom (mm)", fontsize=10)
    fig.tight_layout()
    fig.savefig(out, dpi=110)
    plt.close(fig)
    return out


# ----------------------------------------------------------------------------------------------- generate
def generate(name: str, out: Path, *, png: bool = True, drc: bool = True) -> dict:
    pat = build(name)
    d = Path(out) / name
    paths = {"problem": write_pat(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_pat(pat, d / "reference" / f"{name}.kicad_pcb", copper=True),
             "careless": write_pat(_View(pat, pat.careless), d / "careless" / f"{name}.kicad_pcb", copper=True)}
    nconn = len(pat.connections)
    kicad = {"problem_drc": {"violations": 0, "unconnected": nconn}, "reference_drc": {"violations": 0, "unconnected": 0},
             "careless_drc": {"violations": 0, "unconnected": 0}}
    if drc:
        for key, p in (("problem_drc", paths["problem"]), ("reference_drc", paths["reference"]),
                       ("careless_drc", paths["careless"])):
            r = drc_of(p)
            if r.get("available"):
                kicad[key] = {k: r[k] for k in ("violations", "unconnected", "types")}
    pins = {f"{f.ref}.{p.num}": {"net": p.net, "pos": rp(p.pos), "layer": p.layer, "size": list(p.size)}
            for f in pat.fps for p in f.pads}

    def sol_doc(s):
        return {"label": s.label, "tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": [rp(p) for p in t.pts]}
                                             for t in s.tracks],
                "vias": [{"net": v.net, "pos": rp(v.pos), "dia": v.dia, "drill": v.drill} for v in s.vias]}

    doc = {"name": name, "title": pat.title, "expected_summary": pat.answer, "why": pat.why,
           "board": {"size_mm": list(pat.size), "copper_layers": 2},
           "rules": {"clearance": CLEAR, "track": W_NORMAL, "class_via": list(VIA_CLASS),
                     "board_setup": {"min_track": 0.2, "min_via_dia": MIN_VIA_D, "min_drill": MIN_DRILL, "min_annular": MIN_ANNULAR,
                                     "copper_to_edge": EDGE}, "via_price_mm": VIA_PRICE},
           "pins": pins, "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "expected": pat.expected, "reference": sol_doc(pat.ref), "careless": sol_doc(pat.careless), "kicad": kicad,
           "topor": "see topor50/ (filled by the TopoR run, if any)", "rules_assumed": RULES_ASSUMED}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        paths["png"] = render(pat, doc, d / f"{name}.png")
        paths["zoom"] = render_zoom(pat, d / f"{name}_zoom.png")
    return paths


TOPOR_VERDICT = {
    "via_pair_redundant": "AGREES on topology. Autoroute (defaults, ~40 s): 0 vias, A on F.Cu round the strip's lower end like the "
                          "expected answer; any-angle taut wire, centre line 14.96 mm (expected octilinear 15.95; TopoR's table "
                          "says 16.72), DRC 0 / 0. FreeStyle Auto move (F9) on the careless 2-via hop: both vias stay (14.0 mm "
                          "kept): Auto move does NOT delete the redundant pair, the autorouter simply never builds it.",
    "via_pair_slide": "AGREES on count. Autoroute: 2 vias (class 0.8 / 0.4), one B.Cu stretch under both strips, 18.10 mm (the two "
                      "vias sit 0.2-0.7 mm off the straight line: 0.10 mm longer than expected), DRC 0 / 0. FreeStyle Auto move on "
                      "the careless 4 vias: all 4 stay at their places, no merge.",
    "via_force_relocate": "DISAGREES on positions. Autoroute: 2 vias at x 8.09 and 13.47 (expected 9.0 and 14.0 +-0.25); with its "
                          "0.8 vias the gaps are 1.29 / 3.1 and 0.67 / 1.73 (not equal). FreeStyle Auto move on the squeezed "
                          "careless vias (6.875, 12.875) moves them to 9.87 and 14.9 (gaps 3.2 / 1.5 and 2.3 / 0.48): it leaves "
                          "the squeeze (min gap 0.25 -> 0.48) but overshoots, no equal clearance. Length 18.0 mm in all runs.",
    "branch_point_relocate": "NEAR. Autoroute builds a free T at (6.07, 7.40), 22.87 mm (expected Fermat (5.89, 7.0), 22.66, +0.9 %). "
                             "Auto move on the careless T at (3, 7) (24.0 mm) moves it to (5.59, 7.0), 22.81 mm (+0.7 %, 0.30 mm "
                             "from the Fermat point, outside the 0.15 tolerance). DRC 0 / 0.",
    "branch_point_into_pad": "AGREES. Autoroute: chain A-M-V through the pad, no free junction, 15.44 mm (expected 15.23, +1.4 %), DRC 0 / 0.",
    "via_size_rule": "DISAGREES (as expected, it cannot know the rule). Autoroute: one via per net, all 0.8 / 0.4 = the net class via "
                     "from the DSN (expected 0.45/0.25, 0.52/0.32, 0.84/0.64); the DSN has no 'Board Setup minimum via'. The vias "
                     "sit 0.4-1 mm off the straight row (wires 17.05 / 17.18 / 17.35 mm vs 17.0). DRC 0 / 0.",
}


def readme(out: Path, docs: dict[str, dict]) -> Path:
    L = ["# Via and junction test patterns (shopping list 5.4)", "",
         "Generated by `python -m harness.via_patterns` (module docstring: rules, strips, price). Each pattern: `problem/` "
         "(pads only), `reference/` (expected copper), `careless/` (ruled-out copper, real), `expected.json`, `<name>.png` "
         "(problem | expected | careless), `<name>_zoom.png`, `topor50/` (TopoR Lite 5.0 black-box run, if any).", "",
         "## Rules (every `.kicad_pro`)", "",
         "- Net class Default: clearance 0.20, track 0.25, via 0.80 / 0.40 (deliberately NOT the expected via).",
         "- Board Setup minimums (= supplier capability): track 0.20, clearance 0.20, via diameter 0.45, drill 0.25, annular "
         "ring 0.10, hole to hole 0.25, copper to edge 0.50.",
         "- Expected via = smallest allowed with pi x drill >= trace width: 0.45 / 0.25 for 0.25 mm; via_size_rule shows larger.",
         "- Price = mm + 25 mm per via. 2 layers; F.Cu solid, B.Cu hatched; obstacles are fixed strip pads (no hidden keep-outs).",
         "- `optional 6 via_shared_gnd` NOT built: duplicates plane_via_3r (scratch/plane-via-pattern).", ""]
    judg = []
    for n, d in docs.items():
        e = d["expected"]
        k = d["kicad"]
        L += [f"## {n}: {d['title']}", "", f"![{n}]({n}/{n}.png)", "", f"Zoom: `{n}/{n}_zoom.png`", "",
              f"**Expected.** {d['expected_summary']}", "", f"**Why.** {d['why']}", "",
              f"**KiCad DRC.** problem {k['problem_drc']}, reference {k['reference_drc']}, careless {k['careless_drc']}", ""]
        tp = Path(out) / n / "topor50" / "result.json"
        if tp.exists():
            r = json.loads(tp.read_text())
            L += [f"**TopoR Lite 5.0** ([`topor50/{n}_topor.png`]({n}/topor50/{n}_topor.png); session DSN, KiCad copy and `result.json` "
                  f"beside it): {len(r['vias'])} vias, {r['total_mm']:.2f} mm, KiCad DRC {r['kicad_drc'].get('violations')} / "
                  f"{r['kicad_drc'].get('unconnected')}. {TOPOR_VERDICT[n]}", ""]
        for j in e.get("judgement_calls", []):
            judg.append(f"- {n}: {j}")
    L += ["## TopoR method (black box)", "",
          "Problem board -> `pcbnew.ExportSpecctraDSN` -> `scratch/untangle-topor50/conv_dsn.py` -> import in TopoR Lite 5.0.108 (Wine/Xephyr "
          "sandbox) -> autoroute defaults ~40 s (`topor50/`) -> File > Export > Specctra session -> `harness/via_patterns_topor.py` "
          "(copper added to a copy of the problem board, KiCad DRC, plot). `topor50_freestyle_fs0` / `_auto*`: the careless copper "
          "pre-routed in the DSN, FreeStyle (F8) before and after Auto move (F9). Class via 0.8/0.4 is what TopoR uses (session "
          "files carry no drill: drill = half the diameter for 0.8, 0.25 for 0.45). Strips are ordinary pads (rect, no keep-outs). "
          "TopoR's own table length is larger than the exported centre lines (known). Not run: via_shared_gnd (duplicate of plane_via_3r).", ""]
    L += ["## Judgement calls needing the user", ""] + judg + ["", "## Literature", ""]
    seen = []
    for d in docs.values():
        for x in d["expected"]["literature"]:
            if x not in seen:
                seen.append(x)
    L += [f"- {x}" for x in seen]
    p = Path(out) / "README.md"
    p.write_text("\n".join(L) + "\n")
    return p


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "via-patterns"
    docs = {}
    for n in NAMES:
        g = generate(n, out)
        docs[n] = json.loads(g["expected"].read_text())
        print(n, docs[n]["kicad"])
    print(readme(out, docs))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
