"""BGA fan-out test patterns (user 2026-11-12): small 4-layer boards with one 6 x 6 BGA and a row of through-hole
destination pads on each side, so every ball has somewhere to go. Each pattern has a known optimal fan-out.

Four patterns, all 4 layers, stackup F.Cu signal / In1.Cu GND plane / In2.Cu power plane / B.Cu signal (topor's
default roles for 4 copper layers, `plugin/topor/layers.py` DEFAULTS_BY_COUNT; KiCad layer types left "signal"):

  bga_one_layer   1.0 mm pitch, 0.50 mm pads, 0.15 track / 0.15 clearance: ONE track fits between two balls. A 6 x 6
                  array then escapes completely on F.Cu: 0 vias (Luzin & Polubasov 2008, PDF p. 1: 4(n-1) channels >=
                  (n-2)^2 inner balls holds up to n = 6).
  bga_plane_balls The same array; the 4 centre balls are GND and the 4 corners of ring 1 are VCC, with a GND plane
                  on In1.Cu and a VCC plane on In2.Cu. Every plane ball gets its own via in the cell next to it (one
                  stub, the plane is the terminal, no trace on the plane layer); the 28 signal balls escape on F.Cu.
  bga_dogbone     0.8 mm pitch, 0.40 mm pads, 0.15 / 0.15: NO track fits between two balls (0.40 < 0.45), so ring 0
                  escapes on F.Cu and each of the 16 inner balls dives through its own via at the open-space centre on
                  its outward diagonal (dog-bone) and escapes on B.Cu. No track fits between two vias at one pitch
                  either, so each quadrant's innermost via leaves through the free centre cross of cells.
  bga_neckdown    bga_dogbone's board plus a .kicad_dru 'BGA neckdown' rule (clearance 0.10 inside U1's courtyard):
                  one track per channel again, so everything escapes on F.Cu with 0 vias.

The user's BGA rules (2026-11-12, `RULES_TEXT`): R1 no trace through / near another ball's via, R2 dog-bone via at
the open-space centre, R3 dog-bones point outward per quadrant, R4 one via per plane ball, R5 via-in-pad only as an
option, R6 courtyard custom rules (neckdown), R7 no signal on the plane layers. `rule_faults()` checks them on any
copper; every expected answer has none (asserted), a routed board's faults are drawn and listed by rule.

Every board is SOLVABLE BY CONSTRUCTION: the reference copper is built first from centre lines, `check()` verifies it
with shapely (clearance per layer, hole clearance, copper to edge, connectivity), then KiCad DRC measures it.
Nothing comes from the engine. `alternative` copper (where drawn) is a real, DRC-checked layout that the expected
answer rules out, with the reason.

Construction: the top side is built in the BGA frame and rotated by 90 degrees three times (every side is the same
up to rotation). Destination pads: PTH 0.6 / 0.3 at 1.0 mm pitch (connect on every layer, so the pad row never
forces a via), in the order of the escape lines, so no two wires of one layer cross. Fan-out: all wires of a side
turn 45 degrees at one line and straighten at their pad column.

`python -m harness.bga_patterns [OUT]` writes every pattern under OUT (default `scratch/bga-patterns`): problem/
(pads, planes, nothing routed), reference/ (expected copper), alternative/ (if any), expected.json, `<name>.png`,
README.md. `python -m harness.bga_patterns analyse <name> <routed.kicad_pcb> <out.png> [label]` compares a routed board
with the expected answer (escape layer per ball, vias, via cells, length per net) and draws it.
Coordinates in mm, KiCad frame (y down). Deterministic.
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

from harness.fit_patterns import LAYER_IDS, Fp, Pad, Track, Via, build_board
from harness.fixtures import write_project_file

MM = 1_000_000
SQ2 = math.sqrt(2.0)
EDGE = 0.5
LAYERS4 = ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]
VIA_PRICE = 25.0
STAGGER = 0.10  # mm between the 45-degree turns of neighbouring fan-out wires (side_routes)
VIA_CENTRE_TOL = 0.05  # mm: rule R2 -- a BGA via further than this from the open-space (cell) centre is "off centre"
DEST_PITCH, DEST_PAD, DEST_DRILL = 1.0, 0.6, 0.3
ROWS = "ABCDEFGH"
LAYER_COL = {"F.Cu": "#c83434", "B.Cu": "#2f5fd0", "In1.Cu": "#5f8f2a", "In2.Cu": "#a050c0"}
ROLE_COL = {"ring0": "#e07b39", "ring1": "#d4a017", "ring2": "#8e44ad", "GND": "#3d8b3d", "VCC": "#b0306a"}
BAD = "#e000e0"

LIT = {
    "luzin_p1": "Luzin & Polubasov 2008, 'TopoR: routing PCBs with BGA components' (docs/literature/luzin2008-topor-bga.pdf), "
                "PDF p. 1 (journal p. 44): an n x n array has 4(n-1) channels on its periphery and (n-2)^2 inner balls; "
                "one layer suffices while 4(n-1) >= (n-2)^2, i.e. up to n = 6 (at n = 7 one channel is missing); a "
                "fanout via guarantees no blocking with enough layers, but not on one layer",
    "luzin_p2": "Luzin & Polubasov 2008, PDF p. 2 (journal p. 46): pitch 1 mm, pad 0.5 mm, track 0.15 mm, gap 0.15 mm -> one "
                "track passes between two balls; outside-in, each layer takes two perimeters plus four inner balls; "
                "balls tied to ground / power planes by vias need not be brought to the periphery",
    "luzin_p3": "Luzin & Polubasov 2008, PDF p. 3 (journal p. 47): power and ground balls in the centre of the package "
                "simplify routing (fig. 7)",
    "luzin_p4": "Luzin & Polubasov 2008, PDF p. 4 (journal p. 48): TopoR's BGA model, fig. 9: a via cell is the area between "
                "four neighbouring balls, every used ball except the first row has its own cell; requirements: avoid "
                "jumpers between neighbouring balls, every ball (except the outer rows) its own via rather than another "
                "ball's via of the same net, vias outside the package where possible; lengthening the piece from ball to "
                "via is very undesirable because it can block neighbouring balls",
    "topor50": "TopoR 5.0 help (docs/literature/eremex-topor50-help.txt, page 2/bga.htm and 4/bga.htm): BGA components are "
               "recognised; the smallest via is chosen automatically; one rule for the pad layer and one for the other "
               "layers; warning 3005 when a via does not fit between the pads; rectangular grids only",
    "topor63": "TopoR 6.3 User Manual (docs/literature/eremex2017-topor63-user-manual.pdf) p. 75 'Routing BGA Components' "
               "(same rules as 5.0) and p. 65 (nets to internal plane layers get fanouts only)",
    "haas2026": "Haas 2026, BGA memory bus (docs/literature/haas2026-bga-memory-bus.pdf) PDF p. 2: the fanout engine offers "
                "every ball a stub on its layer or a dog-bone via to the other layer",
    "kicad_dru": "KiCad 10 PCB Editor manual, 'Custom design rules' (Board Setup -> Design Rules -> Custom Rules; "
                 "the neck-down example uses A.intersectsCourtyard('<ref>') with track_width / clearance constraints); "
                 "conditions are tried for (A, B) and (B, A) (pcbnew/drc/drc_rule_condition.cpp EvaluateFor)",
    "planes": "memory planes-are-terminals: an own-net plane is a terminal like a pad, vias on it are always allowed",
    "power_via": "memory power-via-rules: smallest Board Setup via with pi x drill >= trace width",
}


def r4(x):
    return round(float(x), 4)


def rp(p):
    return [r4(p[0]), r4(p[1])]


def plen(pts):
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def dedup(pts):
    out = []
    for p in pts:
        p = (r4(p[0]), r4(p[1]))
        if out and math.dist(out[-1], p) < 1e-6:
            continue
        out.append(p)
    # drop collinear middle points
    k = 1
    while k < len(out) - 1:
        a, b, c = out[k - 1], out[k], out[k + 1]
        if abs((b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])) < 1e-9:
            out.pop(k)
        else:
            k += 1
    return out


# ----------------------------------------------------------------------------------------------- model
@dataclass
class Rules:
    pitch: float
    pad: float
    track: float
    clear: float
    via: tuple[float, float]  # (diameter, drill) = Board Setup minimum = net class via
    source: str = ""

    def kicad(self) -> dict:
        return {"min_clearance": self.clear, "min_track_width": self.track, "min_copper_edge_clearance": EDGE,
                "min_hole_clearance": 0.2, "min_hole_to_hole": 0.25, "min_via_diameter": self.via[0],
                "min_through_hole_diameter": self.via[1], "min_via_annular_width": r4((self.via[0] - self.via[1]) / 2),
                "max_error": 0.005}

    def classes(self) -> list[dict]:
        return [{"name": "Default", "clearance": self.clear, "track_width": self.track, "via_diameter": self.via[0],
                 "via_drill": self.via[1], "priority": 2147483647}]

    def channel(self) -> dict:
        """Tracks that fit between two balls (pad gap) and between two vias at one pitch, and the via-in-cell gap."""
        need1 = self.track + 2 * self.clear
        gap = self.pitch - self.pad
        vgap = self.pitch - self.via[0]
        cell = self.pitch / SQ2 - self.pad / 2 - self.via[0] / 2
        per = lambda g: max(0, math.floor((g - self.clear + 1e-9) / (self.track + self.clear))) if g >= need1 - 1e-9 else 0
        return {"ball_gap": r4(gap), "one_track_needs": r4(need1), "tracks_between_balls": per(gap),
                "via_gap": r4(vgap), "tracks_between_vias": per(vgap), "via_in_cell_gap": r4(cell),
                "via_fits_cell": cell >= self.clear - 1e-9}


@dataclass
class Sol:
    label: str
    tracks: list[Track]
    vias: list[Via]
    why: str = ""


@dataclass
class Item:
    ball: tuple[int, int]  # (col, row) in the top frame
    kind: str  # straight | channel | dogbone | plane
    cell: tuple[float, float] | None = None  # cell centre index, e.g. (0.5, 0.5)
    layer: str = "F.Cu"  # escape layer (dogbone)
    net: str | None = None  # plane net (GND / VCC); signal nets are the ball names
    bpath: tuple = ()  # dogbone: cell-index waypoints on the escape layer after the via, before it turns outward


@dataclass
class BPat:
    name: str
    title: str
    rules: Rules
    n: int
    top: list[Item]  # the top side's balls; the other sides are rotations
    zones: list[tuple[str, str]] = field(default_factory=list)  # (net, layer) full-board planes
    alt_top: list[Item] | None = None
    alt_label: str = ""
    alt_why: str = ""
    alt_rules: str = ""  # which of the user's BGA rules the alternative breaks
    dru: str = ""  # .kicad_dru text (custom rules), "" = none
    region_clear: float | None = None  # clearance inside U1's courtyard (the .kicad_dru neckdown rule)
    answer: str = ""
    why: str = ""
    judgement: list[str] = field(default_factory=list)
    literature: list[str] = field(default_factory=list)
    # filled by build()
    size: tuple[float, float] = (0, 0)
    centre: tuple[float, float] = (0, 0)
    fps: list[Fp] = field(default_factory=list)
    connections: list[tuple[str, str, str]] = field(default_factory=list)
    ref: Sol | None = None
    alt: Sol | None = None
    roles: dict = field(default_factory=dict)  # ball name -> role
    expect: dict = field(default_factory=dict)  # ball name -> {"layer": escape layer or None, "vias": n, "cell": [x, y]}
    layers: int = 4
    keepouts: list = field(default_factory=list)
    ref_tracks: list = field(default_factory=list)
    ref_vias: list = field(default_factory=list)

    @property
    def nets(self):
        out = []
        for f in self.fps:
            for p in f.pads:
                if p.net and p.net not in out:
                    out.append(p.net)
        return out

    def ball_xy(self, c, r):
        o = (self.n - 1) / 2
        return (self.centre[0] + (c - o) * self.rules.pitch, self.centre[1] + (r - o) * self.rules.pitch)

    def cell_xy(self, cx, cy):
        return self.ball_xy(cx, cy)

    def ball_name(self, c, r):
        return f"{ROWS[r]}{c + 1}"

    def bga_box(self):
        x0, y0 = self.ball_xy(0, 0)
        x1, y1 = self.ball_xy(self.n - 1, self.n - 1)
        h = self.rules.pitch / 2
        return (x0 - h, y0 - h, x1 + h, y1 + h)

    def channel(self) -> dict:
        """Rules.channel() with the courtyard rule's clearance inside the BGA (bga_neckdown), else the net class."""
        if self.region_clear is None:
            return self.rules.channel()
        r = Rules(self.rules.pitch, self.rules.pad, self.rules.track, self.region_clear, self.rules.via)
        return dict(r.channel(), courtyard_clearance=self.region_clear)

    def courtyard(self):
        """U1's courtyard (F.CrtYd rectangle): the BGA box (half a pitch around the outer ball centres)."""
        return box(*self.bga_box())

    def clear_for(self, g1, g2=None) -> float:
        """Required clearance between two copper shapes: the courtyard rule's value when either touches U1's
        courtyard (KiCad tries a custom rule's condition for (A, B) and (B, A)), else the net class value."""
        if self.region_clear is not None:
            cy = self.courtyard()
            if g1.intersects(cy) or (g2 is not None and g2.intersects(cy)):
                return self.region_clear
        return self.rules.clear


class _View:
    def __init__(self, pat: BPat, sol: Sol | None):
        self._p = pat
        self.ref_tracks = sol.tracks if sol else []
        self.ref_vias = sol.vias if sol else []

    def __getattr__(self, k):
        return getattr(self._p, k)


def rot_ball(c, r, n, s):
    """Ball / cell index (c, r) rotated s times by 90 degrees clockwise (screen) about the array centre."""
    o = (n - 1) / 2
    x, y = c - o, r - o
    for _ in range(s % 4):
        x, y = -y, x
    return (x + o, y + o)


def rot_pt(p, ctr, s):
    x, y = p[0] - ctr[0], p[1] - ctr[1]
    for _ in range(s % 4):
        x, y = -y, x
    return (ctr[0] + x, ctr[1] + y)


def ring_of(c, r, n):
    return int(min(c, r, n - 1 - c, n - 1 - r))


# ----------------------------------------------------------------------------------------------- construction
def side_routes(pat: BPat, items: list[Item]):
    """Top-side routes in board coordinates: [(item, segs[(layer, pts)], vias[pos], esc (layer, x) | None)] plus the
    fan-out to the destination row; returns (routes, dest_pads[(x, y)] in escape order, y_dest)."""
    R = pat.rules
    y_row0 = pat.ball_xy(0, 0)[1]
    y_s = r4(y_row0 - R.pad / 2 - R.clear - R.track / 2 - 0.25)  # common 45-degree turn line
    routes = []
    for it in items:
        b = pat.ball_xy(*it.ball)
        if it.kind == "straight":
            routes.append([it, [["F.Cu", [b, (b[0], y_s)]]], [], ("F.Cu", b[0])])
            continue
        cpos = pat.cell_xy(*it.cell)
        if it.kind == "channel":
            routes.append([it, [["F.Cu", [b, cpos, (cpos[0], y_s)]]], [], ("F.Cu", cpos[0])])
        elif it.kind == "dogbone":
            way = [pat.cell_xy(*q) for q in it.bpath]
            xe = way[-1][0] if way else cpos[0]
            routes.append([it, [["F.Cu", [b, cpos]], [it.layer, [cpos, *way, (xe, y_s)]]], [cpos], (it.layer, xe)])
        elif it.kind == "plane":
            routes.append([it, [["F.Cu", [b, cpos]]], [cpos], None])
        else:
            raise ValueError(it.kind)
    esc = sorted([rt for rt in routes if rt[3] is not None], key=lambda rt: rt[3][1])
    xs = [rt[3][1] for rt in esc]
    mid = (min(xs) + max(xs)) / 2
    m = len(esc)
    dx = [mid + (k - (m - 1) / 2) * DEST_PITCH for k in range(m)]
    # staggered 45-degree turns: of the wires of one layer that move the same way, the one moving furthest turns
    # first and each next one STAGGER later, so neighbours at half a pitch (0.40 mm at 0.8 mm pitch) are
    # 0.40 + STAGGER apart when both run at 45 degrees (perpendicular gap (0.40 + 0.10) / sqrt 2 - track >=
    # clearance); the length of every wire is unchanged (the turn only moves along its own line)
    y_turn = [y_s] * m
    hmin = (R.track + R.clear) * SQ2 + 0.005  # horizontal offset two parallel 45-degree wires need
    for ly in {rt[3][0] for rt in esc}:
        for sgn in (-1, 1):
            grp = [k for k in range(m) if esc[k][3][0] == ly and (dx[k] - xs[k]) * sgn > 1e-9]
            grp.sort(key=lambda k: -abs(dx[k] - xs[k]))
            for p_, k in zip(grp, grp[1:]):
                h = abs(xs[k] - xs[p_])
                y_turn[k] = r4(y_turn[p_] - (max(STAGGER, hmin - h) if h < hmin - 1e-9 else 0.0))
    y_d = r4(min(y_turn[k] - abs(dx[k] - xs[k]) for k in range(m)) - DEST_PAD / 2 - R.clear - 0.45)
    pads = []
    for k, rt in enumerate(esc):
        x_e = rt[3][1]
        x_d = dx[k]
        seg = rt[1][-1]
        seg[1] = dedup(seg[1] + [(x_e, y_turn[k]), (x_d, y_turn[k] - abs(x_d - x_e)), (x_d, y_d)])
        pads.append((x_d, y_d))
        rt.append(k)
    return routes, pads, y_d


def build_sol(pat: BPat, items: list[Item], label: str, why: str = "", *, register: bool = False) -> Sol:
    """Copper of all four sides from the top side's items; with `register`, also the footprints / pins / roles."""
    R = pat.rules
    n = pat.n
    routes, pads, y_d = side_routes(pat, items)
    tracks, vias = [], []
    ctr = pat.centre
    dests = []
    for s in range(4):
        for it, segs, vs, esc, *k in routes:
            c, r = rot_ball(*it.ball, n, s)
            c, r = int(round(c)), int(round(r))
            name = pat.ball_name(c, r)
            net = it.net or name
            for ly, pts in segs:
                tracks.append(Track(net, ly, R.track, dedup([rot_pt(q, ctr, s) for q in pts])))
            for v in vs:
                q = tuple(r4(t) for t in rot_pt(v, ctr, s))
                if not any(w.net == net and math.dist(w.pos, q) < 1e-6 for w in vias):  # a shared via is one via
                    vias.append(Via(net, q, R.via[0], R.via[1]))
            if register:
                ring = ring_of(c, r, n)
                pat.roles[name] = it.net or f"ring{ring}"
                cell = None
                if it.cell is not None:
                    cc = rot_ball(*it.cell, n, s)
                    cell = [cc[0], cc[1]]
                pat.expect[name] = {"net": net, "ring": ring, "kind": it.kind,
                                    "layer": esc[0] if esc else None, "vias": len(vs), "cell": cell,
                                    "cell_xy": rp(rot_pt(pat.cell_xy(*it.cell), ctr, s)) if it.cell else None,
                                    "dest": f"J{s + 1}.{k[0] + 1}" if esc else None}
        if register:
            dests.append([rot_pt(q, ctr, s) for q in pads])
    if register:
        bga = []
        for r in range(n):
            for c in range(n):
                name = pat.ball_name(c, r)
                net = pat.expect[name]["net"]
                bga.append(Pad(name, net, pat.ball_xy(c, r), (R.pad, R.pad), shape="circle"))
        pat.fps = [Fp("U1", bga, "bga")]
        for s in range(4):
            jp = []
            for it, segs, vs, esc, *k in routes:
                if esc is None:
                    continue
                c, r = (int(round(t)) for t in rot_ball(*it.ball, n, s))
                name = pat.ball_name(c, r)
                net = it.net or name
                q = dests[s][k[0]]
                jp.append((k[0], Pad(str(k[0] + 1), net, (r4(q[0]), r4(q[1])), (DEST_PAD, DEST_PAD), shape="circle",
                                     drill=DEST_DRILL)))
                pat.connections.append((net, f"U1.{name}", f"J{s + 1}.{k[0] + 1}"))
            pat.fps.append(Fp(f"J{s + 1}", [p for _, p in sorted(jp, key=lambda t: t[0])], "terminal"))
        span = max(abs(q[i] - ctr[i]) for qs in dests for q in qs for i in (0, 1)) + DEST_PAD / 2 + 1.6
        pat.size = (math.ceil(2 * span / 0.5) * 0.5,) * 2  # build() centres the BGA on this board
    return Sol(label, tracks, vias, why)


def build(pat: BPat) -> BPat:
    # first pass with a provisional centre to learn the extent, then the final centre
    pat.centre = (0.0, 0.0)
    pat.roles, pat.expect, pat.connections = {}, {}, []
    build_sol(pat, pat.top, "probe", register=True)
    W = pat.size[0]
    pat.centre = (r4(W / 2), r4(W / 2))
    pat.roles, pat.expect, pat.connections = {}, {}, []
    pat.ref = build_sol(pat, pat.top, "expected", register=True)
    pat.size = (W, W)
    pat.ref_tracks, pat.ref_vias = pat.ref.tracks, pat.ref.vias
    if pat.alt_top is not None:
        pat.alt = build_sol(pat, pat.alt_top, pat.alt_label, pat.alt_why)
    return pat


# ----------------------------------------------------------------------------------------------- verification
def _pad_layers(p: Pad):
    return LAYERS4 if p.drill else [p.layer]


def copper(pat: BPat, sol: Sol | None):
    out = []
    for f in pat.fps:
        for p in f.pads:
            g = Point(p.pos).buffer(p.size[0] / 2, quad_segs=24)
            for ly in _pad_layers(p):
                out.append((p.net, ly, g, "pad", f"{f.ref}.{p.num}"))
    if sol:
        for t in sol.tracks:  # one item per segment, as in KiCad (the courtyard rule is decided per item)
            for a, c in zip(t.pts, t.pts[1:]):
                out.append((t.net, t.layer, LineString([a, c]).buffer(t.width / 2, quad_segs=16), "track", None))
        for v in sol.vias:
            g = Point(v.pos).buffer(v.dia / 2, quad_segs=24)
            for ly in LAYERS4:
                out.append((v.net, ly, g, "via", None))
    return out


def holes(pat: BPat, sol: Sol | None):
    hs = [(p.net, Point(p.pos).buffer(p.drill / 2, quad_segs=16)) for f in pat.fps for p in f.pads if p.drill]
    if sol:
        hs += [(v.net, Point(v.pos).buffer(v.drill / 2, quad_segs=16)) for v in sol.vias]
    return hs


def check_sol(pat: BPat, sol: Sol) -> dict:
    R = pat.rules
    items = copper(pat, sol)
    worst, bad = 9e9, []
    by_layer = {}
    for it in items:
        by_layer.setdefault(it[1], []).append(it)
    for ly, its in by_layer.items():
        for i, (n1, _, g1, k1, _) in enumerate(its):
            for n2, _, g2, k2, _ in its[i + 1:]:
                if n1 == n2:
                    continue
                d = g1.distance(g2)
                worst = min(worst, d)
                if d < pat.clear_for(g1, g2) - 1e-6:
                    bad.append((n1, n2, ly, k1, k2, round(d, 4)))
    hc = R.kicad()["min_hole_clearance"]
    hworst = 9e9
    for hn, hg in holes(pat, sol):
        for n2, _, g2, k2, _ in items:
            if n2 == hn:
                continue
            d = hg.distance(g2)
            hworst = min(hworst, d)
            if d < hc - 1e-6:
                bad.append(("hole", hn, n2, k2, round(d, 4)))
    W, H = pat.size
    edge = min(min(g.bounds[0], g.bounds[1], W - g.bounds[2], H - g.bounds[3]) for _, _, g, _, _ in items)
    # connectivity per net: pads joined through same-layer overlaps; vias and PTH join all layers; a plane net counts
    # as connected when every ball of it reaches a via (the plane is the terminal)
    unconnected = []
    for net in pat.nets:
        its = [(ly, g, k, ref) for n, ly, g, k, ref in items if n == net]
        parent = list(range(len(its)))

        def find(a):
            while parent[a] != a:
                parent[a] = parent[parent[a]]
                a = parent[a]
            return a

        for i in range(len(its)):
            for j in range(i + 1, len(its)):
                same = its[i][0] == its[j][0] and its[i][1].intersects(its[j][1])
                multi = its[i][2] in ("via", "pad") and its[j][2] in ("via", "pad") and its[i][1].equals(its[j][1])
                if same or multi:
                    parent[find(i)] = find(j)
        pads = [i for i, it in enumerate(its) if it[2] == "pad"]
        if net in ("GND", "VCC") and any(z[0] == net for z in pat.zones):
            for i in pads:
                if not any(find(i) == find(j) for j, it in enumerate(its) if it[2] == "via"):
                    unconnected.append((net, its[i][3]))
        elif len({find(i) for i in pads}) != 1:
            unconnected.append((net, "split"))
    return {"min_clearance": r4(worst), "min_hole_clearance": r4(hworst), "violations": bad, "min_edge": r4(edge),
            "edge_ok": edge >= EDGE - 1e-6, "unconnected": unconnected}


def check(pat: BPat) -> dict:
    out = {}
    for key, sol in (("reference", pat.ref), ("alternative", pat.alt)):
        if sol is None:
            continue
        rep = check_sol(pat, sol)
        assert not rep["violations"], (pat.name, key, rep["violations"][:5])
        assert rep["edge_ok"], (pat.name, key, rep["min_edge"])
        assert not rep["unconnected"], (pat.name, key, rep["unconnected"][:5])
        out[key] = {k: rep[k] for k in ("min_clearance", "min_hole_clearance", "min_edge")}
    return out


# ----------------------------------------------------------------------------------------------- the user's BGA rules
#: The user's BGA fan-out rules (2026-11-12), as the checker numbers them.
RULES_TEXT = {
    "R1": "no trace passes through or over another ball's via (real widths; DRC clearance and drawn overlap)",
    "R2": "a dog-bone via sits at the centre of the open space between 4 balls (max clearance), never near the pad",
    "R3": "dog-bones point outward from the package centre per quadrant (top-left quadrant -> up-left, ...)",
    "R4": "one via per plane ball (GND / PWR), no shared vias",
    "R5": "via style is a production option: dog-bone by default, via-in-pad only with option via_in_pad",
    "R6": "a .kicad_dru courtyard rule (neckdown) may relax width / clearance inside the BGA",
    "R7": "signals on the plane layers (In1 / In2 of a 4-layer board) are allowed but very costly: expected answers "
          "never use them",
    "short": "copper of two nets touching (shapely overlap on one layer, or KiCad DRC shorting_items)",
}
PLANE_LAYERS = ("In1.Cu", "In2.Cu")  # topor's default 4-layer roles: In1 gnd, In2 pwr


def outward_dirs(pat: BPat, c: int, r: int) -> set:
    """Allowed dog-bone directions (sx, sy) of ball (c, r), screen frame (y down): away from the package centre per
    quadrant. A ball on the centre column / row of an odd array may take either sign on that axis (the expected
    answers use the clockwise pinwheel: top centre -> up-right, right -> down-right, bottom -> down-left, left ->
    up-left, centre ball -> up-left); the 6 x 6 patterns have no such ball."""
    o = (pat.n - 1) / 2
    sx = [int(math.copysign(1, c - o))] if c != o else [-1, 1]
    sy = [int(math.copysign(1, r - o))] if r != o else [-1, 1]
    return {(a, b) for a in sx for b in sy}


def _dir_name(sx, sy):
    return ("up" if sy < 0 else "down") + "-" + ("left" if sx < 0 else "right")


def rule_faults(pat: BPat, sol: Sol, drc: dict | None = None) -> list[dict]:
    """The user's BGA rules R1-R4, R5 (via-in-pad without the option), R7 and shorts, checked on copper `sol` with
    real widths. Each fault: rule, pos (x, y), ball (or None), text. `drc`: a kicad-cli DRC json of the same board
    (its shorting_items are added with KiCad's positions). O(T x V + I^2) per layer."""
    R = pat.rules
    out = []
    balls = []  # (name, net, (x, y), (c, r))
    for r in range(pat.n):
        for c in range(pat.n):
            name = pat.ball_name(c, r)
            balls.append((name, pat.expect[name]["net"], pat.ball_xy(c, r), (c, r)))
    x0, y0, x1, y1 = pat.bga_box()
    cells = [pat.cell_xy(cx + 0.5, cy + 0.5) for cy in range(pat.n - 1) for cx in range(pat.n - 1)]
    planes = {n for n, _ in pat.zones} | {"GND", "VCC"}
    vgeo = [(v, Point(v.pos).buffer(v.dia / 2, quad_segs=24)) for v in sol.vias]
    segs = [(t, LineString([a, b]), LineString([a, b]).buffer(t.width / 2, quad_segs=16))
            for t in sol.tracks for a, b in zip(t.pts, t.pts[1:]) if math.dist(a, b) > 1e-9]
    ball_of_net = {}
    for name, net, pos, cr in balls:
        ball_of_net.setdefault(net, []).append((name, pos, cr))

    def owner(v):  # the BGA ball a via serves: the nearest ball of its net within one pitch
        cand = [(math.dist(v.pos, pos), name, pos, cr) for name, pos, cr in ball_of_net.get(v.net, [])]
        cand = [q for q in cand if q[0] <= R.pitch + 1e-6]
        return min(cand) if cand else None

    # R1: a track of one net within clearance of (or over) a via of another net -- vias are on every layer
    seen = set()
    for t, line, g in segs:
        for v, vg in vgeo:
            if v.net == t.net:
                continue
            d = g.distance(vg)
            need = pat.clear_for(g, vg)
            if d < need - 1e-4:
                key = (t.net, v.net, round(v.pos[0], 2), round(v.pos[1], 2))
                if key in seen:
                    continue
                seen.add(key)
                q = line.interpolate(line.project(Point(v.pos)))
                ow = owner(v)
                what = "over" if d <= 1e-6 else f"{d:.3f} mm from"
                out.append({"rule": "R1", "pos": (r4(q.x), r4(q.y)), "ball": t.net if t.net in pat.expect else None,
                            "short": d <= 1e-6, "dist": r4(d),
                            "text": f"{t.net} {t.layer.replace('.Cu', '')} track {what} the via of "
                                    f"{ow[1] if ow else v.net} at ({v.pos[0]:.2f}, {v.pos[1]:.2f})"})
    # R2 / R3 / R5: every via inside the BGA box that serves a ball
    for v, vg in vgeo:
        if not (x0 <= v.pos[0] <= x1 and y0 <= v.pos[1] <= y1):
            continue
        ow = owner(v)
        if ow is None:
            continue
        dball, name, bpos, (c, r) = ow
        if dball <= VIA_CENTRE_TOL:
            out.append({"rule": "R5", "pos": v.pos, "ball": name, "text": f"{name}: via-in-pad (option via_in_pad off)"})
            continue
        off = min(math.dist(v.pos, q) for q in cells)
        if off > VIA_CENTRE_TOL:
            gap = min(math.dist(v.pos, pos) - R.pad / 2 - v.dia / 2 for _, _, pos, _ in balls)
            out.append({"rule": "R2", "pos": v.pos, "ball": name, "dist": r4(off),
                        "text": f"{name}: via {off:.2f} mm off the open-space centre (gap to the nearest ball {gap:.3f})"})
        sx = int(math.copysign(1, v.pos[0] - bpos[0])) if abs(v.pos[0] - bpos[0]) > 1e-6 else 0
        sy = int(math.copysign(1, v.pos[1] - bpos[1])) if abs(v.pos[1] - bpos[1]) > 1e-6 else 0
        allowed = outward_dirs(pat, c, r)
        if (sx, sy) not in allowed:
            got = _dir_name(sx or 1, sy or 1) if sx and sy else ("up" if sy < 0 else "down" if sy > 0 else
                                                                 "left" if sx < 0 else "right")
            exp = " or ".join(sorted(_dir_name(*a) for a in allowed))
            out.append({"rule": "R3", "pos": v.pos, "ball": name,
                        "text": f"{name}: dog-bone points {got} (expected {exp})"})
    # R4: one own via per plane ball
    for name, net, pos, cr in balls:
        if net not in planes:
            continue
        own = [v for v, _ in vgeo if v.net == net and (ow := owner(v)) is not None and ow[1] == name]
        if not own:
            out.append({"rule": "R4", "pos": pos, "ball": name, "text": f"{name}: plane ball without its own via"})
    for v, _ in vgeo:
        if v.net not in planes:
            continue
        near = [name for name, pos, _ in ball_of_net.get(v.net, []) if math.dist(v.pos, pos) <= R.pitch / SQ2 + 0.05]
        tied = [name for name in near if not any(
            w.net == v.net and w is not v and (ow := owner(w)) is not None and ow[1] == name for w, _ in vgeo)]
        if len(tied) > 1:
            out.append({"rule": "R4", "pos": v.pos, "ball": tied[0],
                        "text": f"via at ({v.pos[0]:.2f}, {v.pos[1]:.2f}) shared by plane balls {', '.join(tied)}"})
    # R7: signal copper on a plane layer
    for t, line, g in segs:
        if t.layer in PLANE_LAYERS and t.net not in planes:
            c = line.interpolate(0.5, normalized=True)
            out.append({"rule": "R7", "pos": (r4(c.x), r4(c.y)), "ball": t.net if t.net in pat.expect else None,
                        "text": f"{t.net}: signal on {t.layer}"})
    # shorts: two nets' copper touching on one layer (track-track, track-pad, via-pad, via-via); track-via is R1
    items = []
    board_pads = getattr(sol, "pads", None) or [(p.net, _pad_layers(p), p, f.ref) for f in pat.fps for p in f.pads]
    for net, lys, p, ref in board_pads:
        g = Point(p.pos).buffer(p.size[0] / 2, quad_segs=24)
        for ly in lys:
            items.append((net, ly, g, f"pad {ref}.{p.num}"))
    for t, line, g in segs:
        items.append((t.net, t.layer, g, f"{t.layer.replace('.Cu', '')} track {t.net}"))
    for v, vg in vgeo:
        for ly in LAYERS4:
            items.append((v.net, ly, vg, f"via {v.net}"))
    seen = set()
    for i, (n1, l1, g1, w1) in enumerate(items):
        for n2, l2, g2, w2 in items[i + 1:]:
            if n1 == n2 or l1 != l2 or ("track" in w1 and "via" in w2) or ("via" in w1 and "track" in w2):
                continue
            if g1.intersects(g2):
                q = g1.intersection(g2).representative_point()
                key = tuple(sorted((w1, w2)))
                if key in seen:
                    continue
                seen.add(key)
                out.append({"rule": "short", "pos": (r4(q.x), r4(q.y)), "ball": n1 if n1 in pat.expect else None,
                            "text": f"{w1} touches {w2} on {l1} at ({q.x:.2f}, {q.y:.2f})"})
    if drc:
        for v in drc.get("violations", []):
            if v.get("type") != "shorting_items":
                continue
            its = v.get("items", [])
            p0 = its[0]["pos"] if its else {"x": 0, "y": 0}
            out.append({"rule": "short", "pos": (r4(p0["x"]), r4(p0["y"])), "ball": None, "drc": True,
                        "text": "KiCad DRC: " + v.get("description", "") + " -- " +
                                "; ".join(f"{it['description']} @ ({it['pos']['x']:.2f}, {it['pos']['y']:.2f})"
                                          for it in its)})
    return out


def faults_by_rule(faults: list[dict]) -> dict:
    out = {}
    for f in faults:
        out.setdefault(f["rule"], []).append(f)
    return out


def net_lengths(sol: Sol) -> dict:
    out = {}
    for t in sol.tracks:
        out[t.net] = out.get(t.net, 0.0) + plen(t.pts)
    return {k: r4(v) for k, v in out.items()}


def price(sol: Sol) -> dict:
    ln = net_lengths(sol)
    tot = sum(ln.values())
    return {"length_mm": r4(tot), "vias": len(sol.vias), "price": r4(tot + VIA_PRICE * len(sol.vias))}


# ----------------------------------------------------------------------------------------------- patterns
R_1MM = Rules(1.0, 0.50, 0.15, 0.15, (0.40, 0.20),
              "pitch / pad: HaasoscopePro IC5 (BGA484) and IC1 (BGA256), 1.0 mm, 0.50 / 0.53 mm pads; track / gap 0.15 / "
              "0.15: Luzin & Polubasov 2008 PDF p. 2 example (one track between two balls); via 0.40 / 0.20: HaasoscopePro "
              "Board Setup minimum and BGA vias")
R_08 = Rules(0.8, 0.40, 0.15, 0.15, (0.40, 0.20),
             "pitch / pad 0.8 / 0.40: ULX3S U1 (CABGA381), ButterStick U4 (caBGA381); via 0.40 / 0.20: ButterStick, ~ULX3S "
             "0.419 / 0.20, ThunderScope 0.406 / 0.203; track / gap 0.15 / 0.15 (6 mil, the common low-cost fab class) "
             "instead of the 0.127 / 0.127 those boards use, so that no track passes between two balls")


def _ring0_top(n):
    return [Item((c, 0), "straight") for c in range(n - 1)]


def p_one_layer() -> BPat:
    n = 6
    top = _ring0_top(n) + [Item((1, 1), "channel", (0.5, 0.5)), Item((2, 1), "channel", (2.5, 0.5)),
                           Item((3, 1), "channel", (3.5, 0.5)), Item((2, 2), "channel", (1.5, 1.5))]
    alt = _ring0_top(n) + [Item((1, 1), "channel", (0.5, 0.5)), Item((2, 1), "channel", (2.5, 0.5)),
                           Item((3, 1), "channel", (3.5, 0.5)), Item((2, 2), "dogbone", (1.5, 1.5), "B.Cu")]
    return BPat("bga_one_layer", "6 x 6, 1.0 mm, one track per channel: everything escapes on F.Cu, 0 vias", R_1MM, n, top,
                alt_top=alt, alt_label="outside-in practice: 4 centre balls dog-bone to B.Cu (4 vias)",
                alt_why="the 'outer two rows on top, inner balls via' habit of big BGAs; here it adds 4 vias (+100 mm price) "
                        "and no length: the 4 centre balls have free channels on F.Cu",
                alt_rules="none (R1-R7 hold), but 4 vias more than needed",
                judgement=[
                    "rules R1-R5 hold trivially (no via); R7: nothing on In1 / In2. 0 vias by price: Luzin p. 1 shows a "
                    "6 x 6 with one track per channel escapes on one layer; the outer-two-rows-on-top habit is drawn as the "
                    "alternative (4 vias, +100 mm). TopoR's 'every ball except the outer rows its own via' (Luzin p. 4) is "
                    "read as a fallback when channels run out",
                    "each side: 5 ring-0 balls straight out, 3 ring-1 balls through the channel at a cell next to them, 1 "
                    "centre ball through the remaining channel; the 4 sides are rotations, so any rotation or mirror of the "
                    "assignment is equally optimal (a channel wire is not a dog-bone: R3 does not apply to it)",
                    "destination PTH rows (0.6 / 0.3 at 1.0 mm) connect on every layer, so a router may choose B.Cu at a cost "
                    "of 1 via per wire; the pad row never forces a via"],
                literature=[LIT["luzin_p1"], LIT["luzin_p2"], LIT["luzin_p4"], LIT["topor50"]])


def p_plane_balls() -> BPat:
    n = 6
    top = _ring0_top(n) + [Item((1, 1), "plane", (0.5, 0.5), net="VCC"), Item((2, 1), "channel", (2.5, 0.5)),
                           Item((3, 1), "channel", (3.5, 0.5)), Item((2, 2), "plane", (1.5, 1.5), net="GND")]
    alt = _ring0_top(n) + [Item((1, 1), "plane", (0.5, 0.5), net="VCC"), Item((2, 1), "channel", (2.5, 0.5)),
                           Item((3, 1), "channel", (3.5, 0.5)), Item((2, 2), "plane", (2.5, 2.5), net="GND")]
    pat = BPat("bga_plane_balls", "Plane balls: one via per ball into its plane, no trace; signals on F.Cu", R_1MM, n, top,
               zones=[("GND", "In1.Cu"), ("VCC", "In2.Cu")],
               alt_top=alt, alt_label="shared GND via: the 4 centre balls on one via in the centre cell",
               alt_why="1 GND via instead of 4 (-75 mm price), but four balls hang on one barrel (rule R4: one via per "
                       "plane ball; Luzin p. 4: every ball its own via, no jumpers between neighbouring balls)",
               alt_rules="R4 (shared via); its stubs also point into the package centre (R3)",
               judgement=[
                   "R4: one own via per plane ball (user rule 2026-11-12, BGAs are power hungry: short paths to the plane); "
                   "the shared-via alternative is drawn as ruled out",
                   "R2: every plane via at the centre of the open space between its 4 balls (0.257 mm to each ball at 1.0 / "
                   "0.50 / 0.40), never next to the pad (solder wicking)",
                   "R3: every plane stub points away from the package centre per quadrant: the 4 GND balls of the centre "
                   "into the 4 diagonal cells around the centre cell, the 4 VCC ring-1 corners to the cells at the array "
                   "corners",
                   "R5: dog-bone (default); with option via_in_pad the stub would be 0 (via in the ball)",
                   "R7: no signal on In1 / In2 (the planes); the signal balls keep the bga_one_layer channels on F.Cu",
                   "the plane nets are power nets only by name (same 0.15 mm stub as the signals, net class Default)"],
               literature=[LIT["luzin_p2"], LIT["luzin_p3"], LIT["luzin_p4"], LIT["topor63"], LIT["planes"], LIT["power_via"]])
    return pat


def _dogbone_top(n):
    """Top side of the dog-bone patterns (rule R3): each inner ball's via in the open space on its outward diagonal,
    so the 4 inner vias of a quadrant fill the 2 x 2 cells at its corner and the centre cross of cells stays free.
    The quadrant's innermost via (C3 for the top-left) is boxed in on B.Cu by its two neighbours (no track passes
    between two vias one pitch apart), so it leaves through the centre cross: right along the lane between ball
    rows B and C, then up the free column between columns 3 and 4 (the B.Cu wire stays in the lanes between balls)."""
    return _ring0_top(n) + [Item((1, 1), "dogbone", (0.5, 0.5), "B.Cu"), Item((2, 1), "dogbone", (1.5, 0.5), "B.Cu"),
                            Item((3, 1), "dogbone", (3.5, 0.5), "B.Cu"),
                            Item((2, 2), "dogbone", (1.5, 1.5), "B.Cu", bpath=((2.1, 1.5), (2.5, 1.1)))]


def p_dogbone() -> BPat:
    n = 6
    return BPat("bga_dogbone", "6 x 6, 0.8 mm, no track between balls: ring 0 on F.Cu, 16 dog-bones to B.Cu", R_08, n,
                _dogbone_top(n),
                judgement=[
                    "R3 fixes every via: each inner ball takes the open space on its outward diagonal, so a quadrant's 4 "
                    "inner vias fill the 2 x 2 cells at its corner and the centre cross of cells stays free. Changed from "
                    "ef43e25: B3, C5, E4, D2 (ring 1 next to the centre column / row) had their via up-right of the ball, "
                    "i.e. toward the neighbouring quadrant",
                    "R2: every via at an open-space centre: 0.166 mm to each of its 4 balls (0.40 via, 0.40 pads, 0.8 pitch "
                    "-- the only spot where it fits at all)",
                    "on B.Cu no track passes between two vias one pitch apart (0.40 < 0.45), so each quadrant's innermost "
                    "via (C3, C4, D4, D3) is boxed in on two sides and leaves through one arm of the free centre cross. "
                    "Expected path for C3: right along the lane between rows B and C, then up between columns 3 and 4 "
                    "(1.77 pitch); the straight 45-degree line under ball B3 is 0.28 mm shorter and also legal (B.Cu under "
                    "an F.Cu ball): the checker accepts both (detour bound reference + 1 mm). Your choice?",
                    "R7: In1.Cu / In2.Cu are planes by topor's default 4-layer roles, so the inner balls escape on B.Cu",
                    "R5: a via-in-pad variant (option via_in_pad) is NOT built: at these rules it is infeasible -- the 12 "
                    "ring-1 vias in their balls form a closed fence (0.40 gap < 0.45 for one track) on every layer of a "
                    "through via, so the 4 ring-2 balls cannot escape without blind / micro vias; and with the neckdown "
                    "rule F.Cu alone escapes everything (bga_neckdown, 0 vias). A meaningful via_in_pad case needs a "
                    "bigger array (7 x 7 / 8 x 8, not built yet)",
                    "0.15 / 0.15 rules with 0.40 pads at 0.8 mm is the zero-channel case (6-mil fab, no custom rule); with "
                    "the BGA neckdown rule one track fits (bga_neckdown)"],
                literature=[LIT["luzin_p1"], LIT["luzin_p4"], LIT["topor50"], LIT["haas2026"], LIT["power_via"]])


NECKDOWN_DRU = """(version 1)
(rule "BGA neckdown"
  (condition "A.intersectsCourtyard('U1')")
  (constraint track_width (min 0.1mm) (opt 0.15mm))
  (constraint clearance (min 0.1mm)))
"""


def p_neckdown() -> BPat:
    n = 6
    # the channel cells follow the dog-bone cells of bga_dogbone, so both answers have the same escape order (and the
    # same destination pads): B3 through the cell up-left of it, C3 up-right and through the column between 3 and 4
    top = _ring0_top(n) + [Item((1, 1), "channel", (0.5, 0.5)), Item((2, 1), "channel", (1.5, 0.5)),
                           Item((3, 1), "channel", (3.5, 0.5)), Item((2, 2), "channel", (2.5, 1.5))]
    alt = _dogbone_top(n)
    return BPat("bga_neckdown", "bga_dogbone + a .kicad_dru BGA neckdown rule: one track per channel, everything on F.Cu, "
                                "0 vias", R_08, n, top, dru=NECKDOWN_DRU, region_clear=0.10,
                alt_top=alt, alt_label="bga_dogbone's answer (16 dog-bones): what a router that ignores the courtyard rule "
                                       "has to do",
                alt_why="DRC clean and obeys R1-R5, but 16 vias (+400 mm price) where the courtyard rule lets one track "
                        "pass between two balls",
                alt_rules="none (R1-R7 hold), but 16 vias more than needed",
                judgement=[
                    "R6: the board is bga_dogbone's (0.8 / 0.40 pads, net class 0.15 / 0.15) plus a .kicad_dru: rule 'BGA "
                    "neckdown' (condition A.intersectsCourtyard('U1'): track_width min 0.10 opt 0.15, clearance min 0.10); "
                    "U1 has an F.CrtYd rectangle half a pitch around the outer balls. kicad-cli DRC reads it (reference "
                    "0 / 0; the same copper without the .kicad_dru: clearance violations between balls and wires)",
                    "inside the courtyard a 0.15 track passes between two balls with 0.125 mm each side >= 0.10: one track "
                    "per channel, so the 6 x 6 escapes like bga_one_layer: 0 vias, all on F.Cu. No neck is needed (the "
                    "rule's opt width 0.15 fits); a 0.10 neck would be equally legal",
                    "outside the courtyard the net class 0.15 clearance holds again: the wires leave at 0.40 mm spacing, "
                    "turn 45 degrees one after the other (0.10 mm stagger) so neighbours stay 0.20 mm apart",
                    "R1-R5 hold trivially (no via); R7: nothing on In1 / In2"],
                literature=[LIT["luzin_p1"], LIT["luzin_p2"], LIT["kicad_dru"]])


BUILDERS = {"bga_one_layer": p_one_layer, "bga_plane_balls": p_plane_balls, "bga_dogbone": p_dogbone,
            "bga_neckdown": p_neckdown}
NAMES = list(BUILDERS)


def expected_summary(pat: BPat) -> dict:
    """Escape layer per ring, vias, via positions and the length bound, from the reference."""
    per_ring = {}
    for name, e in pat.expect.items():
        key = f"ring{e['ring']}" if e["net"] not in ("GND", "VCC") else e["net"]
        d = per_ring.setdefault(key, {"balls": 0, "escape": {}, "vias": 0})
        d["balls"] += 1
        d["vias"] += e["vias"]
        lk = e["layer"] or "plane (via only)"
        d["escape"][lk] = d["escape"].get(lk, 0) + 1
    ln = net_lengths(pat.ref)
    return {"per_ring": per_ring, "vias": len(pat.ref.vias), "reference": price(pat.ref),
            "alternative": price(pat.alt) if pat.alt else None, "per_net_mm": ln,
            "detour_bound": "every signal net <= its reference length + 1.0 mm (and <= 1.15 x); longer = a detour",
            "drc": "0 violations, 0 unconnected", "crossings": 0}


# ----------------------------------------------------------------------------------------------- board files
def add_zones(b, pat: BPat):
    W, H = pat.size
    for net, layer in pat.zones:
        z = pcbnew.ZONE(b)
        z.SetLayer(LAYER_IDS[layer])
        z.SetNet(b.FindNet(net))
        ol = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in ((0.1, 0.1), (W - 0.1, 0.1), (W - 0.1, H - 0.1), (0.1, H - 0.1)):
            ol.Append(pcbnew.VECTOR2I(int(round(x * MM)), int(round(y * MM))))
        ol.SetClosed(True)
        z.AddPolygon(ol)
        z.SetMinThickness(int(0.15 * MM))
        z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_NEVER)
        b.Add(z)
    if pat.zones:
        pcbnew.ZONE_FILLER(b).Fill(b.Zones())


def add_courtyard(b, pat: BPat):
    """U1's courtyard: an F.CrtYd rectangle on the BGA box (needed by intersectsCourtyard('U1'))."""
    fp = b.FindFootprintByReference("U1")
    x0, y0, x1, y1 = pat.bga_box()
    sh = pcbnew.PCB_SHAPE(fp)
    sh.SetShape(pcbnew.SHAPE_T_RECT)
    sh.SetLayer(pcbnew.F_CrtYd)
    sh.SetWidth(int(0.05 * MM))
    sh.SetStart(pcbnew.VECTOR2I(int(round(x0 * MM)), int(round(y0 * MM))))
    sh.SetEnd(pcbnew.VECTOR2I(int(round(x1 * MM)), int(round(y1 * MM))))
    fp.Add(sh)


def write_pat(pat: BPat, sol: Sol | None, path: Path, *, dru: bool = True) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = build_board(_View(pat, sol), copper=sol is not None)
    add_courtyard(b, pat)
    add_zones(b, pat)
    pcbnew.SaveBoard(str(path), b)
    write_project_file(path, rules=pat.rules.kicad(), classes=pat.rules.classes())
    d = path.with_suffix(".kicad_dru")
    if pat.dru and dru:
        d.write_text(pat.dru)
    elif d.exists():
        d.unlink()
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
def _patch_poly(ax, geom, **kw):
    from matplotlib.patches import Polygon as MPoly

    for g in getattr(geom, "geoms", [geom]):
        ax.add_patch(MPoly(list(g.exterior.coords), closed=True, **kw))


def draw(ax, pat: BPat, sol: Sol | None, *, window=None, ratsnest=False, title="", marks=None, cells=False,
         labels="auto", spots=None):
    from matplotlib.patches import Circle, Rectangle

    W, H = pat.size
    ax.add_patch(Rectangle((0, 0), W, H, fc="#fbfbf6", ec="k", lw=1.0, zorder=0))
    for x0, y0, x1, y1 in ((0, 0, W, EDGE), (0, H - EDGE, W, H), (0, EDGE, EDGE, H - EDGE), (W - EDGE, EDGE, W, H - EDGE)):
        ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="#f3d9d9", ec="none", zorder=0.2))
    for net, layer in pat.zones:
        ax.add_patch(Rectangle((0.1, 0.1), W - 0.2, H - 0.2, fc=LAYER_COL[layer], alpha=0.035, ec=LAYER_COL[layer],
                               lw=0.8, ls=(0, (4, 3)), zorder=0.3))
    zoom = window is not None and (window[2] - window[0]) < 0.6 * W
    show_lab = zoom if labels == "auto" else labels
    x0, y0, x1, y1 = pat.bga_box()
    ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="none", ec="#999999", lw=0.6, ls=(0, (2, 2)), zorder=0.5))
    if cells:
        for cy in range(pat.n - 1):
            for cx in range(pat.n - 1):
                q = pat.cell_xy(cx + 0.5, cy + 0.5)
                ax.plot([q[0]], [q[1]], marker="+", ms=3.5, mew=0.5, color="#b0b0b0", zorder=0.6)
    for f in pat.fps:
        for p in f.pads:
            role = pat.roles.get(p.num) if f.ref == "U1" else None
            if p.drill:
                ax.add_patch(Circle(p.pos, p.size[0] / 2, fc="#d9b44a", ec="#7a5a00", lw=0.4, zorder=2))
                ax.add_patch(Circle(p.pos, p.drill / 2, fc="white", ec="#7a5a00", lw=0.3, zorder=2.1))
            else:
                fc = ROLE_COL.get(role, "#cccccc")
                ax.add_patch(Circle(p.pos, p.size[0] / 2, fc=fc, ec="#5a1010", lw=0.5, alpha=0.9, zorder=2))
                if show_lab:
                    ax.text(p.pos[0], p.pos[1], p.num, fontsize=4.6, ha="center", va="center", color="k", zorder=7,
                            clip_on=True)
    if sol is not None:
        order = ["In2.Cu", "In1.Cu", "B.Cu", "F.Cu"]
        for layer in order:
            for t in sol.tracks:
                if t.layer != layer:
                    continue
                g = LineString(t.pts).buffer(t.width / 2, quad_segs=8)
                top = layer == "F.Cu"
                _patch_poly(ax, g, fc=LAYER_COL[layer], ec="none", alpha=0.85 if top else 0.6,
                            zorder=4.5 if top else 4)
        for v in sol.vias:
            ax.add_patch(Circle(v.pos, v.dia / 2, fc="#9a9a9a", ec="k", lw=0.4, zorder=5))
            ax.add_patch(Circle(v.pos, v.drill / 2, fc="white", ec="k", lw=0.2, zorder=5.1))
    if ratsnest:
        pos = {f"{f.ref}.{p.num}": p.pos for f in pat.fps for p in f.pads}
        for net, a, b in pat.connections:
            role = pat.roles.get(a.split(".")[1])
            ax.plot(*zip(pos[a], pos[b]), ls=(0, (2, 2)), lw=0.6, color=ROLE_COL.get(role, "#444444"), zorder=3)
        for name, e in pat.expect.items():
            if e["net"] in ("GND", "VCC"):
                q = pat.ball_xy(ROWS.index(name[0]) * 0 + int(name[1:]) - 1, ROWS.index(name[0]))
                ax.plot([q[0]], [q[1]], marker="v", ms=4, color=ROLE_COL[e["net"]], zorder=8)
    for mk in marks or []:
        ax.add_patch(Circle(mk["pos"], mk.get("r", 0.45), fc="none", ec=mk.get("color", BAD), lw=1.2, zorder=9))
    for k, sp in enumerate(spots or []):
        if sp["rule"] in ("R1", "short"):
            ax.plot([sp["pos"][0]], [sp["pos"][1]], marker="x", ms=7 if zoom else 4, mew=1.6 if zoom else 1.0, color=BAD,
                    zorder=10)
        else:  # R2 / R3: a small square around the via
            ax.plot([sp["pos"][0]], [sp["pos"][1]], marker="s", ms=8 if zoom else 4, mfc="none", mec=BAD,
                    mew=1.0, zorder=10)
        if zoom and sp.get("label"):
            ax.text(sp["pos"][0] + 0.08, sp["pos"][1] - 0.08, sp["label"], fontsize=5.5, color=BAD, zorder=11,
                    fontweight="bold", clip_on=True)
    if window:
        ax.set_xlim(window[0], window[2])
        ax.set_ylim(window[3], window[1])
    else:
        ax.set_xlim(-0.3, W + 0.3)
        ax.set_ylim(H + 0.3, -0.3)
    ax.set_aspect("equal")
    ax.grid(True, lw=0.3, color="#e4e4e4")
    ax.tick_params(labelsize=6)
    ax.set_title(title, fontsize=8, loc="left")


def legend(fig, pat: BPat, y=0.005):
    from matplotlib.lines import Line2D
    from matplotlib.patches import Patch

    used = set(pat.roles.values())
    h = [Patch(fc=ROLE_COL[k], ec="#5a1010", label=lab) for k, lab in
         (("ring0", "ball ring 0 (outer)"), ("ring1", "ball ring 1"), ("ring2", "ball ring 2 (centre)")) if k in used]
    if any(z for z in pat.zones):
        h += [Patch(fc=ROLE_COL["GND"], ec="#5a1010", label="GND ball"), Patch(fc=ROLE_COL["VCC"], ec="#5a1010",
                                                                               label="VCC ball")]
    h += [Patch(fc=LAYER_COL["F.Cu"], alpha=0.85, label="F.Cu track"), Patch(fc=LAYER_COL["B.Cu"], alpha=0.6,
                                                                               label="B.Cu track"),
          Line2D([], [], marker="o", ls="", mfc="#9a9a9a", mec="k", label="via (white = drill)"),
          Line2D([], [], marker="o", ls="", mfc="#d9b44a", mec="#7a5a00", label="destination PTH 0.6/0.3")]
    for net, layer in pat.zones:
        h.append(Patch(fc=LAYER_COL[layer], alpha=0.2, ec=LAYER_COL[layer], ls="--", label=f"{net} plane on {layer}"))
    h += [Line2D([], [], marker="+", ls="", color="#b0b0b0", label="via cell centre"),
          Patch(fc="#f3d9d9", label="copper-to-edge band 0.50")]
    fig.legend(handles=h, loc="lower center", ncol=6, fontsize=6.3, frameon=True, bbox_to_anchor=(0.5, y))


def render(pat: BPat, doc: dict, out: Path) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig = plt.figure(figsize=(13.5, 14.2))
    axs = [fig.add_axes([0.04, 0.53, 0.44, 0.40]), fig.add_axes([0.53, 0.53, 0.44, 0.40]),
           fig.add_axes([0.04, 0.10, 0.44, 0.40]), fig.add_axes([0.53, 0.10, 0.44, 0.40])]
    k = doc["kicad"]
    pr = doc["expected"]["reference"]
    draw(axs[0], pat, None, ratsnest=True, title="problem: balls, destination rows, planes; ratsnest dashed (by ring); "
                                                 "triangles = plane balls")
    draw(axs[1], pat, pat.ref, title=f"EXPECTED (KiCad DRC {k['reference_drc'].get('violations')} / "
                                     f"{k['reference_drc'].get('unconnected')}): {pr['vias']} vias, {pr['length_mm']:.1f} mm, "
                                     f"price {pr['price']:.1f}")
    x0, y0, x1, y1 = pat.bga_box()
    m = 1.2
    win = (x0 - m, y0 - m, x1 + m, y1 + m)
    draw(axs[2], pat, pat.ref, window=win, cells=True, title="EXPECTED, zoom on the BGA (ball names, + = via cells)")
    if pat.alt is not None:
        pa = doc["expected"]["alternative"]
        draw(axs[3], pat, pat.alt, window=win, cells=True,
             title=f"ALTERNATIVE (ruled out; DRC {k['alternative_drc'].get('violations')} / "
                   f"{k['alternative_drc'].get('unconnected')}): {pa['vias']} vias, price {pa['price']:.1f}\n{pat.alt_label}")
    else:
        axs[3].axis("off")
        ch = pat.channel()
        txt = [f"Rules: pitch {pat.rules.pitch}, pad {pat.rules.pad}, track {pat.rules.track}, clearance {pat.rules.clear}, "
               f"via {pat.rules.via[0]}/{pat.rules.via[1]}",
               f"between two balls: gap {ch['ball_gap']} vs one track {ch['one_track_needs']} -> {ch['tracks_between_balls']} "
               f"tracks",
               f"between two vias (one pitch): gap {ch['via_gap']} -> {ch['tracks_between_vias']} tracks",
               f"via in a cell: gap to the 4 balls {ch['via_in_cell_gap']} (>= {pat.rules.clear})", "",
               "Per ring (expected):"]
        for key, d in doc["expected"]["per_ring"].items():
            txt.append(f"  {key}: {d['balls']} balls, escape {d['escape']}, vias {d['vias']}")
        axs[3].text(0.02, 0.98, "\n".join(txt), fontsize=8, va="top", ha="left", family="monospace",
                    transform=axs[3].transAxes)
    legend(fig, pat, y=0.055)
    fig.suptitle(f"{pat.name}: {pat.title}", fontsize=12, y=0.985)
    fig.text(0.02, 0.005, "\n".join(textwrap.wrap("Expected: " + pat.answer, 200)[:4]), fontsize=7, ha="left", va="bottom")
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=110)
    plt.close(fig)
    return out


# ----------------------------------------------------------------------------------------------- generate
def answer_text(pat: BPat, exp: dict) -> str:
    ch = pat.channel()
    parts = []
    for key, d in exp["per_ring"].items():
        esc = ", ".join(f"{v} {k}" for k, v in d["escape"].items())
        parts.append(f"{key} ({d['balls']}): {esc}, {d['vias']} vias")
    s = (f"{exp['vias']} vias in total; " + "; ".join(parts) + f". Reference {exp['reference']['length_mm']:.1f} mm, price "
         f"{exp['reference']['price']:.1f}. KiCad DRC 0 / 0, no crossings, no detour (each net <= reference + 1 mm). "
         f"Channels: {ch['tracks_between_balls']} track(s) between balls (gap {ch['ball_gap']}, one needs "
         f"{ch['one_track_needs']}), {ch['tracks_between_vias']} between vias; via in cell gap {ch['via_in_cell_gap']}.")
    return s


WHY = {
    "bga_one_layer": "One track fits between two balls (gap 0.50 >= 0.15 + 2 x 0.15). A 6 x 6 has 20 peripheral channels "
                     "and 16 inner balls (Luzin p. 1: 4(n-1) >= (n-2)^2 up to n = 6). Per side: ring 0 straight out (5), the "
                     "3 ring-1 balls of the side to their outward-diagonal cell and up its channel, the centre ball of that "
                     "side up the one free channel between ring 1 and through ring 0. Every channel holds one wire, no wire "
                     "crosses, no via is needed, so any via would only add 25 mm price.",
    "bga_plane_balls": "The plane is the terminal (planes-are-terminals): a plane ball needs exactly one via into its plane "
                       "and no trace beyond it. Each plane ball gets its own via (R4) at the centre of the open space on its "
                       "outward diagonal (R2, R3; Luzin p. 4: own via per ball, no jumpers between neighbours, short stub). "
                       "The signal balls keep the bga_one_layer channels (the centre is power now, Luzin p. 3 fig. 7), so "
                       "they stay on F.Cu with 0 vias; no wire passes within clearance of a foreign via (R1).",
    "bga_dogbone": "No track fits between two balls (gap 0.40 < 0.45): ring 0 encloses every inner ball on F.Cu, so each "
                   "of the 16 inner balls needs one via (proved minimum). R2 + R3 fix each via: the open-space centre on the "
                   "ball's outward diagonal (the only spot a 0.40 via fits, 0.166 mm to the balls). A quadrant's 4 vias fill "
                   "its 2 x 2 corner cells; on B.Cu no track passes between two vias one pitch apart, so 3 of them run "
                   "straight out and the innermost one leaves through the free centre cross of cells. Every B.Cu wire stays "
                   "in the lanes between balls and keeps clearance from every foreign via (R1).",
    "bga_neckdown": "The .kicad_dru courtyard rule lowers the clearance inside U1's courtyard to 0.10 mm (R6): a 0.15 "
                    "track then passes between two 0.40 balls at 0.8 mm (0.125 mm each side), one track per channel, and "
                    "a 6 x 6 escapes on one layer (Luzin p. 1: 4(n-1) >= (n-2)^2 up to n = 6). Same channel assignment "
                    "as bga_one_layer: 0 vias; every via would only add 25 mm price.",
}


def generate(name: str, out: Path, *, png: bool = True, drc: bool = True) -> dict:
    pat = build(BUILDERS[name]())
    built = check(pat)
    exp = expected_summary(pat)
    pat.answer = answer_text(pat, exp)
    pat.why = WHY[name]
    d = Path(out) / name
    paths = {"problem": write_pat(pat, None, d / "problem" / f"{name}.kicad_pcb"),
             "reference": write_pat(pat, pat.ref, d / "reference" / f"{name}.kicad_pcb")}
    if pat.alt is not None:
        paths["alternative"] = write_pat(pat, pat.alt, d / "alternative" / f"{name}.kicad_pcb")
    if pat.dru:  # control: the same expected copper without the .kicad_dru must fail KiCad DRC (the rule is read)
        paths["reference_without_dru"] = write_pat(pat, pat.ref, d / "reference_without_dru" / f"{name}.kicad_pcb",
                                                   dru=False)
    kicad = {}
    for key in ("problem", "reference", "alternative", "reference_without_dru"):
        if key in paths:
            kicad[f"{key}_drc"] = drc_of(paths[key]) if drc else {}
    # the user's BGA rules on the expected copper: none may fail; the alternative's faults are its reason
    ref_faults = rule_faults(pat, pat.ref)
    assert not ref_faults, (name, ref_faults[:5])
    alt_faults = rule_faults(pat, pat.alt) if pat.alt else []
    pins = {f"{f.ref}.{p.num}": {"net": p.net, "pos": rp(p.pos), "size": list(p.size), "drill": p.drill}
            for f in pat.fps for p in f.pads}

    def sol_doc(s):
        if s is None:
            return None
        return {"label": s.label, "why": s.why,
                "tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": [rp(p) for p in t.pts]} for t in s.tracks],
                "vias": [{"net": v.net, "pos": rp(v.pos), "dia": v.dia, "drill": v.drill} for v in s.vias]}

    doc = {"name": name, "title": pat.title, "expected_summary": pat.answer, "why": pat.why,
           "board": {"size_mm": list(pat.size), "copper_layers": 4, "stackup": "F.Cu signal / In1.Cu gnd / In2.Cu pwr / B.Cu "
                                                                                 "signal (topor default roles)",
                     "planes": [{"net": n, "layer": l} for n, l in pat.zones], "bga_centre": rp(pat.centre),
                     "bga_box": [r4(v) for v in pat.bga_box()]},
           "rules": {"pitch": pat.rules.pitch, "pad": pat.rules.pad, "track": pat.rules.track, "clearance": pat.rules.clear,
                     "via": list(pat.rules.via), "board_setup": pat.rules.kicad(), "source": pat.rules.source,
                     "channel": pat.channel(), "via_price_mm": VIA_PRICE},
           "balls": pat.expect, "pins": pins,
           "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "expected": exp, "built_check": built, "judgement_calls": pat.judgement, "literature": pat.literature,
           "alternative_label": pat.alt_label, "alternative_why": pat.alt_why,
           "reference": sol_doc(pat.ref), "alternative": sol_doc(pat.alt), "kicad": kicad,
           "bga_rules": RULES_TEXT, "reference_rule_faults": [],
           "alternative_rule_faults": {k: len(v) for k, v in faults_by_rule(alt_faults).items()},
           "alternative_rules": pat.alt_rules, "kicad_dru": pat.dru or None,
           "courtyard_clearance": pat.region_clear}
    d.mkdir(parents=True, exist_ok=True)
    (d / "expected.json").write_text(json.dumps(doc, indent=1) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        paths["png"] = render(pat, doc, d / f"{name}.png")
    return paths


SURVEY = """| Board (copper layers) | BGA | pitch / pad | rules (min clearance / tracks in BGA) | via | hand fan-out |
|---|---|---|---|---|---|
| HaasoscopePro ADC (10) | IC5 BGA484, IC1 BGA256 | 1.0 / 0.50-0.53 | 0.089 / 0.089-0.152 (2 tracks per channel) | 0.40/0.20 | via-in-pad on almost every ball (IC5 347, IC1 215); IC1 outer 2 rings ~1/3 on F.Cu |
| Zynq SoM (8) | U2 CLG484 | 0.8 / 0.406 | 0.09 / 0.10-0.12 (1 per channel) | 0.35/0.20 | ring 0: 56 F.Cu; ring 1: 34 F.Cu / 29 dog-bone; ring >= 2: dog-bone at the cell centre (230 of 302 vias at (0.5, 0.5) pitch) |
| Zynq SoM | U1 BGA96 (DDR3), U7 BGA153 0.5 mm (eMMC) | 0.8 / 0.42; 0.5 / 0.35 | same | 0.35/0.20 | U1 via-in-pad; U7 mostly NC, ring 0 out on the bottom layer |
| ButterStick (8) | U4 caBGA381 | 0.8 / 0.40 | 0.089 / 0.089-0.12 | 0.40/0.20 | via-in-pad 329 of 331 |
| ButterStick | U11/U12 BGA96 (DDR3) | 0.8 / 0.40 | same | 0.40/0.20 | mixed via-in-pad and dog-bone at the cell centre |
| Glasgow revD1 (6) | U8 BGA256; U11/U12 BGA24 1.0 mm | 0.8 / 0.32; 1.0 / 0.32 | 0.09 / 0.09 (2 per channel) | 0.30/0.20 | rings 0-4 about half on F.Cu, rest via-in-pad |
| ThunderScope 5.3 (6) | U18 CSG325 | 0.8 / 0.35 | 0.127 / 0.127 (1 per channel) | 0.406/0.203 | ring 0: 36 top; ring 1: 18 top / 28 dog-bone; ring >= 2 dog-bone at the cell centre, out on Int3 / bottom |
| ULX3S (4) | U1 CABGA381 | 0.8 / 0.40 | 0.127 tracks, measured gaps >= 0.127 (no .kicad_pro) | 0.419/0.20 | outer ring 45 F.Cu / 17 dog-bone, 2nd ring 32 F.Cu / 28 dog-bone, ring >= 3 dog-bone at the cell centre (180 of 265), mostly on the outward diagonal (137 out / 19 in), out on B.Cu / In2 |

Two families: via-in-pad (HaasoscopePro, ButterStick U4, Glasgow, Zynq U1) and dog-bone to the cell centre with the
outer 1-2 rings on the top layer (ULX3S, ThunderScope, Zynq U2): the second is the TopoR cell model (Luzin p. 4) and
what these patterns teach. Measured by `scratchpad/bga/survey.py` on read-only copies (ring = distance from the
array edge; "dog-bone" = a via 0.3-0.75 mm from the ball centre)."""


R1_FORENSICS = """Measured on the routed copies of the ef43e25 gallery (main 0f36b1e engine; topor's tracks 0.20 wide, vias
0.45 / 0.25 after its fab floors). Copper gap = track edge to via edge; hole gap = track edge to drill edge.

| board | spot | track (net, B.Cu, KiCad uuid) | foreign via (uuid, centre) | copper gap | hole gap | KiCad DRC |
|---|---|---|---|---|---|---|
| bga_one_layer | (7.0, 6.6): diagonal past B3's via toward C3 | C3 (6.927, 5.927)-(7.519, 6.654) 9fedad9a, (7.519, 6.654)-(7.680, 7.368) 00396b31 | B3 e2f426b0, (7.780, 6.506) | -0.025 (overlap) | 0.075 | shorting_items, hole_clearance |
| bga_one_layer | (9.8-10.1, 7.0-7.7): converging C4 stubs | C4 (10.079, 7.043)-(9.981, 7.654) 4773e1e2, (9.981, 7.654)-(10.070, 7.100) 90e5e725 | C5 e7959002, (9.940, 7.911) | -0.065 (overlap) | 0.035 | shorting_items, hole_clearance |
| bga_dogbone | (7.1, 6.6) | C3 (7.406, 6.292)-(7.142, 6.556) ebdc6486, (7.142, 6.556)-(7.142, 7.088) b2526b50 | B2 7b7ab353 (6.667, 6.650); B3 4ea261fd (7.756, 6.643) | 0.150 / 0.171 | 0.25 / 0.27 | none (exactly at clearance) |
| bga_dogbone | (9.3, 7.1) | C4 (9.073, 7.081)-(13.494, 7.081) 7836ea6c | B5 114f6a6c, (9.345, 6.655) | 0.100 | 0.200 | clearance |
| bga_dogbone | (6.7, 8.5) | D3 (6.048, 9.010)-(6.723, 8.585) 3247cbc5, (6.723, 8.585)-(7.372, 8.451) a0f2496f | D2 9e203ea1, (6.647, 8.270) | -0.001 (overlap) | 0.099 | shorting_items, hole_clearance |

The two spots the user marked ((7.0, 6.6) and the converging stubs at (9.9-10.1, 7.0-7.4) through C5's via) match the
bga_one_layer run exactly; in the bga_dogbone run the C3 wire at (7.1, 6.6) only touches the clearance line (0.150) and
the C4 wire passes B5's via at 0.100 (a clearance violation, no overlap). The expected answers of ef43e25 had no R1
fault (copper gap >= clearance everywhere); their only rule break was R3 in bga_dogbone (B3, C5, E4, D2 up-right /
toward the next quadrant)."""

RULE_HOW = {
    "R1": "Every track segment (real width) against every via of another net (vias are on all layers): copper gap < "
          "the clearance that applies (net class, or the courtyard rule) = fault; gap <= 0 = drawn overlap (a short).",
    "R2": f"A via inside the BGA box that serves a ball (nearest ball of its net within one pitch) must be within "
          f"{VIA_CENTRE_TOL} mm (VIA_CENTRE_TOL) of an open-space centre.",
    "R3": "The sign of (via - ball) on both axes must equal the sign of (ball - package centre) (outward_dirs()).",
    "R4": "Every plane ball needs a via of its own (nearest plane ball of that via); a via within reach of two plane "
          "balls that have no other via = shared.",
    "R5": "A via within VIA_CENTRE_TOL of its ball centre is via-in-pad: a fault while the option via_in_pad is off "
          "(default).",
    "R6": "bga_neckdown carries the rule; the checker uses its clearance for every pair of which one item touches U1's "
          "courtyard, as KiCad does; kicad-cli DRC is the judge.",
    "R7": "A track of a non-plane net on In1.Cu / In2.Cu (topor's default 4-layer plane roles).",
    "short": "Two nets' copper touching on one layer (shapely: track-track, track-pad, via-pad, via-via; track-via is "
             "R1), plus every KiCad DRC shorting_items of the routed board.",
}

TOPOR_NOTES = {  # what the topor run (main 0f36b1e engine, defaults + loop = topor, 2026-11-12) did, read off the PNGs,
    # rule_faults() and KiCad DRC. topor raises the boards' rules to its JLCPCB 1 oz fab floors (track 0.15 -> 0.20,
    # via 0.40 / 0.20 -> 0.45 / 0.25, hole clearance 0.20 -> 0.35, the courtyard clearance 0.10 -> 0.15)
    "bga_one_layer": "Same as the ef43e25 run: every one of the 16 inner balls gets a via and escapes on B.Cu (expected 0 "
                     "vias). R2: all 16 vias 0.35-0.54 mm off the open-space centre, overlapping their own ball pad; R3: "
                     "B3, C5, E4, D2 point toward the next quadrant; R1: the C3 B.Cu track runs over B3's via at (7.78, "
                     "6.51) and the C4 B.Cu spike (10.08, 7.04) -> (9.98, 7.65) -> (10.07, 7.10) over C5's via at "
                     "(9.94, 7.91): KiCad shorting_items + hole_clearance. C4 detours x1.34.",
    "bga_plane_balls": "DRC clean, R4 holds (one via per plane ball). 8 ring-1 signals take a via each (16 vias, "
                       "expected 8); R2: all 16 vias 0.35-0.54 off centre; R3: 12 vias point the wrong way (GND vias "
                       "toward the package centre, VCC vias up-right everywhere).",
    "bga_dogbone": "Topology right (ring 0 on F.Cu, 16 vias, inner balls on B.Cu, 182.5 mm). R2: every via 0.35-0.43 "
                   "mm off its open-space centre and overlapping its own ball pad (the floored 0.45 via does not fit a "
                   "cell centre: 0.566 - 0.20 - 0.225 = 0.141 < 0.15); R3: C3, C4, D3, D4 point wrong; R1: C3 B.Cu track "
                   "over B3's via at (7.47, 6.65) (shorting_items), D4 B.Cu track 0.070 mm from E4's via at (8.53, "
                   "9.35) (clearance + hole_clearance). KiCad DRC 10 new.",
    "bga_neckdown": "topor reads the rule (CUSTOM_RULES: track_width -> net, clearance -> region) but floors the region "
                    "clearance 0.10 -> 0.15 (CUSTOM_RULE_LIMITED), so the channels stay closed and it routes "
                    "bga_dogbone's way: 16 vias (expected 0), the same R1 / R2 / R3 faults and 8 new DRC violations.",
}


def readme(out: Path, docs: dict) -> Path:
    L = ["# BGA fan-out test patterns", "",
         "Generated by `python -m harness.bga_patterns` (module docstring: construction, rules). Each pattern: `problem/` "
         "(balls, destination rows, planes; nothing routed), `reference/` (expected copper), `alternative/` (a real layout "
         "the expected answer rules out, if any), `expected.json` (per-ball expectations: net, ring, escape layer, vias, "
         "cell, destination; per-net reference lengths), `<name>.png` (problem | expected | zoom | alternative or "
         "channel arithmetic). `topor/`: one topor run (defaults + `[optimize] loop = \"topor\"`) and its comparison PNG.",
         "", "NOT in the test suite: waiting for the user's approval of the PNGs.", "",
         "## Survey of the hand-routed BGAs (router-test-designs)", "", SURVEY, "",
         "## Rules (every `.kicad_pro`)", "",
         "- 4 copper layers, KiCad layer types all signal: topor's default roles F.Cu signal / In1.Cu gnd / In2.Cu pwr / "
         "B.Cu signal, so only F.Cu and B.Cu route.",
         "- The unrouted problem of bga_plane_balls shows 2 isolated_copper (the planes before any via reaches them).",
         "- Net class Default = Board Setup minimums (no necking, no via-size trap): see each pattern.",
         "- Destinations: PTH pads 0.6 / 0.3 at 1.0 mm in a row on each side, ordered like the escape lines (no forced "
         "crossing, no forced via). Price = mm + 25 mm per via. Copper to edge 0.50, hole clearance 0.20.",
         "- U1 has an F.CrtYd rectangle half a pitch around the outer ball centres; only bga_neckdown has a .kicad_dru.", "",
         "## The user's BGA rules (2026-11-12) and how `rule_faults()` checks them", "",
         "Every expected answer has 0 faults (asserted when generating); the PNG of a routed board marks each fault in "
         "magenta (circle = ball, x = R1 / short spot, square = R2 / R3 via) and lists them by rule in the footer.", ""]
    L += [f"- **{k}** {v}. " + RULE_HOW[k] for k, v in RULES_TEXT.items()]
    L += ["", "**Odd arrays (R3, centre row / column):** a ball on the centre column of an odd array takes the outward "
          "diagonal of a clockwise pinwheel (top half -> up-right, right half of the centre row -> down-right, bottom "
          "half -> down-left, left half -> up-left, the centre ball up-left); the checker accepts either outward diagonal "
          "for such balls. The 6 x 6 patterns have no centre row / column.", "",
          "**Not built yet (user question):** a 7 x 7 / 8 x 8 array is the first size where one layer cannot escape "
          "everything even with one track per channel (Luzin p. 1: 4(n-1) channels vs (n-2)^2 inner balls: 24 < 25 at "
          "n = 7, 28 < 36 at n = 8), so 'outer two rings on top, the rest dog-bone' becomes the optimum and the first "
          "interesting via_in_pad case; 0.5 / 0.65 mm pitch is the fine-pitch case (eMMC / LPDDR, Zynq U7) where no "
          "0.40 via fits between four balls at all (0.5 x sqrt 2 = 0.71 mm diagonal), so only via-in-pad or micro vias "
          "work -- i.e. a pattern whose expected answer depends on the via_in_pad option.", ""]
    judg = []
    for n, d in docs.items():
        k = d["kicad"]
        r = d["rules"]
        ch = r["channel"]
        L += [f"## {n}: {d['title']}", "", f"![{n}]({n}/{n}.png)", "",
              f"**Rules.** pitch {r['pitch']}, pad {r['pad']} (round SMD), track {r['track']}, clearance {r['clearance']}, "
              f"via {r['via'][0]} / {r['via'][1]}. Sources: {r['source']}.", "",
              f"**Channels.** Between two balls: gap {ch['ball_gap']}, one track needs {ch['one_track_needs']} -> "
              f"{ch['tracks_between_balls']} track(s). Between two vias one pitch apart: gap {ch['via_gap']} -> "
              f"{ch['tracks_between_vias']}. Via in a cell: {ch['via_in_cell_gap']} mm to the four balls.", "",
              f"**Expected.** {d['expected_summary']}", "", "| ring | balls | escape layer | vias |", "|---|---|---|---|"]
        for key, pr in d["expected"]["per_ring"].items():
            L.append(f"| {key} | {pr['balls']} | {', '.join(f'{v} x {k_}' for k_, v in pr['escape'].items())} | {pr['vias']} |")
        L += ["", f"**Why.** {d['why']}", ""]
        if d.get("kicad_dru"):
            L += ["**.kicad_dru** (next to the .kicad_pro):", "", "```", d["kicad_dru"].rstrip(), "```", ""]
        if d.get("alternative"):
            a = d["expected"]["alternative"]
            L += [f"**Alternative (ruled out).** {d['alternative_label']}: {a['vias']} vias, {a['length_mm']:.1f} mm, price "
                  f"{a['price']:.1f}. {d['alternative_why']}. Rules it breaks: {d.get('alternative_rules') or '-'} "
                  f"(checker: {d.get('alternative_rule_faults') or 'no fault'}).", ""]
        L += ["**KiCad DRC.** " + ", ".join(f"{kk.replace('_drc', '')} {vv.get('violations')} / {vv.get('unconnected')}"
                                             for kk, vv in k.items()), ""]
        tp = Path(out) / n / "topor" / "result.json"
        if tp.exists():
            t = json.loads(tp.read_text())
            L += [f"**topor** (`{n}/topor/`, {t.get('settings')}): {t.get('summary')}", "",
                  f"![{n} topor]({n}/topor/{n}-vs-expected.png)", "", f"Reading: {TOPOR_NOTES.get(n, '')}",
                  f"KiCad render: `{n}/topor/{n}-compare.png` (yellow = new DRC violations).", ""]
        for j in d["judgement_calls"]:
            judg.append(f"- {n}: {j}")
    L += ["## Rule 1 in the ef43e25 runs (\"traces through vias of other pads\")", "", R1_FORENSICS, "",
          "## Judgement calls needing the user", ""] + judg + ["", "## Literature", ""]
    seen = []
    for d in docs.values():
        for x in d["literature"]:
            if x not in seen:
                seen.append(x)
    L += [f"- {x}" for x in seen]
    p = Path(out) / "README.md"
    p.write_text("\n".join(L) + "\n")
    return p


# ----------------------------------------------------------------------------------------------- analyse a routed board
def load_copper(pcb: Path) -> Sol:
    """Tracks and vias of a routed board, plus its pads (`sol.pads`: [(net, layers, Pad, ref)]) so the rule checker
    sees the board's own destination pads."""
    b = pcbnew.LoadBoard(str(pcb))
    tracks, vias = [], []
    pads = []
    for fp in b.GetFootprints():
        for p in fp.Pads():
            pos = (p.GetPosition().x / MM, p.GetPosition().y / MM)
            drill = p.GetDrillSize().x / MM if p.GetAttribute() == pcbnew.PAD_ATTRIB_PTH else None
            ly = LAYERS4 if drill else [b.GetLayerName(l) for l in p.GetLayerSet().CuStack()]
            pads.append((p.GetNetname(), ly, Pad(p.GetNumber(), p.GetNetname(), pos, (p.GetSize().x / MM,) * 2,
                                                  shape="circle", drill=drill), fp.GetReference()))
    for t in b.GetTracks():
        if t.GetClass() == "PCB_VIA":
            vias.append(Via(t.GetNetname(), (t.GetPosition().x / MM, t.GetPosition().y / MM), t.GetWidth(pcbnew.F_Cu) / MM,
                            t.GetDrillValue() / MM))
        elif t.GetClass() == "PCB_ARC":
            pts = [(t.GetStart().x / MM, t.GetStart().y / MM), (t.GetMid().x / MM, t.GetMid().y / MM),
                   (t.GetEnd().x / MM, t.GetEnd().y / MM)]
            tracks.append(Track(t.GetNetname(), b.GetLayerName(t.GetLayer()), t.GetWidth() / MM, pts))
        else:
            tracks.append(Track(t.GetNetname(), b.GetLayerName(t.GetLayer()), t.GetWidth() / MM,
                                [(t.GetStart().x / MM, t.GetStart().y / MM), (t.GetEnd().x / MM, t.GetEnd().y / MM)]))
    sol = Sol("routed", tracks, vias)
    sol.pads = pads
    return sol


def compare(pat: BPat, sol: Sol, drc: dict | None = None) -> dict:
    """Per ball: escape layer(s) through the BGA box edge, vias of its net (count, nearest cell centre), length vs
    reference; plane nets: vias per ball and plane-layer traces."""
    x0, y0, x1, y1 = pat.bga_box()
    edge = box(x0, y0, x1, y1).exterior
    ref_len = net_lengths(pat.ref)
    got_len = net_lengths(sol)
    cells = [pat.cell_xy(cx + 0.5, cy + 0.5) for cy in range(pat.n - 1) for cx in range(pat.n - 1)]
    ch = pat.channel()
    slack = r4(max(0.0, ch["via_in_cell_gap"] - pat.rules.clear) + 0.01)  # how far a via may leave the cell centre
    rows, marks = [], []
    why_of: dict = {}
    for name, e in pat.expect.items():
        net = e["net"]
        c, r = int(name[1:]) - 1, ROWS.index(name[0])
        bpos = pat.ball_xy(c, r)
        if net in ("GND", "VCC"):
            vs = [v for v in sol.vias if v.net == net and math.dist(v.pos, bpos) < 1.5 * pat.rules.pitch]
            own = [v for v in vs if math.dist(v.pos, bpos) <= pat.rules.pitch / SQ2 + 0.05]
            off = [r4(min(math.dist(v.pos, q) for q in cells)) for v in own]
            ok_cell = bool(off) and min(off) <= VIA_CENTRE_TOL
            rows.append({"ball": name, "net": net, "expected": "own via in a cell", "vias_near": len(own),
                         "via_cell_offset": off, "ok": len(own) >= 1 and ok_cell})
            continue
        layers = sorted({t.layer for t in sol.tracks if t.net == net and LineString(t.pts).intersects(edge)})
        nv = [v for v in sol.vias if v.net == net]
        in_box = [v for v in nv if x0 <= v.pos[0] <= x1 and y0 <= v.pos[1] <= y1]
        cell_off = [r4(min(math.dist(v.pos, q) for q in cells)) for v in in_box]
        L, Lr = got_len.get(net, 0.0), ref_len.get(net, 0.0)
        detour = L > min(Lr + 1.0, 1.15 * Lr) + 1e-6
        ok_layer = layers == [e["layer"]]
        ok_vias = len(nv) == e["vias"]
        off_cell = [o for o in cell_off if o > slack]
        rows.append({"ball": name, "net": net, "ring": e["ring"], "expected_layer": e["layer"], "escape_layers": layers,
                     "expected_vias": e["vias"], "vias": len(nv), "via_cell_offset": cell_off, "length": r4(L),
                     "reference": Lr, "ratio": r4(L / Lr) if Lr else None, "detour": detour,
                     "via_off_cell": bool(off_cell), "ok": ok_layer and ok_vias and not detour})
        why = []
        if not ok_layer:
            why.append(f"{'/'.join(l.replace('.Cu', '') for l in layers) or '-'} (exp {e['layer'].replace('.Cu', '')})")
        if not ok_vias:
            why.append(f"{len(nv)} via (exp {e['vias']})")
        if detour:
            why.append(f"x{L / Lr:.2f}")
        why_of[name] = why
    # the user's rules: faults per ball (R1 by the track's ball, R2 / R3 / R4 / R5 by the via's ball) and at their spot
    faults = rule_faults(pat, sol, drc)
    for f in faults:
        if f.get("ball") in pat.expect:
            tag = {"R1": "R1 over a foreign via", "R2": "R2 via off centre", "R3": "R3 dog-bone direction",
                   "R4": "R4 plane via", "R5": "R5 via-in-pad", "R7": "R7 plane layer", "short": "short"}[f["rule"]]
            if tag not in why_of.setdefault(f["ball"], []):
                why_of[f["ball"]].append(tag)
    for row in rows:
        if why_of.get(row["ball"]):
            row["ok"] = False
    for name, why in why_of.items():
        if why:
            c, r = int(name[1:]) - 1, ROWS.index(name[0])
            marks.append({"pos": pat.ball_xy(c, r), "text": f"{name}: " + ", ".join(why), "r": pat.rules.pitch * 0.45})
    spots = [{"pos": f["pos"], "rule": f["rule"], "text": f["text"]} for f in faults if f["rule"] in ("R1", "short", "R2", "R3")]
    sig = [r for r in rows if "escape_layers" in r]
    summ = {"vias": len(sol.vias), "expected_vias": len(pat.ref.vias),
            "balls_as_expected": sum(1 for r in rows if r["ok"]), "balls": len(rows),
            "escape_layer_mismatch": [r["ball"] for r in sig if r["escape_layers"] != [r["expected_layer"]]],
            "via_count_mismatch": [r["ball"] for r in sig if r["vias"] != r["expected_vias"]],
            "detours": [(r["ball"], r["ratio"]) for r in sig if r["detour"]],
            "plane_balls_without_own_via": [r["ball"] for r in rows if "vias_near" in r and not r["vias_near"]],
            "vias_off_cell": [r["ball"] for r in rows if r.get("via_off_cell") or ("vias_near" in r and r["vias_near"]
                                                                                    and not r["ok"])],
            "via_cell_slack_mm": slack,
            "length_mm": r4(sum(got_len.values())), "reference_mm": r4(sum(ref_len.values())),
            "max_via_cell_offset": max([o for r in sig for o in r["via_cell_offset"]], default=None),
            "plane_layer_tracks": sum(1 for t in sol.tracks if t.layer in ("In1.Cu", "In2.Cu")),
            "faults_by_rule": {k: len(v) for k, v in sorted(faults_by_rule(faults).items())}}
    return {"summary": summ, "balls": rows, "marks": marks, "faults": faults, "spots": spots}


def render_compare(pat: BPat, sol: Sol, cmp: dict, out: Path, label: str) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig = plt.figure(figsize=(13.5, 15.4))
    axs = [fig.add_axes([0.04, 0.565, 0.44, 0.375]), fig.add_axes([0.53, 0.565, 0.44, 0.375]),
           fig.add_axes([0.04, 0.185, 0.44, 0.355]), fig.add_axes([0.53, 0.185, 0.44, 0.355])]
    x0, y0, x1, y1 = pat.bga_box()
    m = 1.2
    win = (x0 - m, y0 - m, x1 + m, y1 + m)
    s = cmp["summary"]
    pr = price(pat.ref)
    spots = [dict(sp, label={"R1": "R1", "short": "S"}.get(sp["rule"])) for sp in cmp.get("spots", [])]
    draw(axs[0], pat, pat.ref, title=f"EXPECTED: {pr['vias']} vias, {pr['length_mm']:.1f} mm (rules R1-R7: 0 faults)")
    draw(axs[1], pat, sol, marks=cmp["marks"], spots=spots,
         title=f"{label}: {s['vias']} vias, {s['length_mm']:.1f} mm; magenta = ball not as expected / rule fault")
    draw(axs[2], pat, pat.ref, window=win, cells=True, title="EXPECTED, BGA zoom")
    draw(axs[3], pat, sol, window=win, cells=True, marks=cmp["marks"], spots=spots,
         title=f"{label}, BGA zoom (circle: ball, x: R1 track over a foreign via / S short, square: R2 / R3 via)")
    legend(fig, pat, y=0.15)
    fig.suptitle(f"{pat.name}: expected vs {label}", fontsize=12, y=0.985)
    fb = faults_by_rule(cmp.get("faults", []))
    lines = [f"balls as expected {s['balls_as_expected']}/{s['balls']}; vias {s['vias']} (expected {s['expected_vias']}); "
             f"escape layer differs: {len(s['escape_layer_mismatch'])} {s['escape_layer_mismatch'][:12]}; via count differs: "
             f"{len(s['via_count_mismatch'])}; detours (> ref + 1 mm / x1.15): {len(s['detours'])} {s['detours'][:8]}; "
             f"max via offset from an open-space centre {s['max_via_cell_offset']} mm; tracks on plane layers "
             f"{s['plane_layer_tracks']}",
             "faults by rule: " + (", ".join(f"{k} {len(v)}" for k, v in sorted(fb.items())) or "none")]
    for k in ("R1", "short", "R2", "R3", "R4", "R5", "R7"):
        if k in fb:
            lines.append(f"{k} ({RULES_TEXT[k].split(' (')[0]}): " + "; ".join(f["text"] for f in fb[k][:10])
                         + (f"; ... {len(fb[k]) - 10} more" if len(fb[k]) > 10 else ""))
    lines.append("magenta balls: " + "; ".join(mk["text"] for mk in cmp["marks"]))
    txt = [w for ln in lines for w in textwrap.wrap(ln, 230)]
    fig.text(0.015, 0.004, "\n".join(txt[:22]), fontsize=6.0, ha="left", va="bottom")
    fig.savefig(out, dpi=110)
    plt.close(fig)
    return out


def analyse(name: str, routed: Path, out_png: Path, label: str = "topor", drc_json: Path | None = None) -> dict:
    pat = build(BUILDERS[name]())
    sol = load_copper(Path(routed))
    drc = json.loads(Path(drc_json).read_text()) if drc_json and Path(drc_json).exists() else None
    cmp = compare(pat, sol, drc)
    render_compare(pat, sol, cmp, Path(out_png), label)
    return cmp


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == "analyse":
        drc_json = Path(argv[3]).parent / f"{argv[1]}--drc-routed.json"
        cmp = analyse(argv[1], Path(argv[2]), Path(argv[3]), *(argv[4:5] or []), drc_json=drc_json)
        out = Path(argv[3]).with_suffix(".json")
        out.write_text(json.dumps(cmp, indent=1))
        mf = Path(argv[3]).parent / f"{argv[1]}--metrics.json"
        if mf.exists():  # a route_board run: one result.json for the README
            m = json.loads(mf.read_text())
            s = cmp["summary"]
            keep = ("routed", "connections", "unconnected", "routing_new", "drc_by_type_new", "vias", "vias_hand", "detour",
                    "runtime_s", "router_s", "router_cpu_s", "timeout_s", "timeout_clock", "status", "stop_reason",
                    "cleanup_unstable", "engine_version")
            txt = (f"{m['routed']}/{m['connections']} routed, KiCad DRC {m['routing_new']} new routing violations / "
                   f"{m['unconnected']} unconnected, {m['vias']} vias (expected {m['vias_hand']}); balls as expected "
                   f"{s['balls_as_expected']}/{s['balls']}; escape layer differs on {len(s['escape_layer_mismatch'])}, via "
                   f"count on {len(s['via_count_mismatch'])}, detours {len(s['detours'])}"
                   + (f" {s['detours'][:6]}" if s["detours"] else "")
                   + (f", plane balls without own via {s['plane_balls_without_own_via']}" if s["plane_balls_without_own_via"]
                      else "")
                   + "; faults by rule " + (", ".join(f"{k} {v}" for k, v in s["faults_by_rule"].items()) or "none")
                   + f"; {s['length_mm']:.1f} mm (expected {s['reference_mm']:.1f}); router {m['router_s']} s")
            res = {"settings": "defaults + [optimize] loop = \"topor\"", "summary": txt, "metrics": {k: m.get(k) for k in keep},
                   "analysis": s, "faults": cmp["faults"]}
            (Path(argv[3]).parent / "result.json").write_text(json.dumps(res, indent=1) + "\n")
            print(txt)
        print(json.dumps(cmp["summary"], indent=1))
        return 0
    if argv and argv[0] == "readme":  # README from the expected.json files already written (no DRC, no boards)
        out = Path(argv[1])
        docs = {n: json.loads((out / n / "expected.json").read_text()) for n in NAMES}
        print(readme(out, docs))
        return 0
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "bga-patterns"
    docs = {}
    names = argv[1:] or NAMES
    for n in names:
        g = generate(n, out)
        docs[n] = json.loads(g["expected"].read_text())
        print(n, docs[n]["kicad"], docs[n]["expected"]["reference"])
    if set(names) == set(NAMES):
        print(readme(out, docs))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
