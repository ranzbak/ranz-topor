"""BGA fan-out test patterns (user 2026-11-12): small 4-layer boards with one 6 x 6 BGA and a row of through-hole
destination pads on each side, so every ball has somewhere to go. Each pattern has a known optimal fan-out.

Three patterns, all 4 layers, stackup F.Cu signal / In1.Cu GND plane / In2.Cu power plane / B.Cu signal (topor's
default roles for 4 copper layers, `plugin/topor/layers.py` DEFAULTS_BY_COUNT; KiCad layer types left "signal"):

  bga_one_layer   1.0 mm pitch, 0.50 mm pads, 0.15 track / 0.15 clearance: ONE track fits between two balls. A 6 x 6
                  array then escapes completely on F.Cu: 0 vias (Luzin & Polubasov 2008, PDF p. 1: 4(n-1) channels >=
                  (n-2)^2 inner balls holds up to n = 6).
  bga_plane_balls The same array; the 4 centre balls are GND and the 4 corners of ring 1 are VCC, with a GND plane
                  on In1.Cu and a VCC plane on In2.Cu. Every plane ball gets its own via in the cell next to it (one
                  stub, the plane is the terminal, no trace on the plane layer); the 28 signal balls escape on F.Cu.
  bga_dogbone     0.8 mm pitch, 0.40 mm pads, 0.15 / 0.15: NO track fits between two balls (0.40 < 0.45), so ring 0
                  escapes on F.Cu and each of the 16 inner balls dives through its own via at a cell centre (dog-bone)
                  and escapes on B.Cu. No track fits between two vias at one pitch either, so each side keeps one
                  outer cell free for the ring-2 ball behind it.

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
            routes.append([it, [["F.Cu", [b, cpos]], [it.layer, [cpos, (cpos[0], y_s)]]], [cpos], (it.layer, cpos[0])])
        elif it.kind == "plane":
            routes.append([it, [["F.Cu", [b, cpos]]], [cpos], None])
        else:
            raise ValueError(it.kind)
    esc = sorted([rt for rt in routes if rt[3] is not None], key=lambda rt: rt[3][1])
    xs = [rt[3][1] for rt in esc]
    mid = (min(xs) + max(xs)) / 2
    m = len(esc)
    dx = [mid + (k - (m - 1) / 2) * DEST_PITCH for k in range(m)]
    dmax = max(abs(a - b) for a, b in zip(dx, xs))
    y_d = r4(y_s - dmax - DEST_PAD / 2 - R.clear - 0.45)
    pads = []
    for k, rt in enumerate(esc):
        x_e = rt[3][1]
        x_d = dx[k]
        seg = rt[1][-1]
        seg[1] = dedup(seg[1] + [(x_d, y_s - abs(x_d - x_e)), (x_d, y_d)])
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
        for t in sol.tracks:
            out.append((t.net, t.layer, LineString(t.pts).buffer(t.width / 2, quad_segs=16), "track", None))
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
                if d < R.clear - 1e-6:
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
                judgement=[
                    "the user's suggested (a) (outer two rows on F.Cu, inner balls dog-bone) is not the optimum for a 6 x 6 with "
                    "one track per channel: Luzin p. 1 shows it escapes on one layer; it is drawn as the alternative (4 vias, "
                    "+100 mm). Luzin p. 4 / TopoR's BGA mode asks 'every ball except the outer rows its own via': read here as "
                    "a fallback when channels run out, not as a rule that adds vias where the channels are free",
                    "each side: 5 ring-0 balls straight out, 3 ring-1 balls through the channel at their outward-diagonal "
                    "cell, 1 centre ball through the remaining channel; the 4 sides are rotations, so any rotation or mirror "
                    "of the assignment is equally optimal",
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
               alt_why="1 GND via instead of 4 (-75 mm price), but four balls hang on one barrel: Luzin p. 4 asks every "
                       "ball its own via and no jumpers between neighbouring balls",
               judgement=[
                   "own via per plane ball (Luzin p. 4) rather than the cheapest count; the shared-via alternative is drawn "
                   "(1 GND via, 4 stubs, KiCad DRC clean). Make it an option? (default: own via)",
                   "dog-bone (stub to the cell centre, via there) rather than via-in-pad: Board Setup has no via-in-pad / "
                   "filled-via capability flag; the hand boards split: HaasoscopePro, ButterStick U4, Glasgow use via-in-pad, "
                   "ULX3S, ThunderScope, Zynq U2 dog-bone. Via-in-pad would be 0 stub length",
                   "each plane via sits in the cell on the ball's outward diagonal (away from the package centre), as in "
                   "most ULX3S dog-bones; the cell must not take a channel a signal needs (here none does)",
                   "VCC on the 4 ring-1 corners shows a plane via in ring 1 next to signal channels; the plane nets are "
                   "power nets only by name (same 0.15 mm stub as the signals, net class Default)"],
               literature=[LIT["luzin_p2"], LIT["luzin_p3"], LIT["luzin_p4"], LIT["topor63"], LIT["planes"], LIT["power_via"]])
    return pat


def p_dogbone() -> BPat:
    n = 6
    top = _ring0_top(n) + [Item((1, 1), "dogbone", (0.5, 0.5), "B.Cu"), Item((2, 1), "dogbone", (2.5, 0.5), "B.Cu"),
                           Item((3, 1), "dogbone", (3.5, 0.5), "B.Cu"), Item((2, 2), "dogbone", (1.5, 1.5), "B.Cu")]
    return BPat("bga_dogbone", "6 x 6, 0.8 mm, no track between balls: ring 0 on F.Cu, 16 dog-bones to B.Cu", R_08, n, top,
                judgement=[
                    "In1.Cu / In2.Cu are planes by topor's default 4-layer roles (signal / gnd / pwr / signal), so the inner "
                    "balls escape on B.Cu; with all four layers routable, In1 or In2 would be equally good",
                    "via cells: ring-1 balls on their outward-diagonal outer cell, the ring-2 ball one cell further in; on "
                    "B.Cu no track passes between two vias one pitch apart (0.40 < 0.45), so each side keeps one outer cell "
                    "free for the ring-2 wire. Other free-cell choices (rotations, mirrors) are equally optimal",
                    "0.15 / 0.15 rules with 0.40 pads at 0.8 mm is the zero-channel case (6-mil fab); with the 0.127 / 0.127 "
                    "the survey boards use, one track fits and the 6 x 6 escapes on F.Cu like bga_one_layer",
                    "stub = ball centre to cell centre at 45 degrees (0.566 mm); Luzin p. 4: keep it short so it blocks "
                    "no neighbour"],
                literature=[LIT["luzin_p1"], LIT["luzin_p4"], LIT["topor50"], LIT["haas2026"], LIT["power_via"]])


BUILDERS = {"bga_one_layer": p_one_layer, "bga_plane_balls": p_plane_balls, "bga_dogbone": p_dogbone}
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


def write_pat(pat: BPat, sol: Sol | None, path: Path) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = build_board(_View(pat, sol), copper=sol is not None)
    add_zones(b, pat)
    pcbnew.SaveBoard(str(path), b)
    write_project_file(path, rules=pat.rules.kicad(), classes=pat.rules.classes())
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
         labels="auto"):
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
                    ax.text(p.pos[0], p.pos[1], p.num, fontsize=4.6, ha="center", va="center", color="k", zorder=7)
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
        ch = pat.rules.channel()
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
    ch = pat.rules.channel()
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
                       "and no trace beyond it. Each plane ball gets its own via in the cell on its outward diagonal (Luzin "
                       "p. 4: own via per ball, no jumpers between neighbours, short stub). The signal balls keep the "
                       "bga_one_layer channels (the centre is power now, Luzin p. 3 fig. 7), so they stay on F.Cu with 0 vias.",
    "bga_dogbone": "No track fits between two balls (gap 0.40 < 0.45): ring 0 encloses every inner ball on F.Cu, so each "
                   "of the 16 inner balls needs one via (proved minimum), and a via must sit in a cell next to its ball "
                   "(the 0.40 via fits a cell with 0.166 mm to the balls). On B.Cu only vias are in the way and no track "
                   "passes between two vias one pitch apart, so the ring-1 vias of a side take 3 of the 5 outer cells and "
                   "leave the one in front of the ring-2 ball free; every wire then runs straight out to the pad row.",
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
    kicad = {}
    for key in ("problem", "reference", "alternative"):
        if key in paths:
            kicad[f"{key}_drc"] = drc_of(paths[key]) if drc else {}
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
                     "channel": pat.rules.channel(), "via_price_mm": VIA_PRICE},
           "balls": pat.expect, "pins": pins,
           "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "expected": exp, "built_check": built, "judgement_calls": pat.judgement, "literature": pat.literature,
           "alternative_label": pat.alt_label, "alternative_why": pat.alt_why,
           "reference": sol_doc(pat.ref), "alternative": sol_doc(pat.alt), "kicad": kicad}
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


TOPOR_NOTES = {  # what the one topor run (main 0f36b1e engine, 2026-11-12) did, read off the PNGs and KiCad DRC
    "bga_one_layer": "Every one of the 16 inner balls gets a via and escapes on B.Cu (expected: 0 vias, all on F.Cu "
                     "through the free channels). The vias sit next to the balls, 0.1-0.5 mm off the cell centres, and "
                     "4 of them short against B.Cu wires of the neighbouring ball (shorting_items 4, clearance 8, "
                     "hole_clearance 4). C4 detours (x1.34). Engine finding VIA_SQUEEZE.",
    "bga_plane_balls": "DRC clean. The 8 ring-1 signal balls get a via each and escape on B.Cu (expected F.Cu, 0 vias): "
                       "16 vias instead of 8 (price 492.5 vs 297.1). Plane vias: one per ball, but not in the cell "
                       "centres (the GND ones drift toward the package centre).",
    "bga_dogbone": "Topology right: ring 0 on F.Cu, 16 vias, inner balls on B.Cu, the same 181.6 mm. Via placement wrong: "
                   "every via 0.1-0.43 mm off its cell centre (a 0.40 via has 0.016 mm slack in a 0.8 mm cell), so vias "
                   "touch foreign balls and B.Cu wires: 13 new DRC violations (shorting_items 7, clearance 4, "
                   "hole_clearance 2). Engine finding VIA_SQUEEZE.",
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
         "crossing, no forced via). Price = mm + 25 mm per via. Copper to edge 0.50, hole clearance 0.20.", ""]
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
        if d.get("alternative"):
            a = d["expected"]["alternative"]
            L += [f"**Alternative (ruled out).** {d['alternative_label']}: {a['vias']} vias, {a['length_mm']:.1f} mm, price "
                  f"{a['price']:.1f}. {d['alternative_why']}.", ""]
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
    L += ["## Judgement calls needing the user", ""] + judg + ["", "## Literature", ""]
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
    b = pcbnew.LoadBoard(str(pcb))
    tracks, vias = [], []
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
    return Sol("routed", tracks, vias)


def compare(pat: BPat, sol: Sol) -> dict:
    """Per ball: escape layer(s) through the BGA box edge, vias of its net (count, nearest cell centre), length vs
    reference; plane nets: vias per ball and plane-layer traces."""
    x0, y0, x1, y1 = pat.bga_box()
    edge = box(x0, y0, x1, y1).exterior
    ref_len = net_lengths(pat.ref)
    got_len = net_lengths(sol)
    cells = [pat.cell_xy(cx + 0.5, cy + 0.5) for cy in range(pat.n - 1) for cx in range(pat.n - 1)]
    ch = pat.rules.channel()
    slack = r4(max(0.0, ch["via_in_cell_gap"] - pat.rules.clear) + 0.01)  # how far a via may leave the cell centre
    rows, marks = [], []
    for name, e in pat.expect.items():
        net = e["net"]
        c, r = int(name[1:]) - 1, ROWS.index(name[0])
        bpos = pat.ball_xy(c, r)
        if net in ("GND", "VCC"):
            vs = [v for v in sol.vias if v.net == net and math.dist(v.pos, bpos) < 1.5 * pat.rules.pitch]
            own = [v for v in vs if math.dist(v.pos, bpos) <= pat.rules.pitch / SQ2 + 0.05]
            off = [r4(min(math.dist(v.pos, q) for q in cells)) for v in own]
            ok_cell = bool(off) and min(off) <= slack
            rows.append({"ball": name, "net": net, "expected": "own via in a cell", "vias_near": len(own),
                         "via_cell_offset": off, "ok": len(own) >= 1 and ok_cell})
            if not rows[-1]["ok"]:
                marks.append({"pos": bpos, "text": f"{name}: " + (f"via {min(off):.2f} off the cell centre" if own else
                                                                  "no own via"), "r": pat.rules.pitch * 0.45})
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
                     "via_off_cell": bool(off_cell), "ok": ok_layer and ok_vias and not detour and not off_cell})
        why = []
        if not ok_layer:
            why.append(f"{'/'.join(l.replace('.Cu', '') for l in layers) or '-'} (exp {e['layer'].replace('.Cu', '')})")
        if not ok_vias:
            why.append(f"{len(nv)} via (exp {e['vias']})")
        if detour:
            why.append(f"x{L / Lr:.2f}")
        if off_cell:
            why.append(f"via {max(off_cell):.2f} off the cell centre")
        if why:
            marks.append({"pos": bpos, "text": f"{name}: " + ", ".join(why), "r": pat.rules.pitch * 0.45})
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
            "plane_layer_tracks": sum(1 for t in sol.tracks if t.layer in ("In1.Cu", "In2.Cu"))}
    return {"summary": summ, "balls": rows, "marks": marks}


def render_compare(pat: BPat, sol: Sol, cmp: dict, out: Path, label: str) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig = plt.figure(figsize=(13.5, 14.2))
    axs = [fig.add_axes([0.04, 0.53, 0.44, 0.40]), fig.add_axes([0.53, 0.53, 0.44, 0.40]),
           fig.add_axes([0.04, 0.13, 0.44, 0.38]), fig.add_axes([0.53, 0.13, 0.44, 0.38])]
    x0, y0, x1, y1 = pat.bga_box()
    m = 1.2
    win = (x0 - m, y0 - m, x1 + m, y1 + m)
    s = cmp["summary"]
    pr = price(pat.ref)
    draw(axs[0], pat, pat.ref, title=f"EXPECTED: {pr['vias']} vias, {pr['length_mm']:.1f} mm")
    draw(axs[1], pat, sol, marks=cmp["marks"],
         title=f"{label}: {s['vias']} vias, {s['length_mm']:.1f} mm; magenta = ball not as expected")
    draw(axs[2], pat, pat.ref, window=win, cells=True, title="EXPECTED, BGA zoom")
    draw(axs[3], pat, sol, window=win, cells=True, marks=cmp["marks"], title=f"{label}, BGA zoom (magenta: why)")
    legend(fig, pat, y=0.075)
    fig.suptitle(f"{pat.name}: expected vs {label}", fontsize=12, y=0.985)
    lines = [f"balls as expected {s['balls_as_expected']}/{s['balls']}; vias {s['vias']} (expected {s['expected_vias']}); "
             f"escape layer differs: {len(s['escape_layer_mismatch'])} {s['escape_layer_mismatch'][:12]}; via count differs: "
             f"{len(s['via_count_mismatch'])}; detours (> ref + 1 mm / x1.15): {len(s['detours'])} {s['detours'][:8]}; "
             f"plane balls without own via: {s['plane_balls_without_own_via']}; vias off their cell centre (> "
             f"{s['via_cell_slack_mm']} mm): {len(s['vias_off_cell'])}; max via offset from a cell centre "
             f"{s['max_via_cell_offset']} mm; tracks on plane layers {s['plane_layer_tracks']}"]
    lines.append("magenta balls: " + "; ".join(mk["text"] for mk in cmp["marks"]))
    txt = [w for ln in lines for w in textwrap.wrap(ln, 210)]
    fig.text(0.02, 0.005, "\n".join(txt[:7]), fontsize=6.6, ha="left", va="bottom")
    fig.savefig(out, dpi=110)
    plt.close(fig)
    return out


def analyse(name: str, routed: Path, out_png: Path, label: str = "topor") -> dict:
    pat = build(BUILDERS[name]())
    sol = load_copper(Path(routed))
    cmp = compare(pat, sol)
    render_compare(pat, sol, cmp, Path(out_png), label)
    return cmp


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == "analyse":
        cmp = analyse(argv[1], Path(argv[2]), Path(argv[3]), *(argv[4:5] or []))
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
                   + f", vias off their cell centre {len(s['vias_off_cell'])}"
                   + f"; {s['length_mm']:.1f} mm (expected {s['reference_mm']:.1f}); router {m['router_s']} s")
            res = {"settings": "defaults + [optimize] loop = \"topor\"", "summary": txt, "metrics": {k: m.get(k) for k in keep},
                   "analysis": s}
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
