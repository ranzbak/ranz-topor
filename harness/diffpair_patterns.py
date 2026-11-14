"""Diff-pair test patterns (shopping list 5.6): gateway, polarity swap, swap without room, U-turn parity, bus crossing,
GND pad between P and N with room behind (HDMI connector row) and a flow-through ESD chip (GND via between P and N,
the smallest the Board Setup allows).

Four small 2-layer boards, each with a USB-like pair `USB_P` / `USB_N` (KiCad pair naming, `MatchDpSuffix`, see
docs/dev/kicad-diff-pairs.md section 1.1). The answers follow the user's rules of 2026-11-08 .. 2026-11-10:

1. ORDER: a pair counts as wide as 2 clearance + 2 width + gap when the wires are sorted widest first.
2. PAIR ENDS THE KICAD WAY: two tracks start at the P and N pad, run straight in the pads' exit direction until
   the convergence is DRC clean, then two symmetric 45 degree inward segments reach the rule gap; the end of that
   convergence is the GATEWAY. From there on the gap is constant; the other end mirrors it. (KiCad: docs/dev/
   kicad-diff-pairs.md section 3.2.)
3. POLARITY SWAP: when P and N would arrive swapped, the pair changes layer at a via pair and TURNS BACK (it leaves
   the vias on the side it arrived from), which puts P on the other side without a same-layer crossing; a second via
   pair brings it back to the surface ("one pair to do the swap, one pair to bring the traces back to the surface":
   4 vias in all). A turn-back at ANY layer change counts as a swap (parity). Priced in the topology.
4. NO ROOM: default `[diffpair] swap = "via_pair"` takes the CHEAPER of the swap (4 vias) and two single tracks
   (uncoupled, reported); `via_pair_always` forces the swap.
5. RULE VALUES: pair routing uses the net class's diff_pair_width / diff_pair_gap / diff_pair_via_gap, NOT the
   class's normal track_width / clearance / via size; the class values are chosen different on purpose.
6. VIAS: the smallest drill the supplier allows (Board Setup minimums) with pi * drill >= the trace width, diameter =
   drill + 2 min annular ring (or the min via diameter if larger).

Rule values (state of every pattern board, written to its .kicad_pro):
  Default class        clearance 0.20  track 0.25  via 0.60 / 0.30
  USB class (USB_*)    clearance 0.20  track 0.25  via 0.60 / 0.30 (normal values: never used by a coupled stretch)
                       diff_pair_width 0.20  diff_pair_gap 0.15  diff_pair_via_gap 0.25
  Board Setup minimums track 0.20, clearance 0.20, via diameter 0.45, through-hole drill 0.25, annular ring 0.10,
                       hole to hole 0.25, copper to hole 0.25, copper to edge 0.50
  => pair centre pitch 0.35; every via 0.45 / 0.25; via pair centre pitch = 0.45 + 0.25 = 0.70.
  KiCad lets the gap (0.15) undercut the class clearance (0.20) for P against N (docs/dev/kicad-diff-pairs.md 2.2,
  `AB.isCoupledDiffPair()`), so the references pass DRC with 0 violations.

Price model (all numbers are settings): length in mm + 25 mm per via (user 2026-11-10) + U mm per mm of uncoupled
stretch. U = 2.0 is the proposal of this pattern set (needs the user's confirmation).

Solvable by construction: the reference copper is drawn first from centre lines and the pads are derived from it;
KiCad DRC 0 violations / 0 unconnected. `python -m harness.diffpair_patterns [OUT]` writes every pattern under OUT
(default `scratch/diffpair-patterns`). Coordinates in mm, KiCad frame (y down). Deterministic.
"""
from __future__ import annotations

import json
import math
import sys
import textwrap
from dataclasses import dataclass, field
from pathlib import Path

import pcbnew
from shapely.geometry import LineString, Point, Polygon, box
from shapely.ops import unary_union

from harness.fit_patterns import EDGE, HOLE_CLEAR, RULES, Fp, Pad, Track, Via, build_board
from harness.fixtures import write_project_file

# ----------------------------------------------------------------------------------------------- rule values
CLEAR = 0.20
W_NORMAL = 0.25  # class track_width (single tracks)
VIA_CLASS = (0.60, 0.30)  # class via size: never used here
DP_W, DP_GAP, DP_VIA_GAP = 0.20, 0.15, 0.25
MIN_ANNULAR, MIN_DRILL, MIN_VIA_D = 0.10, 0.25, 0.45
PITCH = DP_W + DP_GAP  # 0.35 track centre pitch of a coupled pair
HALF = PITCH / 2
BAND = 2 * CLEAR + 2 * DP_W + DP_GAP  # user rule 1: width of a pair for ordering = 0.95
VIA_DRILL = MIN_DRILL
VIA_D = max(MIN_VIA_D, VIA_DRILL + 2 * MIN_ANNULAR)  # 0.45
VIA_R = VIA_D / 2
VIA_PITCH = VIA_D + max(DP_VIA_GAP, 0.25 - 2 * MIN_ANNULAR, HOLE_CLEAR - MIN_ANNULAR)  # EffectiveDiffPairViaGap: 0.70
VIA_PRICE = 25.0
UNCOUPLED_PRICE = 2.0  # U: extra mm per uncoupled mm (proposal)
SQ2 = math.sqrt(2.0)
NETS = ["USB_P", "USB_N"]
CLASS_USB = {"name": "USB", "clearance": CLEAR, "track_width": W_NORMAL, "via_diameter": VIA_CLASS[0],
             "via_drill": VIA_CLASS[1], "priority": 0, "diff_pair_width": DP_W, "diff_pair_gap": DP_GAP,
             "diff_pair_via_gap": DP_VIA_GAP}
CLASS_DEFAULT = {"name": "Default", "clearance": CLEAR, "track_width": W_NORMAL, "via_diameter": VIA_CLASS[0],
                 "via_drill": VIA_CLASS[1], "priority": 2147483647}
RULES_DP = dict(RULES, min_track_width=DP_W, min_via_diameter=MIN_VIA_D, min_through_hole_diameter=MIN_DRILL,
                min_via_annular_width=MIN_ANNULAR, min_hole_to_hole=0.25, min_hole_clearance=HOLE_CLEAR, min_clearance=DP_GAP)
NET_PATTERNS = [{"netclass": "USB", "pattern": "USB_*"}]
COLORS = {"USB_P": "#1f77b4", "USB_N": "#ff7f0e"}
BAD, OK = "#d62728", "#138a36"
LIT = {
    "manual63": "TopoR 6.3 User Manual (docs/literature/eremex2017-topor63-user-manual.pdf), chapter 'Differential "
                "Signals': printed pp. 154-155 'Differential Signals' (create by combining two signals; max allowed "
                "difference between paired signals), p. 160 'Restoring Differential Pairs' (pairs are zipped from the "
                "design parameters; guaranteed only with arcs disabled and clearances per layer)",
    "help50": "TopoR 5.0 help (docs/literature/eremex-topor50-help.txt) lines 152 ('Differential signals can now be "
              "split automatically'), 2094 ('route wires, route differential pairs' submodes), 2560 ('Route "
              "differential pairs'), 3778-4130 (differential signals in the design parameters)",
    "kicad": "docs/dev/kicad-diff-pairs.md: 1.1 pair detection by name, 2.1-2.2 rule fields and the coupled-clearance "
             "override, 3.2 pad gateways (fan-out), 3.4 via pairs (EffectiveDiffPairViaGap), 3.5 no uncoupling fallback",
}


# ----------------------------------------------------------------------------------------------- vector helpers
def add(p, q):
    return (p[0] + q[0], p[1] + q[1])


def sub(p, q):
    return (p[0] - q[0], p[1] - q[1])


def mul(p, k):
    return (p[0] * k, p[1] * k)


def lnorm(h):
    """Left normal of a heading in the y-down frame (east -> north = (0, -1))."""
    return (h[1], -h[0])


def unit(p):
    n = math.hypot(*p)
    return (p[0] / n, p[1] / n)


def plen(pts):
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def r4(x):
    return round(x, 4)


def rp(p):
    return [r4(p[0]), r4(p[1])]


def offset_poly(pts, off):
    """Parallel polyline, `off` to the left of the travel direction (negative = right), mitred corners."""
    segs = [unit(sub(b, a)) for a, b in zip(pts, pts[1:])]
    out = [add(pts[0], mul(lnorm(segs[0]), off))]
    for i in range(1, len(pts) - 1):
        n1, n2 = lnorm(segs[i - 1]), lnorm(segs[i])
        d = 1 + n1[0] * n2[0] + n1[1] * n2[1]
        m = ((n1[0] + n2[0]) / d, (n1[1] + n2[1]) / d)
        out.append(add(pts[i], mul(m, off)))
    out.append(add(pts[-1], mul(lnorm(segs[-1]), off)))
    return out


def fan(G, u, n, sp, T, st):
    """Fan of a pair end. G = gateway (centre of the coupled body's end), u = unit direction away from the body, n =
    unit left normal of the body's heading at this end, sp = +1 when P is on the left (n side) else -1, T = target
    centre pitch (pad pitch or via pitch), st = straight run from the end of the 45 degree legs to the terminal.
    Returns (P points, N points) from the gateway outwards: symmetric 45 degree legs, then straight."""
    d = abs(T - PITCH) / 2
    s = 1.0 if T >= PITCH else -1.0
    res = []
    for side in (sp, -sp):
        p0 = add(G, mul(n, side * HALF))
        p1 = add(add(p0, mul(u, d)), mul(n, side * s * d))
        p2 = add(p1, mul(u, st))
        res.append([p0, p1, p2] if d > 1e-12 else [p0, p2])
    return res[0], res[1]


def body_pair(C, sp):
    """P and N centre lines of a coupled body along centre line C (sp = +1: P on the left of the travel direction)."""
    return offset_poly(C, sp * HALF), offset_poly(C, -sp * HALF)


# ----------------------------------------------------------------------------------------------- model
@dataclass
class Sol:
    """One drawn solution: copper tracks and vias (a real board) or schematic centre lines (no copper)."""
    label: str
    tracks: list[Track] = field(default_factory=list)
    vias: list[Via] = field(default_factory=list)
    schematic: list[tuple[str, list]] = field(default_factory=list)
    coupled: list[tuple[str, list]] = field(default_factory=list)  # (layer, centre line) of the coupled bodies
    fans: list[tuple[str, list]] = field(default_factory=list)  # (layer, polyline) of pad / via fans: not "uncoupled"
    marks: list[dict] = field(default_factory=list)
    boxes: list[dict] = field(default_factory=list)
    text: str = ""
    cost: dict = field(default_factory=dict)
    extra: dict = field(default_factory=dict)


@dataclass
class DPat:
    name: str
    title: str
    kind: str
    purpose: str
    answer: str
    why: str
    size: tuple[float, float]
    fps: list[Fp]
    ref: Sol
    alt: Sol | None
    careless: Sol | None
    expected: dict
    gateways: list[dict] = field(default_factory=list)
    walls: list[dict] = field(default_factory=list)  # obstacle outlines for the PNG: {"rect", "label"}
    connections: list[tuple[str, str, str]] = field(default_factory=lambda: [("USB_P", "J1.1", "U1.1"),
                                                                              ("USB_N", "J1.2", "U1.2")])
    zoom: list[dict] = field(default_factory=list)  # {"rect", "label"} detail windows for the zoom PNG
    layers: int = 2
    keepouts: list = field(default_factory=list)
    nets: list[str] = field(default_factory=lambda: list(NETS))
    classes: list[dict] = field(default_factory=lambda: [dict(CLASS_DEFAULT), dict(CLASS_USB)])
    net_patterns: list[dict] = field(default_factory=lambda: [dict(p) for p in NET_PATTERNS])
    rules: dict = field(default_factory=lambda: dict(RULES_DP))
    dru: str = ""
    extra: dict = field(default_factory=dict)
    zones: list[dict] = field(default_factory=list)  # copper pours {"net", "layer", "rect"} (written and filled)

    @property
    def ref_tracks(self):
        return self.ref.tracks

    @property
    def ref_vias(self):
        return self.ref.vias


class _View:
    """A DPat seen with another solution's copper (duck-typed for fit_patterns.build_board)."""

    def __init__(self, pat: DPat, sol: Sol):
        self._p, self.ref_tracks, self.ref_vias = pat, sol.tracks, sol.vias

    def __getattr__(self, k):
        return getattr(self._p, k)


# ----------------------------------------------------------------------------------------------- footprints
def pad_pair(ref, nets_pos, u, L, Wd, walls, layer="F.Cu"):
    """A connector / IC pad group: P and N pads (nets, positions) exiting along unit vector u, pad length L along
    the exit axis, width Wd across, plus no-net wall pads at `walls` positions (neighbouring pins)."""
    size = (L, Wd) if abs(u[0]) > 0.5 else (Wd, L)
    pads = [Pad(str(i + 1), net, pos, size, layer=layer) for i, (net, pos) in enumerate(nets_pos)]
    pads += [Pad(str(len(pads) + i + 1), None, pos, size, layer=layer) for i, pos in enumerate(walls)]
    return Fp(ref, pads, "terminal")


def pad_end(pos, u, L):
    return add(pos, mul(u, L / 2))


def block(ref, x0, y0, x1, y1):
    """A through-hole component body: one big rect pad with a drill = copper on BOTH layers, no net."""
    c = ((x0 + x1) / 2, (y0 + y1) / 2)
    return Fp(ref, [Pad("1", None, c, (x1 - x0, y1 - y0), drill=0.8)], "obstacle")


def assemble(segs, net, width_of):
    """segs = [(layer, pts)] in order, consecutive ones sharing the via point; returns tracks and via positions."""
    tracks, vias = [], []
    for k, (ly, pts) in enumerate(segs):
        tracks.append(Track(net, ly, width_of(ly, k), [(r4(x), r4(y)) for x, y in pts]))
        if k + 1 < len(segs):
            vias.append(segs[k + 1][1][0])
    return tracks, vias


def clean(pts):
    out = [pts[0]]
    for p in pts[1:]:
        if math.dist(p, out[-1]) > 1e-9:
            out.append(p)
    return out


def mk_sol(label, paths, widths=None):
    """paths = {net: [(layer, pts, width), ...]} with shared via points between consecutive items."""
    s = Sol(label)
    for net, segs in paths.items():
        for k, (ly, pts, w) in enumerate(segs):
            s.tracks.append(Track(net, ly, w, [(r4(x), r4(y)) for x, y in clean(pts)]))
            if k + 1 < len(segs):
                s.vias.append(Via(net, (r4(segs[k + 1][1][0][0]), r4(segs[k + 1][1][0][1])), VIA_D, VIA_DRILL))
    return s


# ----------------------------------------------------------------------------------------------- measurement
def track_len(sol: Sol, net: str) -> float:
    return sum(plen(t.pts) for t in sol.tracks if t.net == net)


def net_path(sol: Sol, net: str):
    """The ordered centre-line points of a 2-pad net through its vias (tracks chained by shared end points)."""
    ts = [t for t in sol.tracks if t.net == net]
    chain = [ts.pop(0)]
    while ts:
        for i, t in enumerate(ts):
            if math.dist(t.pts[0], chain[-1].pts[-1]) < 1e-6:
                chain.append(ts.pop(i))
                break
            if math.dist(t.pts[-1], chain[-1].pts[-1]) < 1e-6:
                t.pts.reverse()
                chain.append(ts.pop(i))
                break
            if math.dist(t.pts[-1], chain[0].pts[0]) < 1e-6:
                chain.insert(0, ts.pop(i))
                break
            if math.dist(t.pts[0], chain[0].pts[0]) < 1e-6:
                t.pts.reverse()
                chain.insert(0, ts.pop(i))
                break
        else:
            raise AssertionError(f"{net}: tracks do not chain")
    return chain


def turn_backs(sol: Sol, net: str) -> int:
    """Via pairs where the net leaves a via on the side it arrived from (heading reverses: dot product < 0)."""
    ch = net_path(sol, net)
    n = 0
    for a, b in zip(ch, ch[1:]):
        d_in = unit(sub(a.pts[-1], a.pts[-2]))
        d_out = unit(sub(b.pts[1], b.pts[0]))
        if d_in[0] * d_out[0] + d_in[1] * d_out[1] < -1e-9:
            n += 1
    return n


def uncoupled_len(sol: Sol, net: str) -> float:
    """Length of the net's tracks outside every coupled body (the bodies are the Sol.coupled centre lines)."""
    total = 0.0
    for t in sol.tracks:
        if t.net != net:
            continue
        for a, b in zip(t.pts, t.pts[1:]):
            m = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
            inside = any(ly == t.layer and Point(m).distance(LineString(c)) <= 1.5 * HALF for ly, c in sol.coupled)
            inside = inside or any(ly == t.layer and Point(m).distance(LineString(c)) < 1e-6 for ly, c in sol.fans)
            if not inside:
                total += math.dist(a, b)
    return total


def min_gap(sol: Sol, layer: str) -> float:
    """Smallest edge-to-edge distance between a P and an N track piece on `layer` (diff-pair gap in the coupled body)."""
    p = [LineString(t.pts).buffer(t.width / 2) for t in sol.tracks if t.net == "USB_P" and t.layer == layer]
    n = [LineString(t.pts).buffer(t.width / 2) for t in sol.tracks if t.net == "USB_N" and t.layer == layer]
    return min((a.distance(b) for a in p for b in n), default=math.inf)


def cost(sol: Sol, u: float = UNCOUPLED_PRICE) -> dict:
    ln = {n: track_len(sol, n) for n in NETS}
    unc = {n: uncoupled_len(sol, n) for n in NETS}
    vias = len(sol.vias)
    total = sum(ln.values()) + VIA_PRICE * vias + u * sum(unc.values())
    return {"length_P": r4(ln["USB_P"]), "length_N": r4(ln["USB_N"]), "length_sum": r4(sum(ln.values())), "vias": vias,
            "uncoupled_P": r4(unc["USB_P"]), "uncoupled_N": r4(unc["USB_N"]), "uncoupled_sum": r4(sum(unc.values())),
            "cost": r4(total), "skew_P_minus_N": r4(ln["USB_P"] - ln["USB_N"])}


def _is_pair(n1, n2) -> bool:
    """KiCad pair names (MatchDpSuffix): same name but P / N at the end."""
    return bool(n1 and n2 and n1[:-1] == n2[:-1] and {n1[-1], n2[-1]} == {"P", "N"})


def clearance_report(pat: DPat, sol: Sol) -> dict:
    """Smallest edge-to-edge distance between copper of different nets, per layer, against the clearance rule
    (0.20; P against N track: the pair gap 0.15). A shapely mini-DRC used by check(); KiCad's DRC is the authority."""
    items = []  # (layer, net, geom, kind)
    for t in sol.tracks:
        items.append((t.layer, t.net, LineString(t.pts).buffer(t.width / 2, cap_style="round", join_style="round",
                                                                 quad_segs=8), "track"))
    for v in sol.vias:
        for ly in ("F.Cu", "B.Cu"):
            items.append((ly, v.net, Point(v.pos).buffer(v.dia / 2, quad_segs=16), "via"))
    for f in pat.fps:
        for p in f.pads:
            geo = (Point(p.pos).buffer(p.size[0] / 2) if p.shape == "circle" else
                   box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2, p.pos[0] + p.size[0] / 2,
                       p.pos[1] + p.size[1] / 2))
            for ly in (("F.Cu", "B.Cu") if p.drill else (p.layer,)):
                items.append((ly, p.net, geo, "pad"))
    worst = {"margin": math.inf}
    clear = pat.extra.get("clear", CLEAR)  # the pattern's class clearance (the qmtech-rule patterns: 0.15)
    for i, a in enumerate(items):
        for b in items[i + 1:]:
            if a[0] != b[0] or a[1] == b[1] and a[1] is not None:
                continue
            need = DP_GAP if (a[3] == "track" and b[3] == "track" and _is_pair(a[1], b[1])) else clear
            d = a[2].distance(b[2])
            if d - need < worst["margin"]:
                worst = {"margin": r4(d - need), "layer": a[0], "nets": [a[1], b[1]], "kinds": [a[3], b[3]],
                         "distance": r4(d), "need": need}
    # copper to the board edge
    W, H = pat.size
    inner = box(EDGE, EDGE, W - EDGE, H - EDGE)
    edge = min((inner.exterior.distance(g) if inner.contains(g) else -1.0) for _, _, g, _ in items)
    worst["edge_margin"] = r4(edge - EDGE) if edge >= 0 else -1.0
    return worst


def connected(pat: DPat, sol: Sol, net: str) -> bool:
    """Both pads of the net are joined by the net's tracks and vias (shapely connectivity, layer aware)."""
    nodes = []  # (layer, geom, tag)
    for t in sol.tracks:
        if t.net == net:
            nodes.append((t.layer, LineString(t.pts).buffer(t.width / 2), None))
    for v in sol.vias:
        if v.net == net:
            for ly in ("F.Cu", "B.Cu"):
                nodes.append((ly, Point(v.pos).buffer(v.dia / 2), ("via", v.pos)))
    for f in pat.fps:
        for p in f.pads:
            if p.net == net:
                nodes.append((p.layer, box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2,
                                           p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2), "pad"))
    parent = list(range(len(nodes)))

    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    for i, (la, ga, ka) in enumerate(nodes):
        for j in range(i + 1, len(nodes)):
            lb, gb, kb = nodes[j]
            if (la == lb and ga.intersects(gb)) or (isinstance(ka, tuple) and ka == kb):
                parent[find(i)] = find(j)
    return len({find(i) for i, n in enumerate(nodes) if n[2] == "pad"}) == 1


# ----------------------------------------------------------------------------------------------- shared builders
def exit_fan(fps, P_pos, N_pos, h, L, pad_T, *, grid=0.05, other_pads=None, clear=CLEAR):
    """Smallest run-out d0 (grid steps beyond the pad ends) at which the two tracks, straight out of their pads in the
    travel direction h and then converging 45 degrees to the pair pitch, clear every pad that is not their own by the
    clearance. Returns (d0, gateway G, P pts pad->G, N pts pad->G)."""
    mid = ((P_pos[0] + N_pos[0]) / 2, (P_pos[1] + N_pos[1]) / 2)
    n = lnorm(h)
    sp = 1 if (P_pos[0] - N_pos[0]) * n[0] + (P_pos[1] - N_pos[1]) * n[1] > 0 else -1
    dconv = abs(pad_T - PITCH) / 2
    others = [(f, p) for f in fps for p in f.pads]
    d0 = 0.0
    while d0 < 3.0:
        G = add(mid, mul(h, L / 2 + d0 + dconv))
        pp, nn = fan(G, mul(h, -1), n, sp, pad_T, L / 2 + d0)
        ok = True
        for pts, pos in ((pp, P_pos), (nn, N_pos)):
            tr = LineString(pts).buffer(DP_W / 2, quad_segs=8)
            for f, pd in others:
                if math.dist(pd.pos, pos) < 1e-9:
                    continue  # own pad
                geo = box(pd.pos[0] - pd.size[0] / 2, pd.pos[1] - pd.size[1] / 2, pd.pos[0] + pd.size[0] / 2,
                          pd.pos[1] + pd.size[1] / 2)
                if tr.distance(geo) < clear - 1e-9:
                    ok = False
        if ok:
            return r4(d0), G, pp[::-1], nn[::-1]
        d0 = r4(d0 + grid)
    raise AssertionError("no clean run-out")


def gap_profile(sol: Sol, layer="F.Cu", step=0.05):
    """(min, max) edge-to-edge distance P to N sampled along the P track (the careless panel's gap)."""
    ps = [t for t in sol.tracks if t.net == "USB_P" and t.layer == layer]
    ns = unary_union([LineString(t.pts).buffer(t.width / 2) for t in sol.tracks if t.net == "USB_N" and t.layer == layer])
    ds = []
    for t in ps:
        ln = LineString(t.pts)
        for k in range(int(ln.length / step) + 1):
            pt = ln.interpolate(min(k * step, ln.length))
            ds.append(pt.distance(ns) - t.width / 2)
    return min(ds), max(ds)


# ----------------------------------------------------------------------------------------------- pattern 1: gateway
def p_gateway() -> DPat:
    W, H = 24.0, 14.0
    LJ, WJ = 1.1, 0.3  # connector pads (USB-C like): 0.5 pitch, 1.1 x 0.3, exit east
    LU, WU = 1.0, 0.35  # IC pads: 0.65 pitch, 1.0 x 0.35, exit south
    pj = ((3.0, 6.75), (3.0, 7.25))
    pu = ((17.675, 3.5), (18.325, 3.5))
    j1 = pad_pair("J1", [("USB_P", pj[0]), ("USB_N", pj[1])], (1, 0), LJ, WJ, [(3.0, 6.25), (3.0, 7.75)])
    u1 = pad_pair("U1", [("USB_P", pu[0]), ("USB_N", pu[1])], (0, 1), LU, WU, [(17.025, 3.5), (18.975, 3.5)])
    fps = [j1, u1]
    dj, G1, pj_pts, nj_pts = exit_fan(fps, pj[0], pj[1], (1, 0), LJ, 0.5)
    du, G2, pu_pts, nu_pts = exit_fan(fps, pu[0], pu[1], (0, 1), LU, 0.65)
    # G2 is the north-heading end of the body: exit_fan was given the heading (0,-1) of the body at the U1 end
    tail = 0.10  # straight between the last bend and the gateway: the outer track's mitre needs 0.175 tan 22.5 = 0.0725
    yv = G2[1] + tail
    x1 = G2[0] - (G1[1] - yv)
    C = [G1, (x1, G1[1]), (G2[0], yv), G2]
    bp, bn = body_pair(C, +1)  # P on the left of the travel direction (north at J1, west at U1)
    # exit_fan at U1 returns pad -> gateway for a body heading h; the body here heads north INTO the U1 pads, so the
    # fan must be built away from the body: gateway G2 is the body's end, pads lie further north
    n2 = lnorm((0, -1))
    fp2, fn2 = fan(G2, (0, -1), n2, +1, 0.65, LU / 2 + du)
    Pp = pj_pts + bp[1:] + fp2[1:]
    Np = nj_pts + bn[1:] + fn2[1:]
    ref = mk_sol("reference", {"USB_P": [("F.Cu", Pp, DP_W)], "USB_N": [("F.Cu", Np, DP_W)]})
    ref.coupled = [("F.Cu", C)]
    ref.fans = [("F.Cu", pj_pts), ("F.Cu", nj_pts), ("F.Cu", fp2), ("F.Cu", fn2)]
    # careless: each track routed alone, shortest octilinear, class track width 0.25, no gap/gateway notion
    cp = [pj[0], (pu[0][0] - (pj[0][1] - (pu[0][1] + LU / 2)), pj[0][1]), (pu[0][0], pu[0][1] + LU / 2), pu[0]]
    cn = [pj[1], (pu[1][0] - (pj[1][1] - (pu[1][1] + LU / 2)), pj[1][1]), (pu[1][0], pu[1][1] + LU / 2), pu[1]]
    car = mk_sol("careless", {"USB_P": [("F.Cu", cp, W_NORMAL)], "USB_N": [("F.Cu", cn, W_NORMAL)]})
    gmin, gmax = gap_profile(car)
    car.text = (f"each track alone, shortest octilinear, class width 0.25: edge gap {gmin:.2f}..{gmax:.2f} mm (rule 0.15, "
                f"constant), skew {track_len(car, 'USB_P') - track_len(car, 'USB_N'):+.2f} mm, no gateway, no symmetric "
                f"convergence")
    car.extra = {"gap_min": r4(gmin), "gap_max": r4(gmax)}
    return DPat(
        name="dp_gateway", title="USB pair from a 0.5 mm connector to a 0.65 mm IC pair: gateways, constant gap",
        kind="solvable",
        purpose="Pair ends the KiCad way: straight out of the pads, symmetric 45 degree convergence to the rule gap, "
                "constant gap in between, mirrored at the other end (user rule 2026-11-08).",
        answer="", why="", size=(W, H), fps=fps, ref=ref, alt=None, careless=car, expected={},
        gateways=[{"id": "G1", "pt": G1, "dir": (1, 0)}, {"id": "G2", "pt": G2, "dir": (0, -1)}],
        extra={"d0": [dj, du], "C": C, "tail": tail, "G": [G1, G2], "pads": {"J1": pj, "U1": pu}, "L": [LJ, LU]})


# ----------------------------------------------------------------------------------------------- swap site builders
ST_VIA = 0.2  # straight run between the end of the 45 degree legs and a via centre
D_FAN = (VIA_PITCH - PITCH) / 2  # 0.175: each track moves 0.175 sideways (45 degrees) between body and via


def s1u_swap(C_in, C2, C_out, pre, post):
    """The swap as a sideways U excursion (user 2026-11-10: not an in-line reversal, shaped like dp_uturn_parity).
    The coupled body leaves its line on F.Cu and runs NORTH to E1 = C_in[-1]; a via pair A sits beyond E1 (axis east-west,
    P on the west side); the pair TURNS BACK there (B.Cu leaves the vias heading SOUTH, the side it came from) which flips
    P/N; the B.Cu body C2 (E2 = E1 ... E3, heading south) runs back down, possibly sideways; a via pair B beyond E3 returns
    the pair to F.Cu (no turn-back), the body continues from E4 = C_out[0] (heading south) and turns east to U1 (P south).
    Returns (paths {net: [(layer, pts, width)]}, coupled, fans, info)."""
    E1, E2, E3, E4 = C_in[-1], C2[0], C2[-1], C_out[0]
    assert E1 == E2
    north, south = (0, -1), (0, 1)
    pre_P, pre_N, pre_f = pre
    post_P, post_N, post_f = post
    b1 = body_pair(C_in, +1)
    fA = fan(E1, north, lnorm(north), +1, VIA_PITCH, ST_VIA)
    f2s = fan(E2, north, lnorm(south), -1, VIA_PITCH, ST_VIA)
    b2 = body_pair(C2, -1)
    f2e = fan(E3, south, lnorm(south), -1, VIA_PITCH, ST_VIA)
    f3s = fan(E4, north, lnorm(south), -1, VIA_PITCH, ST_VIA)
    b3 = body_pair(C_out, -1)
    paths = {}
    for k, net in enumerate(NETS):
        p1 = (pre_P if k == 0 else pre_N) + b1[k][1:] + fA[k][1:]
        p2 = f2s[k][::-1] + b2[k][1:] + f2e[k][1:]
        p3 = f3s[k][::-1] + b3[k][1:] + (post_P if k == 0 else post_N)[1:]
        paths[net] = [("F.Cu", p1, DP_W), ("B.Cu", p2, DP_W), ("F.Cu", p3, DP_W)]
    fans = [("F.Cu", f) for f in (*fA, *f3s, *pre_f, *post_f)] + [("B.Cu", f) for f in (*f2s, *f2e)]
    coupled = [("F.Cu", C_in), ("B.Cu", C2), ("F.Cu", C_out)]
    vA, vB = D_FAN + ST_VIA, D_FAN + ST_VIA
    info = {"via_A_centre_line": rp((E1[0], E1[1] - vA)), "via_B_centre_line": rp((E3[0], E3[1] + vB)),
            "via_A": [rp((E1[0] - VIA_PITCH / 2, E1[1] - vA)), rp((E1[0] + VIA_PITCH / 2, E1[1] - vA))],
            "via_B": [rp((E3[0] - VIA_PITCH / 2, E3[1] + vB)), rp((E3[0] + VIA_PITCH / 2, E3[1] + vB))],
            "C_in": [rp(p) for p in C_in], "C2": [rp(p) for p in C2], "C_out": [rp(p) for p in C_out],
            "sideways_shift_s": r4(E3[0] - E1[0])}
    return paths, coupled, fans, info


def s2_hop(C_a, C_b, pre, post):
    """Two single tracks around one hop (rule 4 alternative, "two single tracks, uncoupled, reported"): the pair
    decouples at x0 = C_a[-1], singles of the class width 0.25; P (north) steps to 0.325 from the centre line, N dives
    at V1 (0.25 south of it), runs under P on B.Cu, comes up at V2 (0.25 north of it) while P steps across to the
    south; the body resumes at x1 = C_b[0] = x0 + 2.4 with P on the right. 2 vias."""
    (x0, y0), (x1, y1) = C_a[-1], C_b[0]
    assert abs(x1 - x0 - 2.4) < 1e-9 and abs(y1 - y0) < 1e-9
    pre_P, pre_N, pre_f = pre
    post_P, post_N, post_f = post
    b1 = body_pair(C_a, +1)
    b2 = body_pair(C_b, -1)
    ws = W_NORMAL
    xp = x0 + 1.0
    Pd, Pe = (x0 + 0.15, y0 - 0.325), (x1 - 0.15, y0 + 0.325)
    P_d1 = [(x0, y0 - HALF), Pd]  # the 45 degree leg is still pair width: the pitch is below 0.25 + 0.15 until here
    P_single = [Pd, (xp, y0 - 0.325), (xp + 0.65, y0 + 0.325), Pe]
    P_d2 = [Pe, (x1, y0 + HALF)]
    V1, V2 = (x0 + 0.5, y0 + 0.25), (x0 + 2.0, y0 - 0.25)
    Na, Nb = (x0 + 0.075, y0 + 0.25), (x1 - 0.075, y0 - 0.25)
    N_d1 = [(x0, y0 + HALF), Na]
    N_f1 = [Na, V1]
    N_b = [V1, (x0 + 1.0, y0 - 0.25), V2]
    N_f2 = [V2, Nb]
    N_d2 = [Nb, (x1, y0 - HALF)]
    paths = {
        "USB_P": [("F.Cu", pre_P + b1[0][1:], DP_W), ("F.Cu", P_d1, DP_W), ("F.Cu", P_single, ws),
                  ("F.Cu", P_d2, DP_W), ("F.Cu", b2[0] + post_P[1:], DP_W)],
        "USB_N": [("F.Cu", pre_N + b1[1][1:], DP_W), ("F.Cu", N_d1, DP_W), ("F.Cu", N_f1, ws), ("B.Cu", N_b, ws),
                  ("F.Cu", N_f2, ws), ("F.Cu", N_d2, DP_W), ("F.Cu", b2[1] + post_N[1:], DP_W)]}
    fans = [("F.Cu", f) for f in (*pre_f, *post_f)]
    coupled = [("F.Cu", C_a), ("F.Cu", C_b)]
    info = {"x0": x0, "x1": x1, "V1": rp(V1), "V2": rp(V2), "zone_length": 2.4}
    return paths, coupled, fans, info


def sol_from(label, paths, coupled, fans):
    s = mk_sol(label, paths)
    s.coupled, s.fans = coupled, fans
    # mk_sol drops vias at track junctions only when consecutive layers differ: P in S2 stays on F.Cu (no via)
    s.vias = []
    for net, segs in paths.items():
        for (l1, _, _), (l2, p2, _) in zip(segs, segs[1:]):
            if l1 != l2:
                s.vias.append(Via(net, (r4(p2[0][0]), r4(p2[0][1])), VIA_D, VIA_DRILL))
    return s


# ----------------------------------------------------------------------------------------------- patterns 2 and 4
LU_W = 1.0  # IC pad length (exit axis) of the swap patterns' U1
WU_W = 0.35


def _swap_fps(y_j, y_u, *, walls_u=True):
    """J1: connector, pads exit east, P north / N south, pitch 0.5. U1: IC, pads exit west, pitch 0.65, N north / P south
    (the order is MIRRORED against the source as seen along the route)."""
    j1 = pad_pair("J1", [("USB_P", (3.0, y_j - 0.25)), ("USB_N", (3.0, y_j + 0.25))], (1, 0), 1.1, 0.3,
                  [(3.0, y_j - 0.75), (3.0, y_j + 0.75)])
    u1 = pad_pair("U1", [("USB_P", (22.5, y_u + 0.325)), ("USB_N", (22.5, y_u - 0.325))], (-1, 0), LU_W, WU_W,
                  [(22.5, y_u - 0.975), (22.5, y_u + 0.975)])
    return j1, u1


def _ends(j1, u1):
    """Pad fans of both ends: pre = (P, N, fans) pad->G1 for the eastward body; post = (P, N, fans) G2->pad (P south)."""
    fps = [j1, u1]
    pj = [p.pos for p in j1.pads[:2]]
    pu = [p.pos for p in u1.pads[:2]]
    dj, G1, pjp, njp = exit_fan(fps, pj[0], pj[1], (1, 0), 1.1, 0.5)
    du, G2, _, _ = exit_fan(fps, pu[0], pu[1], (-1, 0), LU_W, 0.65)
    east = (1, 0)
    fp2, fn2 = fan(G2, east, lnorm(east), -1, 0.65, LU_W / 2 + du)  # P on the right (south) at the arrival
    return G1, G2, (pjp, njp, [pjp, njp]), (fp2, fn2, [fp2, fn2]), (dj, du)


S_SHIFT = 1.1  # sideways shift of the returning B.Cu leg (see swap_footprint_min)
READING = ("Reading of the user's feedback (2026-11-10), please correct: the swap is NOT an in-line reversal; the pair "
           "goes UP out of its line on F.Cu, turns back at a via pair at the top (the turn-back flips P/N, as in "
           "dp_uturn_parity), comes DOWN on B.Cu, and a second via pair at the bottom returns it to F.Cu on its line. "
           "Footprint is vertical.")


def p_swap_needed() -> DPat:
    W, H = 26.0, 14.0
    y0 = 8.0
    j1, u1 = _swap_fps(y0, y0)
    G1, G2, pre, post, d0 = _ends(j1, u1)
    xu, s = 9.0, S_SHIFT
    xd = xu + s
    E4y = y0 - 0.5
    E3y = E4y - 0.75
    E2y = E3y - (0.3 + s + 0.1)
    C_in = [G1, (xu - 0.35, y0), (xu, y0 - 0.35), (xu, E2y)]
    C2 = [(xu, E2y), (xu, E2y + 0.3), (xd, E2y + 0.3 + s), (xd, E3y)]
    C_out = [(xd, E4y), (xd, E4y + 0.25), (xd + 0.25, y0), G2]
    paths, coupled, fans, info = s1u_swap(C_in, C2, C_out, pre, post)
    ref = sol_from("reference: U excursion north, turn-back at via pair A, back down on B.Cu, via pair B, 4 vias",
                   paths, coupled, fans)
    x0 = 9.0
    Ca = [G1, (x0, y0)]
    Cb = [(x0 + 2.4, y0), G2]
    paths2, coupled2, fans2, info2 = s2_hop(Ca, Cb, pre, post)
    alt = sol_from("alternative: two single tracks, one hop, 2 vias", paths2, coupled2, fans2)
    return DPat(
        name="dp_swap_needed", title="P/N arrive mirrored: polarity swap as a sideways U excursion at a via pair",
        kind="solvable", purpose="", answer="", why="", size=(W, H), fps=[j1, u1], ref=ref, alt=alt, careless=None,
        expected={}, gateways=[{"id": "G1", "pt": G1, "dir": (1, 0)}, {"id": "G2", "pt": G2, "dir": (1, 0)}],
        extra={"s1": info, "s2": info2, "d0": d0, "s": s})


def p_swap_no_room() -> DPat:
    W, H = 26.0, 14.0
    y0 = 8.0
    j1, u1 = _swap_fps(y0, y0)
    CH = 1.4  # corridor height between the component bodies
    hn, hs = y0 - CH / 2, y0 + CH / 2  # 7.3, 8.7
    top, bot = 4.6, 12.0
    # C1 | door 1 (8.4 .. 10.2) | C2 | door 2 (14.8 .. 16.6) | C3 north of the corridor, C4 south; through-hole bodies (both layers)
    blocks = [block("C1", 5.0, top, 8.4, hn), block("C2", 10.2, top, 14.8, hn), block("C3", 16.6, top, 20.0, hn),
              block("C4", 5.0, hs, 20.0, bot)]
    fps = [j1, u1] + blocks
    G1, G2, pre, post, d0 = _ends(j1, u1)
    # reference: two single tracks around one hop inside the corridor (x 11.0 .. 13.4 lies under C2)
    x0 = 11.0
    Ca = [G1, (x0, y0)]
    Cb = [(x0 + 2.4, y0), G2]
    paths, coupled, fans, info = s2_hop(Ca, Cb, pre, post)
    ref = sol_from("reference: two single tracks, one hop, 2 vias", paths, coupled, fans)
    # alternative: the U swap elsewhere: UP through door 1 (F.Cu), via pair A and the turn-back in the open strip,
    # B.Cu east along the strip and DOWN through door 2, via pair B at the foot of door 2, F.Cu east to U1
    d1, d2 = 9.3, 15.7
    E1y = 3.275
    E3y = 6.3
    C_in = [G1, (d1 - 0.35, y0), (d1, y0 - 0.35), (d1, E1y)]
    C2 = [(d1, E1y), (d1, 3.5), (d1 + 0.35, 3.85), (d2 - 0.35, 3.85), (d2, 4.2), (d2, E3y)]
    C_out = [(d2, E3y + 0.75), (d2, 7.65), (d2 + 0.35, y0), G2]
    paths2, coupled2, fans2, info2 = s1u_swap(C_in, C2, C_out, pre, post)
    alt = sol_from("alternative: the swap elsewhere (4 vias): up door 1, turn back in the strip, down door 2", paths2,
                   coupled2, fans2)
    walls = [{"rect": (b.pads[0].pos[0] - b.pads[0].size[0] / 2, b.pads[0].pos[1] - b.pads[0].size[1] / 2,
                       b.pads[0].pos[0] + b.pads[0].size[0] / 2, b.pads[0].pos[1] + b.pads[0].size[1] / 2),
              "label": b.ref} for b in blocks]
    fp = swap_footprint_min(S_SHIFT)
    # the swap footprint, drawn over the corridor in the problem panel: one U excursion needs this free area
    xu = 11.0 + 0.0
    box_rect = (xu - fp["width"] / 2, y0 - fp["north"], xu + fp["width"] / 2, y0 + 0.25)
    return DPat(
        name="dp_swap_no_room", title="P/N arrive mirrored, the corridor and its doors have no room for the swap's U excursion",
        kind="solvable", purpose="", answer="", why="", size=(W, H), fps=fps, ref=ref, alt=alt, careless=None,
        expected={}, gateways=[{"id": "G1", "pt": G1, "dir": (1, 0)}, {"id": "G2", "pt": G2, "dir": (1, 0)}],
        walls=walls, extra={"s2": info, "s1_elsewhere": info2, "corridor": {"height": CH, "y": [hn, hs],
                                                                            "doors": [[8.4, 10.2], [14.8, 16.6]]},
                            "d0": d0, "need_box": box_rect})


# ----------------------------------------------------------------------------------------------- pattern 3: U-turn parity
def p_uturn_parity() -> DPat:
    W, H = 16.0, 14.0
    j1 = pad_pair("J1", [("USB_P", (3.0, 3.75)), ("USB_N", (3.0, 4.25))], (1, 0), 1.1, 0.3, [(3.0, 3.25), (3.0, 4.75)])
    j2 = pad_pair("J2", [("USB_P", (3.0, 9.75)), ("USB_N", (3.0, 10.25))], (1, 0), 1.1, 0.3, [(3.0, 9.25), (3.0, 10.75)],
                  layer="B.Cu")
    fps = [j1, j2]
    dj, G1, pjp, njp = exit_fan(fps, (3.0, 3.75), (3.0, 4.25), (1, 0), 1.1, 0.5)
    dk, G2, _, _ = exit_fan(fps, (3.0, 9.75), (3.0, 10.25), (1, 0), 1.1, 0.5)
    xa = 6.0
    E1 = (xa - D_FAN - ST_VIA, 4.0)
    C_in = [G1, E1]
    E2 = (xa - D_FAN - ST_VIA, 4.0)
    west = (-1, 0)
    C2 = [E2, (E2[0] - 0.3, 4.0), (E2[0] - 0.8, 4.5), (E2[0] - 0.8, 9.5), (E2[0] - 1.3, 10.0), G2]
    b1 = body_pair(C_in, +1)
    fA = fan(E1, (1, 0), lnorm((1, 0)), +1, VIA_PITCH, ST_VIA)
    f2s = fan(E2, (1, 0), lnorm(west), -1, VIA_PITCH, ST_VIA)
    b2 = body_pair(C2, -1)
    fe = fan(G2, west, lnorm(west), -1, 0.5, 1.1 / 2 + dk)  # P on the right of the westward heading = north
    paths = {}
    for k, net in enumerate(NETS):
        p1 = (pjp, njp)[k] + b1[k][1:] + fA[k][1:]
        p2 = f2s[k][::-1] + b2[k][1:] + fe[k][1:]
        paths[net] = [("F.Cu", p1, DP_W), ("B.Cu", p2, DP_W)]
    ref = sol_from("reference: one via pair, turn-back = the swap", paths,
                   [("F.Cu", C_in), ("B.Cu", C2)], [("F.Cu", pjp), ("F.Cu", njp), ("F.Cu", fA[0]), ("F.Cu", fA[1]),
                                                    ("B.Cu", f2s[0]), ("B.Cu", f2s[1]), ("B.Cu", fe[0]), ("B.Cu", fe[1])])
    # careless: the smooth U on F.Cu (P stays on the left = south on the westward leg), a plain layer change at x = 5.0
    # with NO turn-back, P and N arrive on the wrong pads and cross in the last fan
    xv = 5.2
    Es = (xv + D_FAN + ST_VIA, 10.0)  # body end before the via fan, heading west
    Cs = [G1, (8.0, 4.0), (8.5, 4.5), (8.5, 9.5), (8.0, 10.0), Es]
    bs = body_pair(Cs, +1)
    fv = fan(Es, west, lnorm(west), +1, VIA_PITCH, ST_VIA)
    Eb = (xv - D_FAN - ST_VIA, 10.0)
    fb = fan(Eb, (1, 0), lnorm(west), +1, VIA_PITCH, ST_VIA)
    Cb = [Eb, G2]
    bb = body_pair(Cb, +1)
    # P is on the south side (y 10.175) at G2 and has to reach the NORTH pad, N the reverse: straight fan lines cross
    cp = [bb[0][-1], (3.0, 9.75)]
    cn = [bb[1][-1], (3.0, 10.25)]
    car = sol_from("careless: parity ignored", {
        "USB_P": [("F.Cu", pjp + bs[0][1:] + fv[0][1:], DP_W), ("B.Cu", fb[0][::-1] + bb[0][1:] + cp[1:], DP_W)],
        "USB_N": [("F.Cu", njp + bs[1][1:] + fv[1][1:], DP_W), ("B.Cu", fb[1][::-1] + bb[1][1:] + cn[1:], DP_W)]},
        [("F.Cu", Cs), ("B.Cu", Cb)], [])
    X = LineString(cp).intersection(LineString(cn))
    car.marks = [{"pt": (X.x, X.y), "label": "X1: P and N cross on B.Cu"}]
    return DPat(
        name="dp_uturn_parity", title="U-turn at the layer change: the turn-back is the polarity swap", kind="solvable",
        purpose="", answer="", why="", size=(W, H), fps=fps, ref=ref, alt=None, careless=car, expected={},
        connections=[("USB_P", "J1.1", "J2.1"), ("USB_N", "J1.2", "J2.2")],
        gateways=[{"id": "G1", "pt": G1, "dir": (1, 0)}, {"id": "G2", "pt": G2, "dir": (-1, 0)}],
        extra={"xa": xa, "C2": [rp(p) for p in C2], "d0": [dj, dk], "G": [G1, G2]})


# ----------------------------------------------------------------------------------------------- expected values
def _close(a, b, tol=0.005):
    return abs(a - b) <= tol


def mean_len(sol: Sol) -> float:
    return (track_len(sol, "USB_P") + track_len(sol, "USB_N")) / 2


def hand_gateway(pat: DPat) -> dict:
    """Hand derivation of dp_gateway (every number from the rule values, not from the geometry)."""
    leg1 = (0.5 - PITCH) / 2  # J1: pad pitch 0.5 -> 0.35: each track moves 0.075
    leg2 = (0.65 - PITCH) / 2  # U1: pad pitch 0.65 -> 0.35: 0.15
    # J1: P leg end (3.55 + d0 + leg1, 6.75 + leg1) against the N pad corner (3.55, 7.25 - 0.15): needs 0.1 + 0.2
    need = DP_W / 2 + CLEAR
    d0_j = math.sqrt(need ** 2 - (0.5 - 0.15 - leg1) ** 2) - leg1  # 0.0448 -> next 0.05 step
    G1 = (3.0 + 1.1 / 2 + 0.05 + leg1, 7.0)
    # U1: the leg (0.15 sideways) ends 0.325 left of the N pad corner: sqrt(0.325^2 + 0.15^2) = 0.358 >= 0.30: d0 = 0
    G2 = (18.0, 3.5 + 1.0 / 2 + 0.0 + leg2)
    tail = 0.10
    dyd = G1[1] - (G2[1] + tail)
    x1 = G2[0] - dyd
    centre = (x1 - G1[0]) + dyd * SQ2 + tail
    free = (G2[0] - G1[0]) + (SQ2 - 1) * (G1[1] - G2[1])  # octilinear length with no tail
    skew = 2 * 2 * PITCH * math.tan(math.radians(22.5))  # two 45 degree bends, 2 p tan(theta/2) each (p = centre pitch)
    return {"leg_J1": leg1, "leg_U1": leg2, "d0_J1_min": d0_j, "G1": G1, "G2": G2, "tail": tail, "x_bend": x1,
            "centre_line": centre, "octilinear_no_tail": free, "tail_price": centre - free,
            "skew_P_minus_N": -skew,
            "mean_track": 0.55 + 0.05 + leg1 * SQ2 + centre + (leg2 * SQ2 + 0.5)}


def _via_fan() -> float:
    """Length of a track along one via fan: the 45 degree leg (0.175 sideways) plus the straight run to the via."""
    return D_FAN * SQ2 + ST_VIA


def swap_footprint_min(s: float = S_SHIFT) -> dict:
    """Free area one U excursion needs (user 2026-11-10: room for BOTH via pairs and the pair's turn-back), measured from
    the F.Cu line y0 the pair arrives on, upwards (north):
    - E4 (start of the F.Cu body after via pair B) lies 0.5 above the line (0.25 straight + the 0.25 diagonal that turns it
      east); the via fan before it is 0.75 (two 0.375 fans around the via pair B centre), so E3 = y0 - 1.25;
    - the B.Cu body runs 0.3 + s + 0.1 down to E3 (stub, sideways shift s on a 45 degree diagonal, tail);
    - via pair A sits 0.375 above E2: its centre is 2.025 + s above the line, its copper edge + clearance
      VIA_R + CLEAR = 0.425 further (north = 2.45 + s);
    - width: the up leg carries via pair A (centre pitch 0.70), the down leg via pair B, s apart: s + 2 (0.35 + 0.225 + 0.2).
    - the shift s: the P via of pair B (x_d - 0.35) must clear the N track of the up leg (x_u + 0.175) by via radius +
      half width + clearance = 0.525: s >= 0.35 + 0.175 + 0.525 = 1.05 (built: 1.1)."""
    s_min = VIA_PITCH / 2 + HALF + VIA_R + DP_W / 2 + CLEAR
    to_via_centre = 0.5 + 0.75 + 0.3 + s + 0.1 + 0.375
    north = to_via_centre + VIA_R + CLEAR
    width = s + 2 * (VIA_PITCH / 2 + VIA_R + CLEAR)
    return {"s_min": s_min, "s": s, "via_A_above_line": to_via_centre, "north": north, "width": width,
            "height": north, "via_pair_width": VIA_PITCH + 2 * VIA_R}


def hand_swap_needed(pat: DPat) -> dict:
    s = S_SHIFT
    xu = 9.0
    G1x, G2x = 3.0 + 0.55 + 0.05 + 0.075, 22.0 - 0.15
    pad_j, pad_u = 0.6 + 0.075 * SQ2, 0.15 * SQ2 + 0.5
    up = (8.0 - 0.35) - (8.0 - 1.65 - s)  # from the end of the first bend to E1
    c2 = 0.3 + s * SQ2 + 0.1
    c_out = 0.25 + 0.25 * SQ2 + (G2x - (xu + s + 0.25))
    mean_s1 = pad_j + (xu - 0.35 - G1x) + 0.35 * SQ2 + up + 4 * _via_fan() + c2 + c_out + pad_u
    zone_p = 0.15 * SQ2 + 0.85 + 0.65 * SQ2 + 0.6 + 0.15 * SQ2
    zone_n = 0.075 * SQ2 + 0.425 + 0.5 * SQ2 + 1.0 + 0.325 + 0.075 * SQ2
    x0, x1 = 9.0, 11.4
    s2_p = pad_j + (x0 - G1x) + zone_p + (G2x - x1) + pad_u
    s2_n = pad_j + (x0 - G1x) + zone_n + (G2x - x1) + pad_u
    s3 = sum(19.5 + (SQ2 - 1) * dy for dy in (0.575, 0.575))  # octilinear lower bound, pad centre to pad centre
    return {"mean_track_S1": mean_s1, "S2_P": s2_p, "S2_N": s2_n, "S3_length_lower_bound": s3,
            "footprint": swap_footprint_min()}


def hand_no_room(pat: DPat) -> dict:
    zone_p = 0.15 * SQ2 + 0.85 + 0.65 * SQ2 + 0.6 + 0.15 * SQ2
    zone_n = 0.075 * SQ2 + 0.425 + 0.5 * SQ2 + 1.0 + 0.325 + 0.075 * SQ2
    pad_j, pad_u = 0.6 + 0.075 * SQ2, 0.15 * SQ2 + 0.5
    x0, x1, G1x, G2x = 11.0, 13.4, 3.675, 21.85
    c_in = (8.95 - G1x) + 0.35 * SQ2 + (7.65 - 3.275)
    c2 = (3.5 - 3.275) + 0.35 * SQ2 + (15.35 - 9.65) + 0.35 * SQ2 + (6.3 - 4.2)
    c_out = (7.65 - 7.05) + 0.35 * SQ2 + (G2x - 16.05)
    alt = pad_j + c_in + 4 * _via_fan() + c2 + c_out + pad_u
    fp = swap_footprint_min()
    return {"P": pad_j + (x0 - G1x) + zone_p + (G2x - x1) + pad_u, "N": pad_j + (x0 - G1x) + zone_n + (G2x - x1) + pad_u,
            "zone_P": zone_p, "zone_N": zone_n, "S1_elsewhere_mean": alt, "footprint": fp,
            "S2_footprint_height": 0.325 + W_NORMAL / 2 + 0.25 + VIA_R + 2 * CLEAR, "door_width": 1.8}


def hand_uturn(pat: DPat) -> dict:
    c2 = 0.3 + 0.5 * SQ2 + 5.0 + 0.5 * SQ2 + 0.65
    pad = 0.6 + 0.075 * SQ2
    return {"mean_track": pad + (6.0 - D_FAN - ST_VIA - 3.675) + 2 * _via_fan() + c2 + pad, "c2": c2}


def vias_ok(sol: Sol) -> dict:
    ds = {(v.dia, v.drill) for v in sol.vias}
    return {"sizes": sorted(ds), "pi_drill": r4(math.pi * VIA_DRILL)}


def check(pat: DPat) -> dict:
    """The hand-derived numbers agree with the geometry; the reference follows the rules it claims to follow."""
    ref = pat.ref
    # rule values: class normal values differ from the diff-pair values on purpose
    assert (W_NORMAL, CLEAR, VIA_CLASS) != (DP_W, DP_GAP, (VIA_D, VIA_DRILL))
    assert W_NORMAL != DP_W and VIA_CLASS[0] != VIA_D and CLASS_USB["diff_pair_width"] == DP_W
    # vias: smallest supplier drill with pi * drill >= trace width; diameter = drill + 2 annular (>= min via dia)
    for v in ref.vias + (pat.alt.vias if pat.alt else []):
        assert (v.dia, v.drill) == (VIA_D, VIA_DRILL), v
    assert MIN_DRILL == VIA_DRILL and math.pi * VIA_DRILL >= max(DP_W, W_NORMAL) and VIA_D >= MIN_VIA_D
    assert _close(VIA_D, VIA_DRILL + 2 * MIN_ANNULAR, 1e-9) and _close(VIA_PITCH, 0.70, 1e-9)
    # coupled stretches use the pair values: pair width on every coupled body, constant gap
    bodies = ref.coupled
    for t in ref.tracks:
        for a, b in zip(t.pts, t.pts[1:]):
            m = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
            in_body = any(ly == t.layer and Point(m).distance(LineString(c)) <= HALF * 1.001 + 1e-6 for ly, c in bodies)
            if in_body:
                assert t.width == DP_W, (pat.name, t.net, t.width)
    gaps = []
    for t in ref.tracks:
        if t.net != "USB_P":
            continue
        for ly, c in bodies:
            if ly != t.layer:
                continue
            for a, b in zip(t.pts, t.pts[1:]):
                m = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
                if Point(m).distance(LineString(c)) <= HALF * 1.001 + 1e-6:
                    nn = unary_union([LineString(u.pts).buffer(u.width / 2) for u in ref.tracks
                                      if u.net == "USB_N" and u.layer == ly])
                    gaps.append(Point(m).distance(nn) - DP_W / 2)
    if gaps:
        assert all(_close(g, DP_GAP, 1e-3) for g in gaps), (pat.name, min(gaps), max(gaps))
    # connectivity, clearances, edge
    for sol in [ref] + ([pat.alt] if pat.alt else []):
        assert connected(pat, sol, "USB_P") and connected(pat, sol, "USB_N"), (pat.name, sol.label)
        rep = clearance_report(pat, sol)
        assert rep["margin"] >= -1e-6 and rep["edge_margin"] >= -1e-6, (pat.name, sol.label, rep)
    return {"gap_samples": len(gaps), "clearance": clearance_report(pat, ref)}


# ----------------------------------------------------------------------------------------------- finishing: answers
RULES_TEXT = ("USB class: normal values track 0.25 / clearance 0.20 / via 0.60-0.30 (never used on a coupled stretch); "
              "pair values diff_pair_width 0.20, diff_pair_gap 0.15, diff_pair_via_gap 0.25. Board Setup minimums "
              "(supplier): via 0.45, drill 0.25, annular ring 0.10, clearance 0.15 (= the pair gap), track 0.20.")


def _rules_block() -> dict:
    return {
        "class_USB": {"normal": {"track_width": W_NORMAL, "clearance": CLEAR, "via": list(VIA_CLASS)},
                      "diff_pair": {"width": DP_W, "gap": DP_GAP, "via_gap": DP_VIA_GAP}},
        "board_setup_minimums": {"track": DP_W, "clearance": DP_GAP, "via_diameter": MIN_VIA_D,
                                 "through_hole_drill": MIN_DRILL, "annular_ring": MIN_ANNULAR, "hole_to_hole": 0.25,
                                 "copper_to_hole": HOLE_CLEAR, "copper_to_edge": EDGE},
        "which_value_applies_where": {
            "coupled body (gateway to gateway / via fan to via fan)": f"diff_pair_width {DP_W}, gap {DP_GAP} "
                                                                       f"(centre pitch {PITCH})",
            "pad fan (straight run-out + two symmetric 45 degree legs)": f"diff_pair_width {DP_W}; the spacing runs "
                                                                         "from the pad pitch to the pair pitch",
            "via pair (swap, return)": f"vias {VIA_D} / {VIA_DRILL}, centre pitch = {VIA_D} + diff_pair_via_gap "
                                       f"{DP_VIA_GAP} = {VIA_PITCH}",
            "two single tracks (rule 4 alternative)": f"class track_width {W_NORMAL} between their own decoupling "
                                                      f"points; P against N still keeps {DP_GAP} (KiCad judges any P/N "
                                                      "track pair by the pair gap)",
            "neighbours (other pads, walls, board edge)": f"class clearance {CLEAR}, copper to edge {EDGE}"},
        "via": {"diameter": VIA_D, "drill": VIA_DRILL, "pair_centre_pitch": VIA_PITCH,
                "rule": (f"smallest drill the supplier allows (Board Setup through-hole minimum {MIN_DRILL}) with "
                         f"pi * drill = {math.pi * VIA_DRILL:.3f} >= the trace width it carries ({DP_W} pair, "
                         f"{W_NORMAL} single); diameter = drill + 2 * min annular {MIN_ANNULAR} = {VIA_D} "
                         f"(min via diameter {MIN_VIA_D} does not raise it). The class via {VIA_CLASS[0]}/{VIA_CLASS[1]} "
                         "is NOT used: a router that takes the class size is visibly not optimal."),
                "effective_via_gap": f"max(diff_pair_via_gap {DP_VIA_GAP}, hole-to-hole 0.25 - 2 annular = "
                                     f"{0.25 - 2 * MIN_ANNULAR:.2f}, copper-to-hole {HOLE_CLEAR} - annular = "
                                     f"{HOLE_CLEAR - MIN_ANNULAR:.2f}) = {DP_VIA_GAP} (docs/dev/kicad-diff-pairs.md 3.4)"}}


def _common_lit() -> list[str]:
    return [LIT["manual63"], LIT["help50"], LIT["kicad"]]


def finish_gateway(pat: DPat) -> None:
    h = hand_gateway(pat)
    ref, car = pat.ref, pat.careless
    c = cost(ref)
    assert _close(h["G1"][0], pat.extra["G"][0][0], 1e-6) and _close(h["G2"][1], pat.extra["G"][1][1], 1e-6), h
    assert _close(h["d0_J1_min"], 0.0448, 5e-4) and pat.extra["d0"] == [0.05, 0.0]
    assert _close(mean_len(ref), h["mean_track"], 0.005), (mean_len(ref), h["mean_track"])
    assert _close(c["skew_P_minus_N"], h["skew_P_minus_N"], 0.002)
    pat.purpose = ("Pair ends the KiCad way: two tracks start at the P and N pads, run straight in the pads' exit "
                   "direction until the convergence is DRC clean, then two symmetric 45 degree segments reach the rule "
                   "gap (the gateway); the gap is constant in between; mirrored at the other end (user 2026-11-08).")
    pat.answer = (f"One pair, F.Cu only, 0 vias, P to P, P stays on the left of the travel direction at both ends (0 "
                  f"swaps). J1 (0.5 mm pitch pads, exit east): tracks run straight 0.60 from the pad centres (0.05 past "
                  f"the pad ends: the earlier 45 degree leg would pass 0.185 from the neighbouring pad, 0.20 needed), "
                  f"then two symmetric 45 degree legs (0.075 sideways each) reach the gap 0.15: gateway G1 = "
                  f"({h['G1'][0]:.3f}, 7.000) heading east. U1 (0.65 mm pitch pads, exit south): straight to the pad "
                  f"ends, legs 0.15 sideways, gateway G2 = (18.000, {h['G2'][1]:.3f}) heading north. Between the gateways "
                  f"the pair runs east, one 45 degree stretch and north with a 0.10 straight before G2, edge gap "
                  f"constant 0.15, centre line {h['centre_line']:.3f} mm; skew {h['skew_P_minus_N']:+.2f} mm (two 45 degree "
                  f"bends, not tuned).")
    pat.why = ("The gateway positions follow from the pad geometry and the clearance: J1's convergence must start "
               "0.045 mm beyond the pad end (sqrt(0.30^2 - 0.275^2) - 0.075), rounded up on the 0.05 grid; U1's legs "
               "are clear at once (0.358 >= 0.30). The body is the shortest octilinear path between the gateways that "
               "arrives heading north: east, one 45 degree stretch, north; the 0.10 tail is the straight the outer "
               "track's mitre needs (0.175 tan 22.5 = 0.073) and costs 0.10 (2 - sqrt2) = 0.059 mm over the free "
               "octilinear length " + f"{h['octilinear_no_tail']:.3f}.")
    pat.expected = {
        "interpretation": {"needs_user_confirmation": False,
                           "built": "KiCad-style gateways at both ends exactly as the user's rule 2 describes"},
        "rules": _rules_block(),
        "gateways": [{"id": "G1", "pt": rp(h["G1"]), "heading": "east", "run_out_beyond_pad_end": 0.05,
                      "leg_sideways_each": h["leg_J1"], "pad_pitch": 0.5, "pair_pitch": PITCH,
                      "run_out_argument": "sqrt((d0 + 0.075)^2 + 0.275^2) >= 0.30 (P leg end against the N pad corner, "
                                          "track half 0.10 + clearance 0.20) -> d0 >= 0.0448, next 0.05 step"},
                     {"id": "G2", "pt": rp(h["G2"]), "heading": "north (arrival)", "run_out_beyond_pad_end": 0.0,
                      "leg_sideways_each": h["leg_U1"], "pad_pitch": 0.65, "pair_pitch": PITCH,
                      "run_out_argument": "legs move AWAY from the neighbour pads' corner by less than they approach: "
                                          "sqrt(0.325^2 + 0.15^2) = 0.358 >= 0.30 at once"}],
        "polarity": {"P_left_at_source": True, "P_left_at_destination_arrival": True, "turn_backs": 0,
                     "swaps_needed": 0, "argument": "J1 heads east with P (north pad) on the left; U1 is entered "
                                                    "heading north with P' (west pad) on the left: same handedness"},
        "vias": 0, "crossings": 0,
        "lengths": {"P": c["length_P"], "N": c["length_N"], "skew_P_minus_N": c["skew_P_minus_N"],
                    "hand_skew": r4(h["skew_P_minus_N"]), "hand_mean_track": r4(h["mean_track"]),
                    "centre_line_between_gateways": r4(h["centre_line"]),
                    "octilinear_lower_bound_between_gateways": r4(h["octilinear_no_tail"]),
                    "tail_price": r4(h["tail_price"])},
        "cost": c, "careless": {"description": car.text, **car.extra, "skew": cost(car)["skew_P_minus_N"],
                                "uncoupled_sum": cost(car)["uncoupled_sum"], "cost": cost(car)["cost"]},
        "not_tuned": "skew from the two 45 degree bends (0.58 mm) is not compensated while routing, as in KiCad "
                     "(docs/dev/kicad-diff-pairs.md 3.3)",
        "judgement_calls": ["Whether the tail before a gateway may be 0.10 (outer mitre only) or a longer straight "
                            "(KiCad has no minimum, TopoR zips only straights >= 3 x the pair width, manual p. 171).",
                            "Run-out grid 0.05 mm."],
        "literature": _common_lit(),
    }


def _excursion_box(sol: Sol, y0: float):
    """Bounding box (with clearances) of the excursion: the B.Cu copper and the four vias (they set its extent)."""
    items = [LineString(t.pts).buffer(t.width / 2 + CLEAR) for t in sol.tracks if t.layer == "B.Cu"]
    items += [Point(v.pos).buffer(VIA_R + CLEAR) for v in sol.vias]
    return unary_union(items).bounds


def finish_swap_needed(pat: DPat) -> None:
    h = hand_swap_needed(pat)
    ref, alt = pat.ref, pat.alt
    cr, ca = cost(ref), cost(alt)
    assert _close(mean_len(ref), h["mean_track_S1"], 0.005), (mean_len(ref), h["mean_track_S1"])
    assert _close(track_len(alt, "USB_P"), h["S2_P"], 0.005) and _close(track_len(alt, "USB_N"), h["S2_N"], 0.005)
    fp = h["footprint"]
    s3_cost = r4(h["S3_length_lower_bound"] * (1 + UNCOUPLED_PRICE) + 2 * VIA_PRICE)
    u_star = (cr["length_sum"] + VIA_PRICE * 4 - ca["length_sum"] - VIA_PRICE * 2) / (
        ca["uncoupled_sum"] - cr["uncoupled_sum"])
    bx0, by0, bx1, by1 = _excursion_box(ref, 8.0)
    assert _close(bx1 - bx0, fp["width"], 0.01) and _close(8.0 - by0, fp["north"], 0.01), (bx0, by0, bx1, by1, fp)
    ref.boxes = [{"rect": (bx0, by0, bx1, by1), "label": f"swap site (measured) {bx1 - bx0:.2f} wide x {8.0 - by0:.2f} above the line"}]
    extra = cr["length_sum"] - h["S3_length_lower_bound"]
    pat.purpose = ("P and N arrive swapped (P left at the source, P right at the destination, as seen along the route), "
                   "both layers free. The swap is a sideways U excursion shaped like dp_uturn_parity, not an in-line "
                   "reversal: the pair leaves its line going north on F.Cu, turns back at via pair A (flipping P/N), comes "
                   "down on B.Cu, and via pair B returns it to F.Cu on its line (user 2026-11-10; "
                   "scratch/d7b/user-polarity-swap-example.png).")
    pat.answer = (f"Swap by a U excursion north of the line: F.Cu out of J1 (G1), up at x=9.0 to via pair A (centre "
                  f"{2.025 + S_SHIFT:.3f} above the line), turn-back there, B.Cu back down shifted {S_SHIFT} east, via pair B, F.Cu "
                  f"east to U1 (G2); line and destination on the same y. 4 vias {VIA_D}/{VIA_DRILL} (two pairs, centre pitch "
                  f"{VIA_PITCH}), 0 crossings, coupled everywhere, per-track {cr['length_P']:.2f} / {cr['length_N']:.2f} mm, "
                  f"price {cr['cost']:.1f} = {cr['length_sum']:.2f} mm + 4 x 25 (U = {UNCOUPLED_PRICE}). Footprint: "
                  f"{fp['width']:.2f} wide x {fp['north']:.2f} high above the line (vertical, not along the line); excursion "
                  f"costs {extra:.2f} mm of copper over the straight line.")
    pat.why = (f"Polarity: J1 heads east with P on the left; U1 is entered heading east with P' on the RIGHT: one flip. A "
               f"turn-back at a via pair flips (the tracks leave the vias on the side they arrived from): heading north P "
               f"(west) is on the left; leaving south it is on the right; after the final left turn east P is south. Via "
               f"pair B only returns the pair to F.Cu (no turn-back): 2 via pairs = 4 vias (user 2026-11-10). Why not one "
               f"pair: U1's pads are F.Cu SMD, so the pair must be on F.Cu at the end; with the destination on B.Cu one pair "
               f"suffices (dp_uturn_parity). Cheapest legal shape: via pair B sits at the foot of the down leg so the "
               f"F.Cu body rejoins its line right there; the shift s must be >= {fp['s_min']:.2f} (P via B against the N "
               f"track of the up leg), built {S_SHIFT}. Prices: single-track hop {ca['cost']:.1f} (2 vias, "
               f"{ca['uncoupled_sum']:.1f} mm decoupled), two fully uncoupled singles >= {s3_cost:.1f}; the swap "
               f"{cr['cost']:.1f}. Priced alone the hop would win unless an uncoupled mm costs more than U* = {u_star:.1f}.")
    pat.expected = {
        "interpretation": {
            "needs_user_confirmation": True,
            "built": "swap as a sideways U excursion (4 vias) because rule 3 says the swap is made at a via pair when there "
                     "is room; reading of the user's feedback: " + READING,
            "open_question": (f"With 25 mm per via the 4-via swap ({cr['cost']:.1f}) is dearer than a single-track hop "
                              f"({ca['cost']:.1f}) at U = {UNCOUPLED_PRICE}; it wins only above U* = {u_star:.1f}. Is 'cheaper "
                              "of' meant to apply when room exists too, or only without room (assumed)? Also: 'also can "
                              "swap pairs' - read as 'the pair swaps polarity'; if it meant swapping two PAIRS with each "
                              "other, say so.")},
        "rules": _rules_block(),
        "polarity": {"P_left_at_source": True, "P_left_at_destination_arrival": False,
                     "turn_backs_P": turn_backs(ref, "USB_P"), "turn_backs_N": turn_backs(ref, "USB_N"),
                     "swaps_needed": 1, "via_pairs": 2,
                     "argument": "handedness differs at the ends (one flip needed); via pair A is a turn-back (flip), via "
                                 "pair B is not (heading kept): one flip, two via pairs"},
        "vias": 4, "crossings": 0, "via_sites": pat.extra["s1"],
        "gateways": [{"id": g["id"], "pt": rp(g["pt"])} for g in pat.gateways],
        "swap_site": {"shape": "sideways U excursion north of the line (vertical footprint)",
                      "measured_box_with_clearance": [r4(bx0), r4(by0), r4(bx1), r4(by1)],
                      "minimum_footprint": {k: r4(v) for k, v in fp.items()},
                      "argument": "north extent = 0.5 (E4 above the line) + 0.75 (via fan around via pair B) + 0.3 + s + 0.1 "
                                  "(B.Cu leg) + 0.375 (via fan of pair A) + via radius 0.225 + clearance 0.2 = 2.45 + s; "
                                  "width = s + 2 x (0.35 + 0.225 + 0.2); s >= 1.05 so that the P via of pair B clears the N "
                                  "track of the up leg"},
        "lengths": {"P": cr["length_P"], "N": cr["length_N"], "hand_mean_track": r4(h["mean_track_S1"]),
                    "skew_P_minus_N": cr["skew_P_minus_N"], "excursion_extra_vs_straight_octilinear": r4(extra)},
        "cost": cr,
        "price_table": {
            "model": f"length sum + {VIA_PRICE:.0f} mm per via + U x uncoupled mm, U = {UNCOUPLED_PRICE} (proposal)",
            "S1_swap_4_vias (reference)": cr, "S2_two_single_tracks_one_hop_2_vias (alternative panel)": ca,
            "S3_two_single_tracks_N_on_B.Cu_2_vias (estimate, not drawn)": {
                "length_sum_lower_bound": r4(h["S3_length_lower_bound"]), "vias": 2,
                "uncoupled_sum": r4(h["S3_length_lower_bound"]), "cost": s3_cost},
            "break_even_U_swap_vs_hop": r4(u_star)},
        "alternative": {"label": alt.label, "cost": ca, "hand_P": r4(h["S2_P"]), "hand_N": r4(h["S2_N"]),
                        "geometry": pat.extra["s2"],
                        "why_not_chosen": "rule 3 (swap at a via pair when there is room); a 2-via hop is 'an option at "
                                          "most' (d7b prompt); it is the cheaper one on price"},
        "judgement_calls": [
            f"Swap vs single hop when room exists (above); U = {UNCOUPLED_PRICE} per uncoupled mm is a proposal.",
            "Single tracks use the class track width 0.25 (pair values are for pair routing only); P against N keeps "
            "0.15 anyway because KiCad judges them as a coupled pair by name.",
            "Via pair B keeps the pair orientation (no turn-back): 'one pair to bring the traces back to the surface'; "
            "it sits at the foot of the down leg so the F.Cu body rejoins its line there.",
            "Excursion direction (north) and the shift s = 1.1 are free choices of the same cost class; the bend "
            "stretches (0.1 / 0.25 straights) are as in dp_gateway."],
        "literature": _common_lit() + ["TopoR 6.3 manual p. 173 'Routing Differential Pairs': 'To remove overlap between "
                                       "the wires of a differential pair, use the net swap feature' (Swap Nets); p. 173: "
                                       "changing layer adds a slider and a pair of vias; p. 171: a zipped pair needs "
                                       "straights >= 3 x the pair width"],
    }


def finish_no_room(pat: DPat) -> None:
    h = hand_no_room(pat)
    ref, alt = pat.ref, pat.alt
    cr, ca = cost(ref), cost(alt)
    assert _close(track_len(ref, "USB_P"), h["P"], 0.005) and _close(track_len(ref, "USB_N"), h["N"], 0.005)
    assert _close(mean_len(alt), h["S1_elsewhere_mean"], 0.005), (mean_len(alt), h["S1_elsewhere_mean"])
    fp = h["footprint"]
    cor = pat.extra["corridor"]
    corridor = cor["height"]
    # the honesty of "no room": the U needs north >= 3.55 above the line and 2.65 width; the corridor is 0.70 above the
    # line, a door is 1.8 wide (one leg + the pair band fits, both legs together do not)
    assert fp["north"] > corridor / 2 and fp["width"] > h["door_width"] and fp["via_pair_width"] + 2 * CLEAR < h["door_width"]
    assert corridor >= h["S2_footprint_height"]
    u_star = (ca["length_sum"] + VIA_PRICE * 4 - cr["length_sum"] - VIA_PRICE * 2) / (
        cr["uncoupled_sum"] - ca["uncoupled_sum"])
    pat.purpose = ("Like dp_swap_needed (P/N mirrored), but the route runs in a corridor 1.40 high between through-hole "
                   "bodies with two doors 1.8 wide to an open strip. The pair fits the corridor (0.95 + 2 x 0.2 = 1.35); "
                   f"one swap U excursion needs {fp['width']:.2f} x {fp['north']:.2f}: no room in the corridor, and a door "
                   "(1.8) takes one leg but not both. Rule 4: the cheaper of the swap elsewhere (with its detour) or two "
                   "single tracks.")
    pat.answer = (f"Two single tracks through the corridor (uncoupled, reported), N hops under P: 2 vias, pair decoupled "
                  f"over {cr['uncoupled_sum']:.2f} mm (zone x 11.0 .. 13.4, under C2), price {cr['cost']:.1f} = "
                  f"{cr['length_sum']:.2f} mm + 2 x 25 + {UNCOUPLED_PRICE} x {cr['uncoupled_sum']:.2f}. The swap elsewhere "
                  f"(F.Cu up through door 1, via pair A and the turn-back in the open strip, B.Cu east along the strip and "
                  f"down through door 2, via pair B at the foot of door 2) costs {ca['cost']:.1f} = {ca['length_sum']:.2f} mm "
                  f"+ 4 x 25: dearer by {ca['cost'] - cr['cost']:.1f}, for every U up to {u_star:.1f}.")
    pat.why = (f"What 'no room' means for the U excursion: it needs {fp['north']:.2f} free above the line and {fp['width']:.2f} "
               f"across (via pair A + via pair B {S_SHIFT} apart + clearances). The corridor has {corridor / 2:.2f} above the "
               f"line (wall of the THT bodies), and the doors are 1.8 wide: one leg with its via pair needs "
               f"{fp['via_pair_width'] + 2 * CLEAR:.2f} and fits, both legs {fp['width']:.2f} do not. So the U can only be built "
               f"with its two legs in two different doors (the alternative panel): detour up {8.0 - 3.275:.2f}, along the strip "
               f"and down again. The single-track hop needs {h['S2_footprint_height']:.3f} high and fits the corridor. "
               f"Both options are drawn as copper and pass DRC.")
    pat.expected = {
        "interpretation": {
            "needs_user_confirmation": True,
            "built": "rule 4 default: cheaper of 'swap elsewhere with its detour' and 'two single tracks (uncoupled, "
                     "reported)'; 'two single tracks' read as: the pair decouples only around one single-track hop (2 vias); "
                     + READING,
            "open_question": "Is 'two single tracks' the whole pair decoupled (N on B.Cu from end to end, 2 vias) or the "
                             "local hop built here? The hop is cheaper; the whole-route variant costs about 130 at U = 2 "
                             "(see price table), close to the swap elsewhere."},
        "rules": _rules_block(),
        "polarity": {"swaps_needed": 1, "reference": "single-track hop: N crosses under P on B.Cu (no same-layer crossing)",
                     "alternative_turn_backs": turn_backs(alt, "USB_P")},
        "vias": 2, "crossings": 0,
        "corridor": {"height": corridor, "usable_height_after_clearances": r4(corridor - 2 * CLEAR), "doors": cor["doors"],
                     "door_width": h["door_width"], "pair_needs": r4(BAND),
                     "swap_U_needs": {"width": r4(fp["width"]), "north_of_line": r4(fp["north"])},
                     "one_leg_with_via_pair_needs_width": r4(fp["via_pair_width"] + 2 * CLEAR),
                     "hop_needs": r4(h["S2_footprint_height"]),
                     "argument": "the U needs both via pairs, S apart, in one free area; a door fits one leg, not both; so "
                                 "the swap needs the two legs in two doors (longer), or the pair does not swap at all"},
        "lengths": {"P": cr["length_P"], "N": cr["length_N"], "hand_P": r4(h["P"]), "hand_N": r4(h["N"]),
                    "decoupled_zone_hand": {"P": r4(h["zone_P"]), "N": r4(h["zone_N"]), "straight": 2.4}},
        "cost": cr,
        "price_table": {
            "model": f"length sum + {VIA_PRICE:.0f} mm per via + U x uncoupled mm, U = {UNCOUPLED_PRICE} (proposal)",
            "S2_two_single_tracks_one_hop (reference)": cr, "S1_swap_elsewhere_4_vias (alternative panel)": ca,
            "break_even_U_hop_vs_swap_elsewhere": r4(u_star),
            "S3_N_on_B.Cu_end_to_end_2_vias (estimate)": {
                "cost": r4(2 * 19.7 + 2 * VIA_PRICE + UNCOUPLED_PRICE * 2 * 19.7), "note": "2 x ~19.7 mm, all uncoupled"}},
        "alternative": {"label": alt.label, "cost": ca, "hand_mean_track": r4(h["S1_elsewhere_mean"]),
                        "geometry": pat.extra["s1_elsewhere"],
                        "detour": f"{ca['length_sum'] - 2 * mean_len(ref):+.2f} mm against the reference's mean track x 2"},
        "judgement_calls": [
            f"U = {UNCOUPLED_PRICE} mm per uncoupled mm and the hop reading of 'two single tracks' (above).",
            "With room the answer flips (dp_swap_needed): here the corridor removes the room, not the price.",
            "Door width 1.8: the pair passes (band 0.95 + 2 x 0.2); with doors 2.7 wide the U would fit one door and "
            "the expected answer would change to the swap in the door."],
        "literature": _common_lit(),
    }


def finish_uturn(pat: DPat) -> None:
    h = hand_uturn(pat)
    ref, car = pat.ref, pat.careless
    cr = cost(ref)
    assert _close(mean_len(ref), h["mean_track"], 0.005), (mean_len(ref), h["mean_track"])
    assert cr["skew_P_minus_N"] == 0 or abs(cr["skew_P_minus_N"]) < 1e-3
    tb = turn_backs(ref, "USB_P")
    assert tb == turn_backs(ref, "USB_N") == 1
    pat.purpose = ("The pair must change layer (J2's pads are on B.Cu) and the route makes a U-turn at the layer change. "
                   "A turn-back at a via pair flips P/N (user rule 3: a U-turn at a layer change counts as a swap), so "
                   "with the destination order equal to the source order the parity works out to the one via pair of the "
                   "layer change itself and NO extra swap.")
    pat.answer = (f"One via pair (2 vias) at x=6.0 on the row of J1: F.Cu east out of J1, via pair A, B.Cu back west "
                  f"(turn-back), then south and west into J2 (B.Cu pads). Swaps by parity: 1 flip needed (J1 heads east "
                  f"with P on the left; J2 is entered heading west with P' = the north pad = on the RIGHT) = 1 turn-back "
                  f"provided by the forced layer change: 0 extra swap sites. Per-track {cr['length_P']:.2f} mm (skew 0: left "
                  f"and right turns cancel), price {cr['cost']:.1f} = {cr['length_sum']:.2f} + 2 x 25.")
    pat.why = ("Parity rule: flips needed = [handedness at the source != handedness at the destination arrival] = 1 here "
               "(a smooth U on one layer, east - south - west, keeps P on the left = SOUTH on the westward leg, but P' is "
               "north). Turn-backs available at the layer change: 1. 1 = 1, so the layer change needs no swap site. A "
               "router that counts only the pad order (same on both connectors) and forgets the turn-back adds 2 via pairs "
               "(+100 mm); one that forgets the flip lands P on the N' pad (the careless panel: a same-layer crossing).")
    pat.expected = {
        "interpretation": {"needs_user_confirmation": True,
                           "built": "U-turn at a layer change counts as one swap (user 2026-11-10); here it is the whole swap",
                           "open_question": "A turn-back via pair that is also the only layer change: is it one via pair (2 "
                                            "vias, as drawn) or one via pair + the swap's 'bring back to the surface' pair? "
                                            "J2 is on B.Cu so no return to F.Cu is needed; I count 2 vias."},
        "rules": _rules_block(),
        "polarity": {"P_left_at_source": True, "P_left_at_destination_arrival": False, "turn_backs_P": tb,
                     "turn_backs_N": tb, "flips_needed": 1, "swaps_beyond_the_layer_change": 0, "via_pairs": 1,
                     "argument": "flips needed = (handedness source) xor (handedness arrival) = 1; flips provided = turn-backs "
                                 "= 1; extra swaps = 0"},
        "vias": 2, "crossings": 0, "via_sites": {"A": [rp((6.0, 3.65)), rp((6.0, 4.35))]},
        "lengths": {"P": cr["length_P"], "N": cr["length_N"], "hand_mean_track": r4(h["mean_track"]),
                    "skew_P_minus_N": cr["skew_P_minus_N"]},
        "cost": cr,
        "careless": {"description": "smooth U on F.Cu (P stays on the left = south), plain layer change near J2 without "
                                    "turn-back: P arrives on the south pad's side and must cross N to reach P' (X1)",
                     "crossings": 1, "cost_if_repaired_by_swap_site_lower_bound": r4(cost(car)["length_sum"] + 6 * VIA_PRICE)},
        "judgement_calls": ["J2 as an SMD connector on the bottom side (a real board would mirror its footprint; the pads "
                            "here are B.Cu SMD with B.Mask).",
                            "Whether a turn-back that is needed ANYWAY for the layer change is allowed to be placed so that "
                            "the arrival fan comes in 'from behind' (reported as a graded cost, not a wall)."],
        "literature": _common_lit(),
    }


FINISH = {"dp_gateway": finish_gateway, "dp_swap_needed": finish_swap_needed, "dp_swap_no_room": finish_no_room,
          "dp_uturn_parity": finish_uturn}
BUILD = {"dp_gateway": p_gateway, "dp_swap_needed": p_swap_needed, "dp_swap_no_room": p_swap_no_room,
         "dp_uturn_parity": p_uturn_parity}
NAMES = list(BUILD)


# ----------------------------------------------------------------------------------------------- pattern 5: bus crossing
BUS_X = [11.75, 13.25, 14.75, 16.25]  # pair centre lines; pitch 1.5 as on the qmtech HDMI bus (measured: 1.5)
PNETS = [n for k in range(4) for n in (f"TX{k}_P", f"TX{k}_N")]
CNETS = ["SIG_A", "SIG_B", "SIG_C"]
BUS_L, BUS_R = BUS_X[0] - HALF - DP_W / 2, BUS_X[-1] + HALF + DP_W / 2  # copper edges of the bus: 11.475 / 16.525
_PC = ["#1f77b4", "#2ca02c", "#9467bd", "#8c564b"]
_NC = ["#6baed6", "#98df8a", "#c5b0d5", "#c49c94"]
for _k in range(4):
    COLORS[f"TX{_k}_P"], COLORS[f"TX{_k}_N"] = _PC[_k], _NC[_k]
COLORS.update({"SIG_A": "#ff7f0e", "SIG_B": "#17becf", "SIG_C": "#bcbd22"})
BUS_CLASS = dict(CLASS_USB, name="HDMI_TX")
X_VIA_L, X_VIA_R = 10.4, 17.6  # crosser vias: 1.075 / 1.075 centre to bus copper edge (>= 0.2 + 0.225 needed)
YA, YB = 4.2, 13.6  # rows of via pairs A / B in the alternative (pairs dive under the crossers)


def _cost_bus(sol: Sol, u: float = UNCOUPLED_PRICE) -> dict:
    nets = sorted({t.net for t in sol.tracks})
    ln = {n: track_len(sol, n) for n in nets}
    unc = sum(uncoupled_len(sol, n) for n in PNETS)
    tot = sum(ln.values()) + VIA_PRICE * len(sol.vias) + u * unc
    return {"length_sum": r4(sum(ln.values())), "vias": len(sol.vias), "uncoupled_pairs": r4(unc), "cost": r4(tot),
            "pair_lengths": {f"TX{k}": [r4(ln[f"TX{k}_P"]), r4(ln[f"TX{k}_N"])] for k in range(4)},
            "pair_skew_max": r4(max(abs(ln[f"TX{k}_P"] - ln[f"TX{k}_N"]) for k in range(4))),
            "crossers": {n: r4(ln[n]) for n in CNETS}}


def p_bus_crossing() -> DPat:
    W, H = 28.0, 18.0
    L, WD = 1.0, 0.3
    yu, yj = 3.0, 15.0
    walls_x = [BUS_X[0] - 0.75] + [x + 0.75 for x in BUS_X]
    u1 = pad_pair("U1", [(n, (BUS_X[k // 2] + (-0.25 if n.endswith("P") else 0.25), yu)) for k, n in enumerate(PNETS)],
                  (0, 1), L, WD, [(x, yu) for x in walls_x])
    j1 = pad_pair("J1", [(n, (BUS_X[k // 2] + (-0.25 if n.endswith("P") else 0.25), yj)) for k, n in enumerate(PNETS)],
                  (0, -1), L, WD, [(x, yj) for x in walls_x])
    ya = [("A1", "SIG_A", (4.0, 5.0)), ("A2", "SIG_B", (4.0, 6.5)), ("A3", "SIG_C", (4.0, 12.8))]
    yb = [("B1", "SIG_A", (24.0, 5.0)), ("B2", "SIG_B", (24.0, 11.5)), ("B3", "SIG_C", (24.0, 12.8))]
    sig = [Fp(r, [Pad("1", n, pos, (0.8, 0.8))], "terminal") for r, n, pos in ya + yb]
    fps = [u1, j1] + sig
    top, bot, Gt, Gb, d0s = [], [], [], [], []
    for k in range(4):
        pu = [p.pos for p in u1.pads[2 * k:2 * k + 2]]
        pj = [p.pos for p in j1.pads[2 * k:2 * k + 2]]
        d_t, G_t, tp, tn = exit_fan(fps, pu[0], pu[1], (0, 1), L, 0.5)
        d_b, G_b, bp, bn = exit_fan(fps, pj[0], pj[1], (0, -1), L, 0.5)
        top.append((tp, tn)), bot.append((bp, bn)), Gt.append(G_t), Gb.append(G_b), d0s.append((d_t, d_b))
    # ---- expected: straight coupled bodies on F.Cu, crossers dive under the whole bus on B.Cu
    paths = {}
    coupled, fans = [], []
    for k in range(4):
        C = [Gt[k], Gb[k]]
        bp_, bn_ = body_pair(C, -1)  # P (west) is on the right of the southward travel direction
        for net, b, t, d in ((f"TX{k}_P", bp_, top[k][0], bot[k][0]), (f"TX{k}_N", bn_, top[k][1], bot[k][1])):
            paths[net] = [("F.Cu", t + b[1:] + d[::-1][1:], DP_W)]
        coupled.append(("F.Cu", C))
        fans += [("F.Cu", top[k][0]), ("F.Cu", top[k][1]), ("F.Cu", bot[k][0]), ("F.Cu", bot[k][1])]
    pa = {r: pos for r, _, pos in ya}
    pb = {r: pos for r, _, pos in yb}
    paths["SIG_A"] = [("F.Cu", [pa["A1"], (X_VIA_L, 5.0)], W_NORMAL), ("B.Cu", [(X_VIA_L, 5.0), (X_VIA_R, 5.0)], W_NORMAL),
                      ("F.Cu", [(X_VIA_R, 5.0), pb["B1"]], W_NORMAL)]
    paths["SIG_B"] = [("F.Cu", [pa["A2"], (X_VIA_L, 6.5)], W_NORMAL),
                      ("B.Cu", [(X_VIA_L, 6.5), (10.9, 6.5), (15.9, 11.5), (X_VIA_R, 11.5)], W_NORMAL),
                      ("F.Cu", [(X_VIA_R, 11.5), pb["B2"]], W_NORMAL)]
    paths["SIG_C"] = [("F.Cu", [pa["A3"], (X_VIA_L, 12.8)], W_NORMAL),
                      ("B.Cu", [(X_VIA_L, 12.8), (X_VIA_R, 12.8)], W_NORMAL), ("F.Cu", [(X_VIA_R, 12.8), pb["B3"]], W_NORMAL)]
    ref = sol_from("reference: pairs stay on F.Cu, crossers dive under the bus (6 vias)",
                   paths, coupled, fans)
    # ---- alternative (careless): crossers straight on F.Cu, every pair dives instead (4 vias per pair, 16 in all)
    paths2, coupled2, fans2 = {}, [], []
    for k in range(4):
        X = BUS_X[k]
        E1, E2 = (X, YA - 0.375), (X, YA + 0.375)
        E3, E4 = (X, YB - 0.375), (X, YB + 0.375)
        C_in, C2, C_out = [Gt[k], E1], [E2, E3], [E4, Gb[k]]
        south, north = (0, 1), (0, -1)
        b1, b2, b3 = body_pair(C_in, -1), body_pair(C2, -1), body_pair(C_out, -1)
        fA = fan(E1, south, lnorm(south), -1, VIA_PITCH, ST_VIA)
        f2s = fan(E2, north, lnorm(south), -1, VIA_PITCH, ST_VIA)
        f2e = fan(E3, south, lnorm(south), -1, VIA_PITCH, ST_VIA)
        f3s = fan(E4, north, lnorm(south), -1, VIA_PITCH, ST_VIA)
        for i, net in enumerate((f"TX{k}_P", f"TX{k}_N")):
            p1 = top[k][i] + b1[i][1:] + fA[i][1:]
            p2 = f2s[i][::-1] + b2[i][1:] + f2e[i][1:]
            p3 = f3s[i][::-1] + b3[i][1:] + bot[k][i][::-1][1:]
            paths2[net] = [("F.Cu", p1, DP_W), ("B.Cu", p2, DP_W), ("F.Cu", p3, DP_W)]
        coupled2 += [("F.Cu", C_in), ("B.Cu", C2), ("F.Cu", C_out)]
        fans2 += [("F.Cu", f) for f in (*top[k], *bot[k], *fA, *f3s)] + [("B.Cu", f) for f in (*f2s, *f2e)]
    for (r1, n, a), (r2, _, b_) in zip(ya, yb):
        paths2[n] = [("F.Cu", [a, b_], W_NORMAL)]
    alt = sol_from("crossers straight on F.Cu, the pairs dive (16 vias)", paths2, coupled2, fans2)
    connections = [(f"TX{k}_{s}", f"U1.{2 * k + (1 if s == 'P' else 2)}", f"J1.{2 * k + (1 if s == 'P' else 2)}")
                   for k in range(4) for s in "PN"] + [(n, a, b) for (a, n, _), (b, _, _) in
                                                       zip([(r, n, p) for r, n, p in ya], [(r, n, p) for r, n, p in yb])]
    connections = [(n, a if "." in a else f"{a}.1", b if "." in b else f"{b}.1") for n, a, b in connections]
    return DPat(
        name="dp_bus_crossing", title="4 diff pairs in a bus, 3 single signals cross it: the crossers dive, the pairs stay",
        kind="solvable", purpose="", answer="", why="", size=(W, H), fps=fps, ref=ref, alt=alt, careless=None, expected={},
        connections=connections, gateways=[], nets=PNETS + CNETS, classes=[dict(CLASS_DEFAULT), dict(BUS_CLASS)],
        net_patterns=[{"netclass": "HDMI_TX", "pattern": "TX*"}],
        extra={"Gt": Gt, "Gb": Gb, "d0": d0s, "ya": ya, "yb": yb})


def hand_bus(pat: DPat) -> dict:
    pad = 0.5 + 0.05 + 0.075 * SQ2  # pad centre to the end of the 45 degree legs (run-out 0.05, as dp_gateway J1)
    body = (15.0 - 0.5 - 0.05 - 0.075) - (3.0 + 0.5 + 0.05 + 0.075)
    track = body + 2 * pad
    s2 = 6.4 + 0.5 + 5 * SQ2 + 1.7 + 6.4
    cross = {"SIG_A": 20.0, "SIG_B": s2, "SIG_C": 20.0}
    alt_track = 2 * pad + (YA - 0.375 - (3.0 + 0.5 + 0.05 + 0.075)) + 4 * _via_fan() + ((YB - 0.375) - (YA + 0.375)) \
        + ((15.0 - 0.5 - 0.05 - 0.075) - (YB + 0.375))
    alt_cross = {"SIG_A": 20.0, "SIG_B": math.hypot(20.0, 5.0), "SIG_C": 20.0}
    return {"track": track, "body": body, "crossers": cross, "alt_track": alt_track, "alt_crossers": alt_cross,
            "ref_total": 8 * track + sum(cross.values()) + 6 * VIA_PRICE,
            "alt_total": 8 * alt_track + sum(alt_cross.values()) + 16 * VIA_PRICE}


def check_bus(pat: DPat) -> dict:
    ref, alt = pat.ref, pat.alt
    h = hand_bus(pat)
    c = _cost_bus(ref)
    assert (c["vias"], _cost_bus(alt)["vias"]) == (6, 16), c
    for sol in (ref, alt):
        assert all(t.width == DP_W for t in sol.tracks if t.net in PNETS)
        assert all(t.width == W_NORMAL for t in sol.tracks if t.net in CNETS)
        assert all((v.dia, v.drill) == (VIA_D, VIA_DRILL) for v in sol.vias)
        for n in pat.nets:
            assert connected(pat, sol, n), (sol.label, n)
        rep = clearance_report(pat, sol)
        assert rep["margin"] >= -1e-6 and rep["edge_margin"] >= -1e-6, (sol.label, rep)
    # pairs: constant gap on the bodies, no skew, the reference touches them with nothing
    for k in range(4):
        P, N = f"TX{k}_P", f"TX{k}_N"
        assert abs(track_len(ref, P) - track_len(ref, N)) < 1e-6 and _close(track_len(ref, P), h["track"], 0.005)
        nn = unary_union([LineString(t.pts).buffer(DP_W / 2) for t in ref.tracks if t.net == N])
        for t in ref.tracks:
            if t.net == P:
                for a, b in zip(t.pts, t.pts[1:]):
                    m = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
                    if any(Point(m).distance(LineString(cl)) <= HALF * 1.001 + 1e-6 for _, cl in ref.coupled):
                        assert _close(Point(m).distance(nn) - DP_W / 2, DP_GAP, 1e-3)
    assert all(t.layer == "F.Cu" for t in ref.tracks if t.net in PNETS) and not any(v.net in PNETS for v in ref.vias)
    # crossers: vias outside the bus (never between P and N, never between pairs), crossing the bus on B.Cu
    for v in ref.vias:
        assert v.pos[0] <= BUS_L - CLEAR - VIA_R + 1e-9 or v.pos[0] >= BUS_R + CLEAR + VIA_R - 1e-9, v
    bus = box(BUS_L, 0, BUS_R, 18)
    angles = []
    for t in ref.tracks:
        if t.net in CNETS and t.layer == "B.Cu":
            for a, b in zip(t.pts, t.pts[1:]):
                if LineString([a, b]).intersects(bus):
                    ang = math.degrees(math.atan2(abs(b[0] - a[0]), abs(b[1] - a[1])))  # 90 = perpendicular to the bus
                    angles.append(round(ang, 1))
                    assert ang >= 44.9, (t.net, ang)
    for n in CNETS:
        assert _close(track_len(ref, n), h["crossers"][n], 0.005), (n, track_len(ref, n))
    ca = _cost_bus(alt)
    assert _close(ca["cost"] - ca["uncoupled_pairs"] * UNCOUPLED_PRICE, h["alt_total"], 0.01), (ca, h)
    assert _close(c["cost"] - c["uncoupled_pairs"] * UNCOUPLED_PRICE, h["ref_total"], 0.01), (c, h)
    assert c["cost"] < ca["cost"]
    return {"angles": angles}


def finish_bus(pat: DPat) -> None:
    h = hand_bus(pat)
    ref, alt = pat.ref, pat.alt
    cr, ca = _cost_bus(ref), _cost_bus(alt)
    angles = check_bus(pat)["angles"]
    split_est = 16 * VIA_PRICE + UNCOUPLED_PRICE * 8 * 2.4  # one crosser kept on F.Cu: every P and N hops (2 vias each)
    pat.purpose = ("A bus of 4 coupled pairs (qmtech HDMI: ADV7511 -> connector, pair pitch 1.5) is crossed by 3 single "
                   "signals whose direct path cuts it. Question: do the normal traces disrupt the pairs? Expected: no. The "
                   "crossers take the other layer under the whole bus with their vias outside it; every pair keeps its "
                   "constant gap, equal P/N lengths and its layer.")
    pat.answer = (f"Pairs: 4 straight coupled bodies on F.Cu (gap 0.15 constant, pad fans of the dp_gateway kind, 0 vias, P=N "
                  f"{h['track']:.3f} mm, skew 0). Crossers SIG_A / SIG_B / SIG_C: F.Cu to a via 1.075 beyond the bus edge, B.Cu "
                  f"under the whole bus ({BUS_L:.3f} .. {BUS_R:.3f}), via on the far side, F.Cu on: 2 vias each, 6 in all, "
                  f"lengths {h['crossers']['SIG_A']:.1f} / {h['crossers']['SIG_B']:.2f} / {h['crossers']['SIG_C']:.1f}; crossing "
                  f"angles {angles} deg to the bus (90 = perpendicular). Price {cr['cost']:.1f} = {cr['length_sum']:.2f} mm + "
                  f"6 x 25 + U x {cr['uncoupled_pairs']:.1f} uncoupled. The careless layout (crossers straight on F.Cu, every "
                  f"pair dives with 4 vias) costs {ca['cost']:.1f}: +{ca['cost'] - cr['cost']:.1f}, and cuts every pair into "
                  f"three layer segments.")
    pat.why = ("A crosser must change layer: on F.Cu it would cross the pairs' tracks (short circuits). The only places a "
               "crosser can reach from its pad on F.Cu are outside the bus, so its two vias are outside it (inside the 0.95 "
               "gap between two pairs a via would fit, 0.45 + 2 x 0.2 = 0.85, but nothing can get there without crossing a pair "
               "first, and between P and N (gap 0.15) no via fits at all). One dive under the whole bus costs 2 vias "
               "however many pairs it crosses; making the pairs give way costs 4 vias per pair (16) and breaks the pairs' "
               "coupling at every via pair. Crossing angles: B.Cu is a free layer here, so 90 degrees (S1, S3) and 45 (S2, "
               "as on qmtech) are both legal; a graded crossing-angle cost would prefer 90.")
    pat.expected = {
        "interpretation": {"needs_user_confirmation": True,
                           "built": "2-layer board, pairs on F.Cu, crossers on B.Cu under the bus; pair values are the module's "
                                    "(0.20 / 0.15 / 0.25), not qmtech's, so that all patterns share one Board Setup",
                           "open_question": "Reference plane: qmtech is 4-layer (F, In1 GND plane, In2 3V3 plane, B) and its "
                                            "pairs run over the unbroken In1 GND plane; the crossers' B.Cu stretches are below "
                                            "that plane and their vias (>= 2.4 mm from the bus on qmtech) only punch antipads "
                                            "outside the bus. On a 2-layer board there is no plane, so the crossers under the "
                                            "pairs would cut the pairs' return path on B.Cu. Should the planner keep crosser "
                                            "vias a minimum distance from the bus (here 1.075 centre to bus edge) and/or "
                                            "price B.Cu runs under a pair (graded), and what about a plane under the pairs? "
                                            "Not decided here."},
        "rules": _rules_block(),
        "qmtech_grounding": {
            "board": "scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb (hand routed, read on a copy)",
            "copper_layers": 4, "stackup": ["F.Cu", "In1.Cu GND plane", "In2.Cu 3V3 plane", "B.Cu"],
            "pairs": ["ADV_TX2+/-", "ADV_TX1+/-", "ADV_TX0+/-", "ADV_TXC+/-"],
            "pair_routing": "all 8 tracks on F.Cu only, 0 vias, about 15.9 mm per track, vertical (x 130.2 .. 135.8, y 55.1 .. "
                            "70.1), width 0.2",
            "pair_values_in_board_setup": {"class HDMI_TX (patterns ADV_TX, ADV_TX*)": {"track": 0.2, "clearance": 0.127,
                                                                                         "via": [0.45, 0.3], "dp_width": 0.16,
                                                                                         "dp_gap": 0.2, "dp_via_gap": 0.25},
                                           "Default": {"dp_width": 0.2, "dp_gap": 0.25, "dp_via_gap": 0.25}},
            "hand_measured": {"pair_centre_pitch_P_N": "0.45 (edge gap 0.25 = the Default class gap, not HDMI_TX 0.2)",
                              "pair_to_pair_centre_pitch": 1.5, "pair_x_centres": [130.7, 132.2, 133.7, 135.2],
                              "gnd_stitching_vias": "on the pair centre lines x = 130.7 / 132.2 / 133.7 / 135.2 at y 58.3, 60.2, "
                                                    "64.0, 65.9 (between the pair's straight sections)"},
            "crossers_in_the_hand_layout": {
                "ADV_DDCSDA": "B.Cu, 45 degrees, vias at x 140.0 (4.2 east of the bus) and x 123.0",
                "ADV_DDCSCL": "B.Cu, 45 degrees + horizontal, vias at x 140.0 and 123.8",
                "/HDMI transmitter/HDMI5V": "B.Cu horizontal (90 degrees), y 61.2 from x 137.1 to 129.2, vias at 127.7 and 142.9",
                "ADV_CEC": "B.Cu 45 degrees (and F.Cu beside the bus at x 136.2), vias at 138.2 and 127.0",
                "/rtc-spi/vbat": "B.Cu 45 degrees across the top end, vias far away (x 97.7 / 160.7)"},
            "summary": "5 crossers, all on B.Cu under the F.Cu pairs (below the In1 GND plane), crossing at 45 or 90 degrees, "
                       "every via 2.4 mm or more outside the bus, none between pairs or inside one"},
        "polarity": {"swaps_needed": 0, "turn_backs": 0},
        "vias": 6, "crossings": 0, "via_sites": {"left_x": X_VIA_L, "right_x": X_VIA_R, "bus_copper_edges": [BUS_L, BUS_R],
                                                  "margin_to_bus_edge_centre": r4(BUS_L - X_VIA_L)},
        "bus": {"pairs": 4, "pair_pitch": 1.5, "inter_pair_edge_gap": r4(1.5 - PITCH - DP_W),
                "via_fits_in_inter_pair_gap": r4(VIA_D + 2 * CLEAR) <= r4(1.5 - PITCH - DP_W),
                "crossing_angles_deg": angles},
        "lengths": {"pair_track_hand": r4(h["track"]), "crossers_hand": {k: r4(v) for k, v in h["crossers"].items()},
                    "pair_skew_max": cr["pair_skew_max"]},
        "cost": cr,
        "price_table": {
            "model": f"length sum + {VIA_PRICE:.0f} mm per via + U x uncoupled mm (pairs only), U = {UNCOUPLED_PRICE}",
            "reference (crossers dive, 6 vias)": cr, "careless (pairs dive, 16 vias)": ca,
            "estimate: a crosser kept on F.Cu and every P and N hopping singly under it": {
                "vias_per_crosser": 16, "uncoupled_mm_per_crosser": r4(8 * 2.4),
                "cost_added_per_crosser": r4(split_est), "note": "not drawn"}},
        "alternative": {"label": alt.label, "cost": ca, "vias": 16,
                        "disrupts": "every pair changes layer twice (4 vias per pair): 4 coupled bodies become 12 segments"},
        "judgement_calls": [
            "Crosser vias sit 1.075 mm (centre) from the bus copper edge on both sides: a graded distance, the minimum is "
            "0.2 + 0.225; qmtech has them >= 2.4 mm out.",
            "Crossing angle: S2 is 45 degrees as on qmtech; S1, S3 perpendicular. Cost model does not price the angle.",
            "Crossers use the class track width 0.25 and the smallest vias 0.45/0.25 (supplier rule), not the qmtech class "
            "via 0.8/0.4.",
            "Pair values 0.20 / 0.15 / 0.25 are the module's; qmtech's HDMI_TX class is 0.16 / 0.2 / 0.25 (hand layout 0.2 / "
            "0.25), clearance 0.127: using them needs the module's constants per pattern."],
        "literature": _common_lit() + ["TopoR 6.3 manual p. 173: changing layer with a pair adds a via pair (pairs that dive "
                                       "cost 4 vias in this model); p. 171: a pair is routed as one zipped wire"],
    }


BUILD["dp_bus_crossing"] = p_bus_crossing
FINISH["dp_bus_crossing"] = finish_bus
CHECK = {"dp_bus_crossing": lambda pat: None}  # finish_bus runs check_bus
NAMES = list(BUILD)


# ----------------------------------------------------------------------------------------------- patterns 6, 7: GND pad between P and N
# HDMI-like rows (qmtech USB1 HDMI-001S and U3 TPD12S520): P / GND / N per pair at 0.5 pitch, the GND (shield) pad in
# the middle of each pair. GND is a B.Cu pour over the whole board (2 layers like every pattern here); a GND pad reaches
# it through one via of the pattern via size. The pour is a terminal (planes-are-terminals): a via on it is always
# allowed, routing into it is free.
GND = "GND"
COLORS[GND] = "#b8860b"
GPNETS = ["TX0_P", "TX0_N", "TX1_P", "TX1_N"]
PGN = ("TX0_P", GND, "TX0_N", "TX1_P", GND, "TX1_N")  # HDMI order: D2+ / shield / D2- / D1+ / shield / D1-
HP = 0.5  # pad pitch of the rows (qmtech USB1 and U3: 0.5)
JL, JW = 2.6, 0.3  # connector pads (qmtech USB1: 0.3 x 2.6)
UQL = 1.6  # transmitter QFP pads (qmtech ADV7511 LQFP-100: 0.3 x 1.6, pairs P N adjacent, pair pitch 1.5)
UTL = 1.5  # ESD chip pads (qmtech U3 TPD12S520 TSSOP-38: 0.3 x 1.475)
# qmtech Board Setup (scratch/boards/qmtech-hand/qmtech-minimig.kicad_pro, read 2026-11-10), used by dp_esd_flowthrough:
# Constraints page minimums and the clearances of the classes involved (GND 0.15, HDMI_TX 0.127; KiCad applies the larger
# of the two items' class clearances, so GND copper against a pair track needs 0.15)
QM_RULES = {"min_clearance": 0.09, "min_track_width": 0.09, "min_via_diameter": 0.45, "min_through_hole_diameter": 0.2,
            "min_via_annular_width": 0.1, "min_hole_clearance": 0.25, "min_hole_to_hole": 0.25}
QM_CLEAR = 0.15
QM_DRILL = QM_RULES["min_through_hole_diameter"]  # smallest drill allowed; pi x 0.2 = 0.63 >= the 0.25 GND stub
QM_VIA_D = max(QM_RULES["min_via_diameter"], QM_DRILL + 2 * QM_RULES["min_via_annular_width"])  # 0.45 (0.4 by the ring)
QM_CLASS_VIA = (0.8, 0.4)  # qmtech GND / Default class via: what a router takes when it uses the net-class size
QM_CLASSES = [
    {"name": "Default", "clearance": QM_CLEAR, "track_width": 0.25, "via_diameter": 0.8, "via_drill": 0.4,
     "priority": 2147483647},
    {"name": "GND", "clearance": QM_CLEAR, "track_width": 0.25, "via_diameter": 0.8, "via_drill": 0.4, "priority": 3},
    {"name": "HDMI_TX", "clearance": 0.127, "track_width": 0.2, "via_diameter": 0.45, "via_drill": 0.3, "priority": 4,
     "diff_pair_width": DP_W, "diff_pair_gap": DP_GAP, "diff_pair_via_gap": DP_VIA_GAP}]
ESD_DX = 0.05  # connector row 0.05 west of the ESD rows (qmtech: 0.027); absorbed inside the connector pads
ESD_ROWS = 5.75  # ESD chip row pitch (qmtech U3 TPD12S520 TSSOP-38: 5.726)
J1_TO_ESD = 4.15  # connector row -> ESD top row (qmtech: 4.137)
ESD_TO_TX = 5.2  # ESD bottom row -> transmitter row (qmtech: 64.963 -> 70.159)
GND_W = W_NORMAL  # GND stub: Default class track width


def _grid_up(x: float, g: float = 0.05) -> float:
    return r4(math.ceil(x / g - 1e-9) * g)


# via centre beyond the GND pad's end so that the via clears the P and N pad corners (0.5 - 0.15 = 0.35 across):
# sqrt(0.35^2 + d^2) >= 0.225 + 0.2 -> d >= 0.2411 -> 0.25
GND_VIA_D = _grid_up(math.sqrt((VIA_R + CLEAR) ** 2 - (HP - JW / 2) ** 2))
HDMI_LIT = ("TopoR 6.3 User Manual (docs/literature/eremex2017-topor63-user-manual.pdf) printed p. 171 'Routing "
            "Differential Pairs': clicking a pad creates the two wires at the pads, a slider is placed and 'from now on, the "
            "zipped wire pair is routed as a single wire'; at the far pads the slider is placed at the end of the zipped "
            "wire and wires connect it to the pads (our gateway = TopoR's slider); zipping needs straights >= 3 x (2 w + "
            "gap) = 1.65 mm here")
KICAD_COUPLING = ("docs/dev/kicad-diff-pairs.md 2.3: KiCad's DRC drops a coupled candidate when other copper lies "
                  "between the nearest P and N points, so a GND via between P and N makes that stretch uncoupled; 3.2: "
                  "the pad fan-out is a short uncoupled stub")


def _pgn_fp(ref, x0, y, L, pins, *, wall_l=True, wall_r=True):
    """One pad row at pitch HP from x0 (pins in order, None = no-net pin), pads JW wide and L long (vertical), plus
    no-net wall pads one pitch beyond each end."""
    pads = [Pad(str(i + 1), n, (r4(x0 + i * HP), y), (JW, L)) for i, n in enumerate(pins)]
    k = len(pins)
    if wall_l:
        pads.append(Pad(str(k + 1), None, (r4(x0 - HP), y), (JW, L)))
    if wall_r:
        pads.append(Pad(str(k + 2), None, (r4(x0 + k * HP), y), (JW, L)))
    return Fp(ref, pads, "terminal")


def _pad(fp, net, i=0):
    return [p for p in fp.pads if p.net == net][i]


def _coupled_len(sol: Sol, net: str) -> float:
    """Length of the net's segments lying in a coupled body (segment midpoint HALF from a body centre line)."""
    tot = 0.0
    for t in sol.tracks:
        if t.net != net:
            continue
        for a, b in zip(t.pts, t.pts[1:]):
            m = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)  # a mitre point's distance to the centre line is not HALF
            if any(ly == t.layer and abs(Point(m).distance(LineString(c)) - HALF) < 1e-4 for ly, c in sol.coupled):
                tot += math.dist(a, b)
    return tot


def _cost_gnd(pat: DPat, sol: Sol, u: float = UNCOUPLED_PRICE) -> dict:
    """Price of the pairs (mm + 25 per via + U per uncoupled mm) and of the GND ties (stub mm + 25 per via). Uncoupled =
    track length outside the coupled bodies beyond the MINIMAL pad fans (pad half + run-out d0 + 45 degree leg, from the
    reference): the reference has 0 by construction, an obstructed fan counts its extra stretch."""
    mf = pat.extra["min_fan"]
    ln = {n: track_len(sol, n) for n in GPNETS}
    cp = {n: _coupled_len(sol, n) for n in GPNETS}
    unc = {n: max(0.0, ln[n] - cp[n] - mf[n]) for n in GPNETS}
    gv = sum(v.net == GND for v in sol.vias)
    pv = len(sol.vias) - gv
    stub = track_len(sol, GND)
    pair_cost = sum(ln.values()) + VIA_PRICE * pv + u * sum(unc.values())
    gnd_cost = stub + VIA_PRICE * gv
    return {"pair_length_sum": r4(sum(ln.values())), "pair_vias": pv, "uncoupled_pairs": r4(sum(unc.values())),
            "pair_cost": r4(pair_cost), "gnd_vias": gv, "gnd_stub_mm": r4(stub), "gnd_cost": r4(gnd_cost),
            "vias": len(sol.vias), "length_sum": r4(sum(ln.values()) + stub), "cost": r4(pair_cost + gnd_cost),
            "pair_lengths": {f"TX{k}": [r4(ln[f"TX{k}_P"]), r4(ln[f"TX{k}_N"])] for k in range(2)},
            "pair_skew_max": r4(max(abs(ln[f"TX{k}_P"] - ln[f"TX{k}_N"]) for k in range(2))),
            "coupled": {n: r4(cp[n]) for n in GPNETS}, "uncoupled": {n: r4(unc[n]) for n in GPNETS}}


def _pair_south(fps, Pj, Nj, Lj, Tj, Pu, Nu, Lu, Tu, *, sj=0.0, su=0.0, clear=CLEAR):
    """A pair from a top row (pads exit south) to a bottom row (pads exit north): KiCad-style fans at both ends
    (exit_fan), straight coupled body. The tracks start `sj` / `su` mm east of the pad centres (off-centre pad entry,
    0 = centre); a remaining x offset between the gateways becomes ONE 45 degree jog in the middle of the body (an S:
    the two bends' skews cancel). Returns (P pts, N pts, C, fans, (d0 top, d0 bottom), G1, G2)."""

    def shifted(pos_p, pos_n, s):
        """fps with the own P / N pads moved by s (exit_fan skips the pad at the start point as 'own')."""
        sp, sn = (r4(pos_p[0] + s), pos_p[1]), (r4(pos_n[0] + s), pos_n[1])
        out = []
        for f in fps:
            pads = [Pad(q.num, q.net, sp if q.pos == pos_p else sn if q.pos == pos_n else q.pos, q.size, q.shape, q.drill,
                        q.layer) for q in f.pads]
            out.append(Fp(f.ref, pads, f.role))
        return out, sp, sn

    fj, Pj2, Nj2 = shifted(Pj, Nj, sj)
    fu, Pu2, Nu2 = shifted(Pu, Nu, su)
    dj, G1, pj, nj = exit_fan(fj, Pj2, Nj2, (0, 1), Lj, Tj, clear=clear)
    du, G2, pu, nu = exit_fan(fu, Pu2, Nu2, (0, -1), Lu, Tu, clear=clear)
    dx = G2[0] - G1[0]
    if abs(dx) < 1e-9:
        C = [G1, G2]
    else:
        ym = r4((G1[1] + G2[1]) / 2 - abs(dx) / 2)
        C = [G1, (G1[0], ym), (G2[0], r4(ym + abs(dx))), G2]
    bp, bn = body_pair(C, -1)  # heading south, P (west) on the right
    P = pj + bp[1:] + pu[::-1][1:]
    N = nj + bn[1:] + nu[::-1][1:]
    return P, N, C, [pj, nj, pu[::-1], nu[::-1]], (dj, du), G1, G2


def _via_beyond(dia: float, clear: float) -> float:
    """Via centre beyond a GND pad's end so that the via clears the P and N pad corners (0.35 across at 0.5 pitch):
    sqrt(0.35^2 + d^2) >= dia / 2 + clear, on the 0.05 grid."""
    return _grid_up(math.sqrt(max(0.0, (dia / 2 + clear) ** 2 - (HP - JW / 2) ** 2)))


def _gnd_tie(pad: Pad, h, *, behind: bool, dia=VIA_D, drill=VIA_DRILL, d=None):
    """GND stub (pad centre -> via) and via; h = the pad's exit direction (towards the routing area); behind = on the
    other side. d = via centre beyond the pad end (default: the module rule set's GND_VIA_D)."""
    s = -1.0 if behind else 1.0
    d = GND_VIA_D if d is None else d
    v = add(pad.pos, mul(h, s * (pad.size[1] / 2 + d)))
    v = (r4(v[0]), r4(v[1]))
    return Track(GND, "F.Cu", GND_W, [pad.pos, v]), Via(GND, v, dia, drill)


def _gnd_via_fan(P_pos, N_pos, L, via, vr, clear, *, grid=0.05):
    """Fan of a pair leaving a flow-through P / GND / N row (pads exit south, the tracks run on from the pad centres)
    past a GND via sitting between P and N: if the via does not clear the straight tracks (centre distance < via r +
    clearance + half width), both tracks step out 45 degrees as LATE as possible to the needed distance (graded: the
    smaller the via, the less widening); past the via, 45 degree legs converge to the pair pitch as soon as legs and
    body clear it. Returns (P pts pad->G, N pts pad->G, G, info)."""
    xg = (P_pos[0] + N_pos[0]) / 2
    yend = P_pos[1] + L / 2
    half0 = (N_pos[0] - P_pos[0]) / 2
    need = vr + clear + DP_W / 2
    out = _grid_up(max(0.0, need - half0), 0.025)
    wide = half0 + out
    leg = wide - HALF
    clr = lambda seg: min(LineString(sd_seg).distance(Point(via)) for sd_seg in seg)  # noqa: E731
    y_st = yend
    if out > 0:  # latest step-out start: the straight run at half0 and the 45 degree step must both clear the via
        y_st = r4(math.floor(via[1] / grid) * grid)
        while y_st > yend + 1e-9:
            segs = [[(xg + sd * half0, yend), (xg + sd * half0, y_st)] for sd in (-1, 1)] + \
                   [[(xg + sd * half0, y_st), (xg + sd * wide, y_st + out)] for sd in (-1, 1)]
            if clr(segs) >= need - 1e-9:
                break
            y_st = r4(y_st - grid)
        y_st = max(y_st, yend)
    ys = r4(y_st + out)
    while True:
        legs_ok = clr([[(xg + sd * wide, ys), (xg + sd * HALF, ys + leg)] for sd in (-1, 1)]) >= need - 1e-9
        body_ok = ys + leg >= via[1] and math.hypot(HALF, ys + leg - via[1]) >= need - 1e-9
        if legs_ok and body_ok:
            break
        ys = r4(ys + grid)
    G = (r4(xg), r4(ys + leg))
    pts = []
    for sd, pos in ((-1, P_pos), (1, N_pos)):
        q = [pos, (pos[0], y_st)] + ([(r4(xg + sd * wide), r4(y_st + out))] if out > 0 else []) + \
            [(r4(xg + sd * wide), ys), (r4(xg + sd * HALF), r4(ys + leg))]
        pts.append(clean(q))
    return pts[0], pts[1], G, {"step_out": out, "step_at_past_pad_end": r4(y_st - yend),
                               "converge_end_past_pad_end": r4(ys + leg - yend), "need": r4(need)}


def _split_fan(fps, P_pos, N_pos, L, via, *, out=0.025, grid=0.05):
    """Fan of a pair whose GND via sits IN FRONT of the GND pad (between P and N): both tracks step OUT by `out` (45
    degrees) right at the pad end so that they clear the via (track centre 0.5 + out >= via r + clearance + half
    width = 0.525 from the via axis), run straight past it and converge with 45 degree legs as soon as the legs clear
    the via. Pads exit south. Returns (P pts pad->G, N pts pad->G, G, straight run past the pad end)."""
    xg = (P_pos[0] + N_pos[0]) / 2
    yend = P_pos[1] + L / 2
    need = VIA_R + CLEAR + DP_W / 2
    half0 = (N_pos[0] - P_pos[0]) / 2  # 0.5
    wide = half0 + out
    leg = wide - HALF
    assert wide >= need - 1e-9  # the straight run past the via clears it
    ys = r4(yend + out)
    while ys < yend + 5:
        legs_ok = all(LineString([(xg + sd * wide, ys), (xg + sd * HALF, ys + leg)]).distance(Point(via)) >= need - 1e-9
                      for sd in (-1, 1))
        # the coupled body from y = ys + leg southwards runs HALF from the axis: it must start past the via
        body_ok = ys + leg >= via[1] and math.hypot(HALF, ys + leg - via[1]) >= need - 1e-9
        if legs_ok and body_ok:
            break
        ys = r4(ys + grid)
    G = (r4(xg), r4(ys + leg))
    pts = []
    for sd, pos in ((-1, P_pos), (1, N_pos)):
        pts.append([pos, (pos[0], yend), (r4(pos[0] + sd * out), r4(yend + out)), (r4(xg + sd * wide), r4(ys)),
                    (r4(xg + sd * HALF), r4(ys + leg))])
    return pts[0], pts[1], G, r4(ys - yend)


def _gnd_pat_common(name, title, W, H, fps, ref, alt, gw, extra, *, gnd_pads, conns=None, classes=None, patterns=None,
                    rules=None):
    nets = GPNETS + [GND]
    if conns is None:
        conns = []
        for k in range(2):
            for s, i in (("P", 0), ("N", 1)):
                net = f"TX{k}_{s}"
                a = next(f"J1.{p.num}" for p in fps[0].pads if p.net == net)
                b = next(f"U1.{p.num}" for p in fps[1].pads if p.net == net)
                conns.append((net, a, b))
    conns = conns + [(GND, g, "B.Cu pour") for g in gnd_pads]
    return DPat(name=name, title=title, kind="solvable", purpose="", answer="", why="", size=(W, H), fps=fps, ref=ref,
                alt=alt, careless=None, expected={}, gateways=gw, connections=conns, nets=nets,
                classes=classes or [dict(CLASS_DEFAULT), dict(BUS_CLASS)],
                net_patterns=patterns or [{"netclass": "HDMI_TX", "pattern": "TX*"}], rules=rules or dict(RULES_DP),
                zones=[{"net": GND, "layer": "B.Cu", "rect": (0.25, 0.25, W - 0.25, H - 0.25)}], extra=extra)


def p_gnd_between() -> DPat:
    """Connector row J1 (P GND N P GND N, 0.5 pitch, exit south) -> transmitter QFP row U1 (P N x P N, exit north)."""
    W, H = 7.5, 13.0
    yj, yu = 4.6, 10.6
    j1 = _pgn_fp("J1", 2.5, yj, JL, PGN)
    u1 = _pgn_fp("U1", 2.75, yu, UQL, ("TX0_P", "TX0_N", None, "TX1_P", "TX1_N"))
    fps = [j1, u1]
    paths, coupled, fans, gw, d0s, Cs, min_fan = {}, [], [], [], [], [], {}
    for k in range(2):
        P, N = f"TX{k}_P", f"TX{k}_N"
        Pp, Np, C, fn, d0, G1, G2 = _pair_south(fps, _pad(j1, P).pos, _pad(j1, N).pos, JL, 2 * HP,
                                                _pad(u1, P).pos, _pad(u1, N).pos, UQL, HP)
        paths[P], paths[N] = [("F.Cu", Pp, DP_W)], [("F.Cu", Np, DP_W)]
        coupled.append(("F.Cu", C))
        fans += [("F.Cu", f) for f in fn]
        gw += [{"id": f"G{2 * k + 1}", "pt": G1, "dir": (0, 1)}, {"id": f"G{2 * k + 2}", "pt": G2, "dir": (0, -1)}]
        d0s.append(d0)
        Cs.append(C)
        min_fan[P] = plen(fn[0]) + plen(fn[2])
        min_fan[N] = plen(fn[1]) + plen(fn[3])
    ref = sol_from("reference: GND vias BEHIND the GND pads, the pairs converge right past them", paths, coupled, fans)
    gpads = [p for p in j1.pads if p.net == GND]
    for g in gpads:
        t, v = _gnd_tie(g, (0, 1), behind=True)
        ref.tracks.append(t)
        ref.vias.append(v)
    # careless (DRC-clean): GND via IN FRONT of each GND pad, between P and N; the pair splits around it
    paths2, coupled2, fans2, runs = {}, [], [], []
    alt_vias, alt_stubs = [], []
    for k, g in enumerate(gpads):
        t, v = _gnd_tie(g, (0, 1), behind=False)
        alt_stubs.append(t)
        alt_vias.append(v)
        P, N = f"TX{k}_P", f"TX{k}_N"
        pj, nj, Gs, run = _split_fan(fps, _pad(j1, P).pos, _pad(j1, N).pos, JL, v.pos)
        runs.append(run)
        C2 = [Gs, Cs[k][-1]]
        bp, bn = body_pair(C2, -1)
        fu = [f for ly, f in fans if math.dist(f[0], bp[-1]) < 1e-6 or math.dist(f[0], bn[-1]) < 1e-6]
        fpu = next(f for f in fu if math.dist(f[0], bp[-1]) < 1e-6)
        fnu = next(f for f in fu if math.dist(f[0], bn[-1]) < 1e-6)
        paths2[P] = [("F.Cu", pj + bp[1:] + fpu[1:], DP_W)]
        paths2[N] = [("F.Cu", nj + bn[1:] + fnu[1:], DP_W)]
        coupled2.append(("F.Cu", C2))
        fans2 += [("F.Cu", f) for f in (pj, nj, fpu, fnu)]
    alt = sol_from("GND via IN FRONT of each GND pad (between P and N), the pair splits around it", paths2,
                   coupled2, fans2)
    alt.tracks += alt_stubs
    alt.vias += alt_vias
    alt.marks = [{"pt": alt_vias[0].pos, "label": "GND via between P and N"}]
    return _gnd_pat_common(
        "dp_gnd_between", "HDMI connector row P / GND / N: the pairs converge past the GND pad, its via goes behind it",
        W, H, fps, ref, alt, gw, {"d0": d0s, "C": Cs, "min_fan": min_fan, "runs_careless": runs, "yj": yj, "yu": yu,
                                  "alt_note": "the pairs converge only past the GND vias"},
        gnd_pads=[f"J1.{p.num}" for p in gpads])


def _row(fp, net, y):
    return next(p for p in fp.pads if p.net == net and abs(p.pos[1] - y) < 1e-9)


def _esd_route(fps, j1, u2, u1, yt, yb, vias_mid, vr):
    """The pairs J1 -> U2 top row -> (through) -> U2 bottom row -> (through) -> U1. vias_mid = {(k, 'top'|'bot'): via
    position} of the GND vias between P and N on the inner side of each ESD row, radius vr. Returns (paths, coupled,
    fans, min_fan, gateways, info)."""
    paths, coupled, fans, min_fan, gw, info = {}, [], [], {}, [], {}
    yJ = j1.pads[0].pos[1]
    mir = lambda q: (q[0], r4(2 * yb - q[1]))  # noqa: E731  (bottom row seen as a row exiting south)
    for k in range(2):
        P, N = f"TX{k}_P", f"TX{k}_N"
        pt, nt = _row(u2, P, yt).pos, _row(u2, N, yt).pos
        pb, nb = _row(u2, P, yb).pos, _row(u2, N, yb).pos
        # J1 -> U2 top: KiCad fans, straight body; J1 tracks enter 0.05 east of the pad centres (the row offset)
        A_P, A_N, C1, fnA, d0A, G1, G2 = _pair_south(fps, _row(j1, P, yJ).pos, _row(j1, N, yJ).pos, JL, 2 * HP, pt, nt,
                                                     UTL, 2 * HP, sj=ESD_DX, clear=QM_CLEAR)
        # through the top row, past the GND via between P and N, converge
        tP, tN, G3, it = _gnd_via_fan(pt, nt, UTL, vias_mid[(k, "top")], vr, QM_CLEAR)
        # diverge before the bottom row's via (mirror of the same fan), into and through the bottom row
        bPm, bNm, G4m, ib = _gnd_via_fan(mir(pb), mir(nb), UTL, mir(vias_mid[(k, "bot")]), vr, QM_CLEAR)
        bP, bN, G4 = [mir(q) for q in bPm][::-1], [mir(q) for q in bNm][::-1], mir(G4m)  # G -> pad
        C2 = [G3, G4]
        b2p, b2n = body_pair(C2, -1)
        # U2 bottom -> U1 (QFP, P N adjacent)
        D_P, D_N, C3, fnD, d0D, G5, G6 = _pair_south(fps, pb, nb, UTL, 2 * HP, _row(u1, P, u1.pads[0].pos[1]).pos,
                                                     _row(u1, N, u1.pads[0].pos[1]).pos, UQL, HP, clear=QM_CLEAR)
        paths[P] = [("F.Cu", A_P + tP[1:] + b2p[1:] + bP[1:] + D_P[1:], DP_W)]
        paths[N] = [("F.Cu", A_N + tN[1:] + b2n[1:] + bN[1:] + D_N[1:], DP_W)]
        coupled += [("F.Cu", C1), ("F.Cu", C2), ("F.Cu", C3)]
        fl = fnA + [tP, tN, bP, bN] + fnD
        fans += [("F.Cu", f) for f in fl]
        min_fan[P] = sum(plen(f) for f in (fnA[0], fnA[2], tP, bP, fnD[0], fnD[2]))
        min_fan[N] = sum(plen(f) for f in (fnA[1], fnA[3], tN, bN, fnD[1], fnD[3]))
        gw += [{"id": f"G{6 * k + i + 1}", "pt": g, "dir": (0, 1)} for i, g in enumerate((G1, G2, G3, G4, G5, G6))]
        info[k] = {"d0_J1_U2top": d0A, "d0_U2bot_U1": d0D, "top_via_fan": it, "bottom_via_fan": ib,
                   "body_between_rows": r4(G4[1] - G3[1]), "C1": r4(G2[1] - G1[1]), "C3": r4(G6[1] - G5[1])}
    return paths, coupled, fans, min_fan, gw, info


def p_esd_flowthrough() -> DPat:
    """qmtech's HDMI chain: connector row J1 (P GND N, exit south) -> ESD chip U2 (TPD12S520-like, two rows, flow-through:
    each signal has a pad in both rows and the track runs straight through both) -> transmitter U1 (QFP, P N adjacent).
    qmtech Board Setup constraints and clearances."""
    W, H = 7.5, 21.5
    yj = 3.6
    yt = r4(yj + J1_TO_ESD)
    yb = r4(yt + ESD_ROWS)
    yq = r4(yb + ESD_TO_TX)
    j1 = _pgn_fp("J1", 2.5, yj, JL, PGN)
    top = _pgn_fp("U2", r4(2.5 + ESD_DX), yt, UTL, PGN)
    bot = _pgn_fp("U2", r4(2.5 + ESD_DX), yb, UTL, PGN)
    u2 = Fp("U2", [Pad(str(i + 1), q.net, q.pos, q.size) for i, q in enumerate(bot.pads + top.pads)], "terminal")
    xg0 = r4(2.5 + ESD_DX + HP)  # GND axis of pair 0 in U2
    u1 = _pgn_fp("U1", r4(xg0 - HP / 2), yq, UQL, ("TX0_P", "TX0_N", None, "TX1_P", "TX1_N"))
    fps = [j1, u2, u1]
    d_mid = _via_beyond(QM_VIA_D, QM_CLEAR)
    gtop = [p for p in u2.pads if p.net == GND and abs(p.pos[1] - yt) < 1e-9]
    gbot = [p for p in u2.pads if p.net == GND and abs(p.pos[1] - yb) < 1e-9]
    gj = [p for p in j1.pads if p.net == GND]
    ties_top = [_gnd_tie(g, (0, 1), behind=False, dia=QM_VIA_D, drill=QM_DRILL, d=d_mid) for g in gtop]
    ties_bot = [_gnd_tie(g, (0, -1), behind=False, dia=QM_VIA_D, drill=QM_DRILL, d=d_mid) for g in gbot]
    ties_j = [_gnd_tie(g, (0, 1), behind=True, dia=QM_VIA_D, drill=QM_DRILL, d=d_mid) for g in gj]
    vm = {(k, "top"): ties_top[k][1].pos for k in range(2)} | {(k, "bot"): ties_bot[k][1].pos for k in range(2)}
    paths, coupled, fans, min_fan, gw, info = _esd_route(fps, j1, u2, u1, yt, yb, vm, QM_VIA_D / 2)
    ref = sol_from(f"reference: GND vias {QM_VIA_D}/{QM_DRILL} (smallest allowed) between P and N in the ESD rows, "
                   "behind the connector pads", paths, coupled, fans)
    for t, v in ties_j + ties_top + ties_bot:
        ref.tracks.append(t)
        ref.vias.append(v)
    # careless: the GND class via 0.8 / 0.4 between P and N; P and N must step out around it, and the two pairs' vias
    # must be staggered so that neighbouring bulges (TX0_N, TX1_P) do not meet: pair 1's via later in the top row,
    # pair 0's later in the bottom row (each pair loses coupled length in one row)
    dc = _via_beyond(QM_CLASS_VIA[0], QM_CLEAR)
    stagger = 0.0
    while True:
        ct = [_gnd_tie(g, (0, 1), behind=False, dia=QM_CLASS_VIA[0], drill=QM_CLASS_VIA[1], d=dc + (stagger if k else 0))
              for k, g in enumerate(gtop)]
        cb = [_gnd_tie(g, (0, -1), behind=False, dia=QM_CLASS_VIA[0], drill=QM_CLASS_VIA[1], d=dc + (0 if k else stagger))
              for k, g in enumerate(gbot)]
        vmc = {(k, "top"): ct[k][1].pos for k in range(2)} | {(k, "bot"): cb[k][1].pos for k in range(2)}
        p2, c2, f2, _, _, info2 = _esd_route(fps, j1, u2, u1, yt, yb, vmc, QM_CLASS_VIA[0] / 2)
        alt = sol_from(f"GND net-class via {QM_CLASS_VIA[0]}/{QM_CLASS_VIA[1]} between P and N: the pair steps "
                       "out around it, the two pairs' vias staggered", p2, c2, f2)
        for t, v in ties_j + ct + cb:
            alt.tracks.append(t)
            alt.vias.append(v)
        probe = DPat("probe", "", "", "", "", "", (W, H), fps, alt, None, None, {}, extra={"clear": QM_CLEAR})
        if clearance_report(probe, alt)["margin"] >= -1e-6:
            break
        stagger = r4(stagger + 0.05)
        assert stagger < 3
    alt.marks = [{"pt": ct[0][1].pos, "label": f"class via {QM_CLASS_VIA[0]}"}]
    conns = []
    for k in range(2):
        for net in (f"TX{k}_P", f"TX{k}_N"):
            a = f"J1.{_row(j1, net, yj).num}"
            b, c = f"U2.{_row(u2, net, yt).num}", f"U2.{_row(u2, net, yb).num}"
            d = f"U1.{_row(u1, net, yq).num}"
            conns += [(net, a, b), (net, b, c), (net, c, d)]
    gpads = [f"J1.{p.num}" for p in gj] + [f"U2.{p.num}" for p in gtop + gbot]
    body = (1.75, r4((yt + yb) / 2 - 2.2), 5.85, r4((yt + yb) / 2 + 2.2))
    return _gnd_pat_common(
        "dp_esd_flowthrough", "Flow-through ESD pads: the GND via between P and N, the smallest the constraints allow",
        W, H, fps, ref, alt, gw,
        {"yj": yj, "yt": yt, "yb": yb, "yq": yq, "min_fan": min_fan, "info": info, "info_careless": info2,
         "stagger": stagger, "d_mid": d_mid, "d_class": dc, "clear": QM_CLEAR,
         "ref_via": (QM_VIA_D, QM_DRILL), "via_sizes": {(QM_VIA_D, QM_DRILL), QM_CLASS_VIA}, "behind_refs": ["J1"],
         "middle_refs": ["U2"],
         "outlines": [{"rect": body, "label": "U2 body (ESD, flow-through)"}],
         "alt_note": "class vias force the pairs apart"},
        gnd_pads=gpads, conns=conns, classes=[dict(c) for c in QM_CLASSES],
        patterns=[{"netclass": "HDMI_TX", "pattern": "TX*"}, {"netclass": "GND", "pattern": "GND"}],
        rules=dict(RULES_DP, **QM_RULES))


def check_gnd(pat: DPat) -> dict:
    ref, alt = pat.ref, pat.alt
    for sol in (ref, alt):
        assert all(t.width == DP_W for t in sol.tracks if t.net in GPNETS)
        assert all(t.width == GND_W and t.layer == "F.Cu" for t in sol.tracks if t.net == GND)
        sizes = {pat.extra.get("ref_via", (VIA_D, VIA_DRILL))} if sol is ref else \
            set(pat.extra.get("via_sizes", {(VIA_D, VIA_DRILL)}))
        assert all((v.dia, v.drill) in sizes for v in sol.vias), (sol.label, {(v.dia, v.drill) for v in sol.vias})
        assert not any(v.net in GPNETS for v in sol.vias)
        for n in GPNETS:
            assert connected(pat, sol, n), (sol.label, n)
        for f in pat.fps:  # every GND pad has its own stub to its own via (the pour joins the vias)
            for p in f.pads:
                if p.net == GND:
                    st = [t for t in sol.tracks if t.net == GND and math.dist(t.pts[0], p.pos) < 1e-6]
                    assert len(st) == 1 and any(math.dist(st[0].pts[-1], v.pos) < 1e-6 for v in sol.vias)
        rep = clearance_report(pat, sol)
        assert rep["margin"] >= -1e-6 and rep["edge_margin"] >= -1e-6, (pat.name, sol.label, rep)
    # reference: constant gap on every coupled body, no skew, no uncoupled stretch beyond the minimal fans
    for k in range(2):
        P, N = f"TX{k}_P", f"TX{k}_N"
        nn = unary_union([LineString(t.pts).buffer(DP_W / 2) for t in ref.tracks if t.net == N])
        n_body = 0
        for t in ref.tracks:
            if t.net == P:
                for a, b in zip(t.pts, t.pts[1:]):
                    m = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
                    if any(abs(Point(m).distance(LineString(c)) - HALF) < 1e-4 for _, c in ref.coupled):
                        assert _close(Point(m).distance(nn) - DP_W / 2, DP_GAP, 1e-3)
                        n_body += 1
        assert n_body >= 1
    c = _cost_gnd(pat, ref)
    assert c["uncoupled_pairs"] < 1e-6 and c["pair_skew_max"] < 1e-6, c
    # reference GND vias: behind the pad (away from the pad's exit direction) where the row has room behind
    # (pat.extra["behind_refs"], default all), else between the P and N tracks on the routing side (flow-through rows)
    behind_refs = pat.extra.get("behind_refs", [f.ref for f in pat.fps])
    for f in pat.fps:
        for p in f.pads:
            if p.net != GND:
                continue
            v = min((v for v in ref.vias if abs(v.pos[0] - p.pos[0]) < 1e-6), key=lambda v: math.dist(v.pos, p.pos))
            if f.ref in behind_refs:
                h = (0, 1) if f.ref == "J1" else (0, -1)
                assert (v.pos[1] - p.pos[1]) * h[1] < 0, (f.ref, p.num, v.pos)
            else:
                cut = LineString([(v.pos[0] - 1.5, v.pos[1]), (v.pos[0] + 1.5, v.pos[1])])
                xs = {}
                for t in ref.tracks:
                    if t.net in GPNETS and LineString(t.pts).intersects(cut):
                        xs.setdefault(t.net, []).append(LineString(t.pts).intersection(cut).centroid.x)
                west = [n for n, x in xs.items() if all(q < v.pos[0] for q in x) and n.endswith("_P")]
                east = [n for n, x in xs.items() if all(q > v.pos[0] for q in x) and n.endswith("_N")]
                assert any(w[:-2] == e[:-2] for w in west for e in east), (f.ref, p.num, v.pos, xs)
    assert _cost_gnd(pat, alt)["cost"] > c["cost"]
    return {"ref": c}


def hand_gnd(pat: DPat) -> dict:
    """Hand derivation of dp_gnd_between from the rule values (dp_esd_flowthrough: hand_esd)."""
    leg_pgn = (2 * HP - PITCH) / 2  # 0.325: pads 1.0 apart (GND between) -> pitch 0.35
    leg_q = (HP - PITCH) / 2  # 0.075: QFP pads 0.5 apart
    # run-out at a P GND N row: the P leg (y = x + d0 from the pad end) against the GND pad corner (0.35, 0):
    # (0.35 + d0) / sqrt2 >= 0.1 + 0.2 -> d0 >= 0.0743 -> 0.10
    d0_pgn_min = (DP_W / 2 + CLEAR) * SQ2 - (HP - JW / 2)
    # QFP row (P N adjacent): sqrt((d0 + 0.075)^2 + 0.275^2) >= 0.3 -> d0 >= 0.0448 -> 0.05 (as dp_gateway J1)
    d0_q_min = math.sqrt((DP_W / 2 + CLEAR) ** 2 - (HP - JW / 2 - leg_q) ** 2) - leg_q
    d0_pgn, d0_q = _grid_up(d0_pgn_min), _grid_up(d0_q_min)
    fan_j = JL / 2 + d0_pgn + leg_pgn * SQ2
    out = {"leg_pgn": leg_pgn, "leg_qfp": leg_q, "d0_pgn_min": d0_pgn_min, "d0_qfp_min": d0_q_min, "d0_pgn": d0_pgn,
           "d0_qfp": d0_q, "gnd_via_beyond_pad_end": GND_VIA_D,
           "gnd_via_beyond_min": math.sqrt((VIA_R + CLEAR) ** 2 - (HP - JW / 2) ** 2),
           "via_in_pad_margin": HP - JW / 2 - VIA_R - CLEAR,
           "via_between_tracks_needs_pitch": 2 * (VIA_R + CLEAR + DP_W / 2)}
    yj, yu = pat.extra["yj"], pat.extra["yu"]
    assert pat.name == "dp_gnd_between"
    if True:
        fan_u = UQL / 2 + d0_q + leg_q * SQ2
        g1 = yj + JL / 2 + d0_pgn + leg_pgn
        g2 = yu - UQL / 2 - d0_q - leg_q
        out.update(body=g2 - g1, track=fan_j + (g2 - g1) + fan_u, stub=JL / 2 + GND_VIA_D, gnd_vias=2,
                   G_top_y=g1, G_bottom_y=g2)
    return out


QMTECH_HDMI = {
    "board": "scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb (hand routed, read on a copy)",
    "chain": "ADV7511 (LQFP-100, 0.5 pitch, pairs P N adjacent, pair pitch 1.5) -> U3 TPD12S520DBT (ESD, TSSOP-38, "
             "flow-through: each signal has a pin in both rows, the track runs over both pads) -> USB1 HDMI-001S "
             "connector (lcsc, SMD)",
    "connector": "USB1 is ONE row of 19 SMD pads, 0.3 x 2.6, pitch 0.5, at y 55.1 (rotated 180): 1 TX2+, 2 GND, 3 TX2-, "
                 "4 TX1+, 5 GND, 6 TX1-, 7 TX0+, 8 GND, 9 TX0-, 10 TXC+, 11 GND, 12 TXC-; plus 4 THT shell pads. It is "
                 "NOT a two-row staggered footprint.",
    "esd_chip": "U3 two rows 5.726 apart (y 59.237 and 64.963), pads 0.3 x 1.475, pitch 0.5, rows ALIGNED in x; each row "
                "P / GND / N per pair at the same x as the other row (pins 4/35 TX2+, 5/34 GND, 6/33 TX2- ...)",
    "row_offset": "connector row x = U3 row x + 0.027 (130.227 vs 130.2): the only 'slight misalignment'; the hand layout "
                  "absorbs it by starting the tracks 0.027 off-centre in the connector pads (track x 130.2 in a pad at "
                  "130.227), no jog",
    "row_gap": "connector row to U3 top row 4.137 (pad ends 56.4 -> 58.4995: 2.10 free)",
    "pairs_hand": "width 0.2, pitch 0.45 (gap 0.25 = Default class gap) when coupled; connector: straight out of the pad "
                  "centre to 0.61 past the pad end (57.01), 45 degree legs 0.275 to the pair axis = the GND pad's axis "
                  "(57.285), coupled 0.43 (to 57.715), diverge to 1.1 pitch (130.15 / 131.25, 0.05 OUTSIDE the U3 pad "
                  "centres) to pass the U3 GND via, into U3; between the U3 rows coupled 2.8 mm (60.696 .. 63.5)",
    "gnd_ties": "connector GND pads: ONE via each, 0.56 / 0.3, 0.30 BEHIND the pad's back end (e.g. pad 2: via (130.71, "
                "53.497), pad end 53.8), stub 0.2 along the pad; U3 GND pads: TWO vias each, 0.6 / 0.3 at 0.23 outside "
                "and 0.56 / 0.3 at 0.20 inside the row, both BETWEEN the P and N tracks (flow-through leaves no 'behind')",
    "planes": "4 layers; GND pours on F.Cu, In1.Cu and B.Cu (whole board), 3V3 on In2.Cu; the pairs run over In1 GND",
    "classes": "HDMI_TX: track 0.2, clearance 0.127, dp 0.16 / 0.2 / 0.25, via 0.45 / 0.3; GND class: track 0.25, "
               "clearance 0.15, via 0.8 / 0.4 (the hand GND vias are 0.56-0.6 / 0.3); board min clearance 0.09",
}


def finish_gnd_between(pat: DPat) -> None:
    h = hand_gnd(pat)
    ref, alt = pat.ref, pat.alt
    chk = check_gnd(pat)
    cr, ca = chk["ref"], _cost_gnd(pat, alt)
    assert [list(d) for d in pat.extra["d0"]] == [[h["d0_pgn"], h["d0_qfp"]]] * 2, (pat.extra["d0"], h)
    assert _close(h["d0_pgn_min"], 0.0743, 5e-4) and GND_VIA_D == 0.25 and h["via_in_pad_margin"] < 0
    for n in GPNETS:
        assert _close(track_len(ref, n), h["track"], 0.005), (n, track_len(ref, n), h["track"])
    assert _close(cr["gnd_stub_mm"], 2 * h["stub"], 1e-4)
    run = pat.extra["runs_careless"][0]
    assert ca["pair_length_sum"] >= cr["pair_length_sum"] and ca["uncoupled_pairs"] > cr["uncoupled_pairs"]
    u_star = "none: the careless layout is longer AND less coupled, dearer at every U >= 0"
    pat.purpose = ("HDMI connector row (qmtech USB1): P / GND / N per pair at 0.5 pitch, two pairs, to the transmitter "
                   "(P N adjacent). Each pair must leave P and N past the GND pad between them and converge to the pair "
                   "gap (gateway as in dp_gateway); each GND pad must reach GND (B.Cu pour) through a via. Question: where "
                   "does the GND via go so that it does not split or delay the pair?")
    pat.answer = (f"GND vias BEHIND the GND pads (on the side away from the pair exit; the 'room behind' case of the "
                  f"graded rule: no room behind, as in a flow-through ESD row, puts the smallest allowed via between P "
                  f"and N, see dp_esd_flowthrough): via 0.45/0.25 centred "
                  f"{GND_VIA_D:.2f} beyond the pad's back end, stub 0.25 from the pad centre ({h['stub']:.2f} mm); 2 GND "
                  f"vias. Pairs on F.Cu, 0 vias: straight out of the P and N pad centres, run-out {h['d0_pgn']:.2f} past "
                  f"the pad end (the 45 degree legs must clear the GND pad corner), legs {h['leg_pgn']:.3f} sideways to the "
                  f"gap 0.15 on the GND pad's axis = gateway (G1, G3 at y {h['G_top_y']:.3f}); straight coupled body "
                  f"{h['body']:.3f} mm to the QFP fan (run-out {h['d0_qfp']:.2f}, legs {h['leg_qfp']:.3f}; G2, G4). Per track "
                  f"{h['track']:.3f} mm, skew 0, uncoupled 0 beyond the fans. Price {cr['cost']:.1f} = pairs "
                  f"{cr['pair_length_sum']:.2f} mm + GND {cr['gnd_stub_mm']:.2f} mm + 2 x 25. Careless (GND via in front, "
                  f"between P and N): {ca['cost']:.1f}, +{ca['cost'] - cr['cost']:.2f}.")
    pat.why = (f"A via cannot sit in the GND pad (0.45 via in a 0.3 pad at 0.5 pitch: {h['via_in_pad_margin']:+.3f} mm to the "
               f"P pad at clearance 0.2) nor beside it, so it goes in front of or behind the pad, {GND_VIA_D:.2f} beyond the "
               f"end (sqrt(0.35^2 + d^2) >= 0.425 -> d >= {h['gnd_via_beyond_min']:.3f}). Both stubs are {h['stub']:.2f} long "
               f"and both cost one via, so the GND tie alone does not choose; the pair does. In front, the via sits on "
               f"the pair's axis exactly where the pair must converge: P and N must stay apart past it (track centres "
               f"{h['via_between_tracks_needs_pitch']:.2f} apart > the pad pitch 1.0: both step out 0.025) and converge "
               f"{run:.2f} past the pad end instead of {h['d0_pgn']:.2f}, i.e. {ca['uncoupled_pairs']:.2f} mm uncoupled "
               f"over 4 tracks (KiCad's DRC also drops coupling where copper lies between P and N). Behind, the via costs "
               f"the pair nothing. The pair is the wider item (ordering width 0.95 vs the GND stub 0.25 + 2 x 0.2 = 0.65), "
               f"so it is placed first and the via takes the free side; qmtech's connector does the same.")
    pat.expected = {
        "interpretation": {
            "needs_user_confirmation": True,
            "built": "2-layer board, GND = a B.Cu pour (terminal), one via per GND pad behind the pad; two HDMI pairs "
                     "like qmtech USB1 pins 1-6; pair values are the module's (0.20 / 0.15 / 0.25)",
            "open_question": "GND model: a B.Cu pour on a 2-layer board (built) or a 4-layer variant like qmtech (In1 GND "
                             "plane; the via then lands on In1)? And: one via per GND pad (built, as qmtech) or may two "
                             "GND pads share one via (cheaper by the price model: 25 vs a 1.5 mm stub)?"},
        "rules": _rules_block(),
        "gnd": {"model": "B.Cu pour over the whole board, net GND (copper-to-edge 0.5); a terminal: vias on it always "
                         "allowed", "stub_width": GND_W, "via": [VIA_D, VIA_DRILL],
                "via_beyond_pad_end": GND_VIA_D, "via_beyond_min": r4(h["gnd_via_beyond_min"]),
                "via_in_pad_margin": r4(h["via_in_pad_margin"]), "side": "behind (away from the pair exit)"},
        "qmtech_grounding": QMTECH_HDMI,
        "polarity": {"swaps_needed": 0, "turn_backs": 0},
        "vias": len(ref.vias), "pair_vias": 0, "gnd_vias": 2, "crossings": 0,
        "gateways": [{"id": g["id"], "pt": rp(g["pt"])} for g in pat.gateways],
        "lengths": {"per_track": r4(h["track"]), "body": r4(h["body"]), "gnd_stub": r4(h["stub"]),
                    "pair_skew_max": cr["pair_skew_max"], "run_out_pgn": h["d0_pgn"], "run_out_qfp": h["d0_qfp"]},
        "cost": cr,
        "price_table": {
            "model": f"pairs: mm + {VIA_PRICE:.0f} per via + U x uncoupled mm (beyond the minimal fans), U = "
                     f"{UNCOUPLED_PRICE}; GND: stub mm + {VIA_PRICE:.0f} per via",
            "reference (GND vias behind)": cr, "careless (GND vias in front, between P and N)": ca,
            "break_even_U": u_star},
        "alternative": {"label": alt.label, "cost": ca, "converge_starts_past_pad_end": run,
                        "argument": "the via on the pair axis keeps P and N apart (1.05 pitch) until the legs clear it"},
        "judgement_calls": [
            f"GND via {GND_VIA_D} beyond the pad end on the 0.05 grid (minimum {h['gnd_via_beyond_min']:.3f}); qmtech 0.30.",
            "GND stub in the Default class width 0.25 (pi x 0.25 = 0.785 >= 0.25 for the via); qmtech 0.2.",
            "Careless panel steps P and N out by 0.025 at the pad end (45 degrees) to clear the via; qmtech does the same "
            "at U3 (tracks 0.05 outside the pad centres) with its smaller clearance 0.127."],
        "literature": _common_lit() + [HDMI_LIT, KICAD_COUPLING],
    }


def hand_esd(pat: DPat) -> dict:
    """Hand derivation of dp_esd_flowthrough from qmtech's Board Setup values."""
    c, w2 = QM_CLEAR, DP_W / 2
    leg = (2 * HP - PITCH) / 2  # 0.325: P and N pads 1.0 apart -> pitch 0.35
    leg_q = (HP - PITCH) / 2  # 0.075
    half0 = HP  # straight tracks through the P and N pads: 0.5 either side of the GND axis
    corner = HP - JW / 2  # 0.35: from the GND axis to the near edge of the P / N pad
    # smallest via: drill = the Board Setup through-hole minimum (pi x 0.2 = 0.63 >= the 0.25 GND stub it carries);
    # diameter = max(minimum via diameter 0.45, drill + 2 x minimum annular ring 0.1 = 0.40)
    dia, drill = QM_VIA_D, QM_DRILL
    need = dia / 2 + c + w2  # via centre to a pair track centre
    need_h = drill / 2 + QM_RULES["min_hole_clearance"] + w2  # hole edge to track copper
    d_min = math.sqrt((dia / 2 + c) ** 2 - corner ** 2)  # via clear of the P / N pad corners beyond the GND pad end
    d = _grid_up(d_min)
    # converge past the via: legs from x = 0.5 to 0.175 start a below the via centre; the leg line x + y = 0.5 + a must
    # keep need from the via centre: (0.5 + a) / sqrt2 >= need -> a >= need sqrt2 - 0.5; then the body end clears it
    a_min = need * SQ2 - half0
    ys = _grid_up(d + a_min)  # past the pad end, on the 0.05 grid
    conv = ys + leg
    # run-outs: P G N rows at clearance 0.15: (0.35 + d0) / sqrt2 >= 0.1 + 0.15 -> d0 >= 0.0036 -> 0.05; at J1 the
    # tracks enter 0.05 east of the pad centres (row offset), so P's leg has the GND corner 0.05 closer: 0.0536 -> 0.10
    d0_c = _grid_up((w2 + c) * SQ2 - corner)
    d0_j = _grid_up((w2 + c) * SQ2 - (corner - ESD_DX))
    d0_q = 0.0 if math.hypot(corner - leg_q, 0) >= w2 + c else _grid_up(
        math.sqrt((w2 + c) ** 2 - (corner - leg_q) ** 2) - leg_q)  # QFP: sqrt((d0 + 0.075)^2 + 0.275^2) >= 0.25 -> 0
    yj, yt, yb, yq = (pat.extra[k] for k in ("yj", "yt", "yb", "yq"))
    C1 = (yt - UTL / 2) - (yj + JL / 2) - (d0_j + leg) - (d0_c + leg)
    body = (yb - UTL / 2) - (yt + UTL / 2) - 2 * conv
    C3 = (yq - UQL / 2) - (yb + UTL / 2) - (d0_c + leg) - (d0_q + leg_q)
    via_fan = UTL / 2 + ys + leg * SQ2  # pad centre -> straight through the pad and past the via -> 45 degree leg
    track = ((JL / 2 + d0_j + leg * SQ2) + C1 + (leg * SQ2 + d0_c + UTL / 2) + via_fan + body + via_fan +
             (UTL / 2 + d0_c + leg * SQ2) + C3 + (leg_q * SQ2 + d0_q + UQL / 2))
    # careless: the GND class via 0.8 / 0.4
    cd = QM_CLASS_VIA[0]
    need_c = cd / 2 + c + w2
    out_c = _grid_up(need_c - half0, 0.025)
    d_c_min = math.sqrt((cd / 2 + c) ** 2 - corner ** 2)
    # qmtech's own U3 vias 0.6 / 0.3: need 0.3 + 0.15 + 0.1 = 0.55 -> step-out 0.05 per side = the hand layout's 1.1
    out_qm = _grid_up(0.6 / 2 + c + w2 - half0, 0.025)
    return {"via": [dia, drill], "via_rule": "drill = min through hole; dia = max(min via diameter, drill + 2 annular)",
            "pi_drill": math.pi * drill, "need": need, "fit_margin": half0 - need, "hole_margin": half0 - need_h,
            "need_with_module_clear_0.2": dia / 2 + CLEAR + w2, "via_beyond_min": d_min, "via_beyond": d,
            "converge_a_min": a_min, "converge_legs_start": ys, "converge_end": conv, "d0_pgn": d0_c, "d0_J1": d0_j,
            "d0_qfp": d0_q, "C1": C1, "body_between_rows": body, "C3": C3, "via_fan": via_fan, "track": track,
            "stub_J1": JL / 2 + d, "stub_U2": UTL / 2 + d, "class_via_need": need_c, "class_via_step_out": out_c,
            "class_via_beyond_min": d_c_min, "class_via_beyond": _grid_up(d_c_min), "qmtech_via_step_out": out_qm,
            "pad_ends_gap_rows": (yb - UTL / 2) - (yt + UTL / 2)}


def finish_esd(pat: DPat) -> None:
    h = hand_esd(pat)
    ref, alt = pat.ref, pat.alt
    chk = check_gnd(pat)
    cr, ca = chk["ref"], _cost_gnd(pat, alt)
    assert h["via"] == [0.45, 0.2] and _close(h["fit_margin"], 0.025, 1e-9) and h["via_beyond"] == pat.extra["d_mid"]
    assert h["class_via_beyond"] == pat.extra["d_class"] and h["qmtech_via_step_out"] == 0.05
    for k in range(2):
        i = pat.extra["info"][k]
        assert i["d0_J1_U2top"] == (h["d0_J1"], h["d0_pgn"]) and i["d0_U2bot_U1"] == (h["d0_pgn"], h["d0_qfp"]), i
        for f in ("top_via_fan", "bottom_via_fan"):
            assert i[f]["step_out"] == 0 and _close(i[f]["converge_end_past_pad_end"], h["converge_end"], 1e-9), i[f]
        assert _close(i["body_between_rows"], h["body_between_rows"], 1e-6) and _close(i["C1"], h["C1"], 1e-6)
        assert _close(i["C3"], h["C3"], 1e-6)
        ic = pat.extra["info_careless"][k]
        assert ic["top_via_fan"]["step_out"] == h["class_via_step_out"] == ic["bottom_via_fan"]["step_out"]
    for n in GPNETS:
        assert _close(track_len(ref, n), h["track"], 0.005), (n, track_len(ref, n), h["track"])
    assert _close(cr["gnd_stub_mm"], 2 * h["stub_J1"] + 4 * h["stub_U2"], 1e-4)
    assert ca["pair_length_sum"] > cr["pair_length_sum"] and ca["uncoupled_pairs"] > cr["uncoupled_pairs"]
    assert ca["cost"] > cr["cost"] + 10
    ic = pat.extra["info_careless"]
    st = pat.extra["stagger"]
    pat.purpose = ("qmtech's HDMI chain: connector row J1 (P / GND / N per pair, 0.5 pitch, room behind) -> ESD chip U2 "
                   "(TPD12S520-like, FLOW-THROUGH: every signal has a pad in both rows and the pair runs straight over "
                   "both pads; a GND pad between P and N in each row) -> transmitter U1 (QFP, P N adjacent). The J1 row "
                   f"is {ESD_DX:.2f} west of the U2 rows (qmtech 0.027). Every GND pad needs a via to GND. Board Setup and "
                   "net classes are qmtech's. Question: where do the U2 GND vias go, and how big?")
    pat.answer = (f"U2: GND via BETWEEN P and N on the inner side of each row (the outer side of a flow-through row "
                  f"is where the pair comes from or goes on to: no 'behind'), the SMALLEST via the constraints allow: "
                  f"{h['via'][0]}/{h['via'][1]} (drill = min through hole {QM_DRILL}, pi x {QM_DRILL} = {h['pi_drill']:.2f} "
                  f">= the {GND_W} stub; diameter = max(min via {QM_RULES['min_via_diameter']}, {QM_DRILL} + 2 x "
                  f"{QM_RULES['min_via_annular_width']}) = {h['via'][0]}), centred {h['via_beyond']:.2f} beyond the pad end. "
                  f"It fits between the STRAIGHT tracks: via r + clearance {QM_CLEAR} + half track = {h['need']:.3f} <= 0.5 "
                  f"(margin {h['fit_margin']:.3f}), so the pair is not widened; it converges {h['converge_end']:.3f} past "
                  f"the pad end (legs {0.325:.3f}) and is coupled {h['body_between_rows']:.2f} mm between the rows. J1 has "
                  f"room behind: its GND vias go behind (as dp_gnd_between). Pairs F.Cu, 0 vias; the J1 tracks enter "
                  f"{ESD_DX:.2f} east of the pad centres (run-out {h['d0_J1']:.2f}), so U2 is entered centred. Per track "
                  f"{h['track']:.3f} mm, skew 0, uncoupled 0 beyond the fans. Price {cr['cost']:.1f} = pairs "
                  f"{cr['pair_length_sum']:.2f} + GND {cr['gnd_stub_mm']:.2f} + 6 x 25. Careless (GND class via "
                  f"{QM_CLASS_VIA[0]}/{QM_CLASS_VIA[1]}): {ca['cost']:.1f}, +{ca['cost'] - cr['cost']:.1f}.")
    pat.why = (f"Graded rule (user, 2026-11-10): a GND via goes behind its pad when there is room, else between P and N "
               f"with the smallest via allowed. In a flow-through row the pair occupies both sides, so the via must sit "
               f"between P and N and the pair must pass it: a via wider than {2 * (HP - QM_CLEAR - DP_W / 2):.2f} pushes P and "
               f"N apart (0.005 per side per 0.01 of diameter). The smallest via ({h['via'][0]}) needs {h['need']:.3f} from each track centre and the "
               f"tracks are 0.5 away: no widening (hole: {QM_DRILL / 2} + {QM_RULES['min_hole_clearance']} + 0.1 = "
               f"{0.5 - h['hole_margin']:.2f} <= 0.5). The GND class via {QM_CLASS_VIA[0]} needs {h['class_via_need']:.2f}: "
               f"both tracks step out {h['class_via_step_out']:.2f} (pitch {2 * (HP + h['class_via_step_out']):.1f}), the via "
               f"must sit {h['class_via_beyond']:.2f} beyond the pad (minimum {h['class_via_beyond_min']:.3f}), the pair "
               f"converges {ic[0]['top_via_fan']['converge_end_past_pad_end']:.3f} past the pad end instead of "
               f"{h['converge_end']:.3f}, and the neighbouring bulges collide (TX0_N and TX1_P both 0.65 off their axes 1.5 apart: centres 0.2 "
               f"apart), so "
               f"one pair's via per row moves {st:.2f} further in ({ic[1]['top_via_fan']['converge_end_past_pad_end']:.3f} "
               f"past the pad end): the coupled stretch between the rows drops from {h['body_between_rows']:.2f} to "
               f"{ic[0]['body_between_rows']:.2f}, {ca['uncoupled_pairs']:.2f} mm uncoupled over 4 tracks, "
               f"+{ca['pair_length_sum'] - cr['pair_length_sum']:.2f} mm, +{ca['gnd_stub_mm'] - cr['gnd_stub_mm']:.1f} mm GND "
               f"stub. qmtech's hand vias at U3 (0.6 / 0.3) need 0.55: its tracks step out {h['qmtech_via_step_out']:.2f} "
               f"per side (the measured 1.1 pitch), the same mechanism one size up. KiCad's DRC also drops coupling "
               f"where copper lies between P and N (the via), which the fans already exclude.")
    pat.expected = {
        "interpretation": {
            "needs_user_confirmation": True,
            "built": "2-layer board, GND = B.Cu pour (terminal); qmtech Board Setup constraints and GND / HDMI_TX "
                     "clearances; the ESD chip as two flow-through rows (TPD12S520: rows 5.726 apart, here 5.75), J1 0.05 "
                     "west of U2 (qmtech 0.027) absorbed by off-centre entry in the 0.3 connector pads; pair values the "
                     "module's 0.20 / 0.15 / 0.25",
            "open_question": "qmtech's HDMI_TX dp values are 0.16 / 0.2 / 0.25 (built: the module's 0.20 / 0.15 / 0.25 "
                             "shared by all patterns; with 0.16 the via would have 0.045 margin). Copper-to-edge 0.5 "
                             "(qmtech 0.0). Via on the inner side of each ESD row (built) or outer (qmtech has one each "
                             "side, two per GND pad)? 2-layer pour or 4 layers with an In1 GND plane?"},
        "rules": {"board_setup_qmtech": dict(QM_RULES, min_copper_edge_clearance=f"{EDGE} (qmtech 0.0)"),
                  "classes": QM_CLASSES, "predefined_vias_qmtech": [[0.56, 0.3], [0.6, 0.3], [0.8, 0.4], [1.0, 0.45]],
                  "gnd_to_pair_clearance": f"{QM_CLEAR} (KiCad: the larger of the two items' class clearances: GND 0.15, "
                                           "HDMI_TX 0.127)"},
        "gnd": {"model": "B.Cu pour over the whole board (terminal: vias on it always allowed)", "stub_width": GND_W,
                "via": h["via"], "via_rule": h["via_rule"], "via_beyond_pad_end": h["via_beyond"],
                "via_beyond_min": r4(h["via_beyond_min"]), "side": {"J1": "behind (room)", "U2": "between P and N, "
                                                                     "inner side of each row (flow-through)"},
                "fit_between_straight_tracks": {"need": r4(h["need"]), "have": HP, "margin": r4(h["fit_margin"]),
                                                "with_clearance_0.2": r4(h["need_with_module_clear_0.2"])}},
        "qmtech_grounding": QMTECH_HDMI,
        "polarity": {"swaps_needed": 0, "turn_backs": 0},
        "vias": len(ref.vias), "pair_vias": 0, "gnd_vias": 6, "crossings": 0,
        "gateways": [{"id": g["id"], "pt": rp(g["pt"])} for g in pat.gateways],
        "lengths": {"per_track": r4(h["track"]), "C1_J1_U2": r4(h["C1"]), "body_between_rows": r4(h["body_between_rows"]),
                    "C3_U2_U1": r4(h["C3"]), "converge_past_pad_end": r4(h["converge_end"]),
                    "gnd_stub_J1": r4(h["stub_J1"]), "gnd_stub_U2": r4(h["stub_U2"]), "pair_skew_max": cr["pair_skew_max"],
                    "run_out": {"J1": h["d0_J1"], "U2": h["d0_pgn"], "U1": h["d0_qfp"]}},
        "cost": cr,
        "price_table": {
            "model": f"pairs: mm + {VIA_PRICE:.0f} per via + U x uncoupled mm (beyond the minimal fans), U = "
                     f"{UNCOUPLED_PRICE}; GND: stub mm + {VIA_PRICE:.0f} per via",
            f"reference (U2 GND vias {h['via'][0]}/{h['via'][1]} between P and N)": cr,
            f"careless (GND class vias {QM_CLASS_VIA[0]}/{QM_CLASS_VIA[1]}, pairs step out, staggered)": ca,
            "break_even_U": "none: the careless layout is longer AND less coupled, dearer at every U >= 0"},
        "alternative": {"label": alt.label, "cost": ca, "step_out": h["class_via_step_out"], "stagger": st,
                        "converge_past_pad_end": [ic[0]["top_via_fan"]["converge_end_past_pad_end"],
                                                  ic[1]["top_via_fan"]["converge_end_past_pad_end"]]},
        "judgement_calls": [
            f"Via size graded by room: the smallest allowed ({h['via'][0]}/{h['via'][1]}) when it sits between P and N; "
            "a router should price each extra 0.01 of diameter by the widening and lost coupling it causes, not pick the "
            "net-class via.",
            f"Via on the 0.05 grid {h['via_beyond']:.2f} beyond the pad end (minimum {h['via_beyond_min']:.3f}); qmtech "
            "0.20-0.23.",
            "One via per GND pad (qmtech has two per U3 GND pad, one each side of the row).",
            f"Careless panel staggers one pair's vias per row by {st:.2f} (searched on the 0.05 grid until the shapely "
            "check passes)."],
        "literature": _common_lit() + [HDMI_LIT, KICAD_COUPLING],
    }


for _n, _b, _f in (("dp_gnd_between", p_gnd_between, finish_gnd_between),
                   ("dp_esd_flowthrough", p_esd_flowthrough, finish_esd)):
    BUILD[_n], FINISH[_n], CHECK[_n] = _b, _f, (lambda pat: None)  # the finish functions run check_gnd
NAMES = list(BUILD)
GND_PATS = ("dp_gnd_between", "dp_esd_flowthrough")
BUSLIKE = ("dp_bus_crossing",) + GND_PATS
COSTF = {n: (lambda pat, sol: _cost_gnd(pat, sol)) for n in GND_PATS}
COSTF["dp_bus_crossing"] = lambda pat, sol: _cost_bus(sol)



def pair_via(pat: DPat, cls: dict) -> tuple[float, float]:
    """The via of a pair of class `cls` on `pat`'s board, derived from the Board Setup minimums by hand (rule 6; user
    2026-11-12: "the smaller the via the better, to keep impedance disruption to a minimum"): drill = the smallest
    through-hole drill with pi x drill >= the pair width, diameter = max(min via diameter, drill + 2 min annular), the
    class via when that is smaller still (no pattern here has pre-defined vias). The router's default
    ``[diffpair] via = "smallest"`` must arrive at the same size (toporlib.diffpair.smallest_via)."""
    R = pat.rules
    w = cls.get("diff_pair_width") or cls["track_width"]
    drill = R["min_through_hole_diameter"]
    assert math.pi * drill >= w - 1e-9, (pat.name, drill, w)
    d = max(R["min_via_diameter"], drill + 2 * R["min_via_annular_width"])
    return min((round(d, 4), round(drill, 4)), (cls["via_diameter"], cls["via_drill"]))


def rules_assumed(pat: DPat) -> dict:
    """The rule set the expected answer of `pat` is built for (harness.validity ``rules_assumed``, mm): the board's
    own Board Setup, net classes and .kicad_dru (no fab profile, exact clearances), and for every pair net the one
    via it may place, :func:`pair_via` (the gate checks it against the router's effective via list). A single net
    whose reference via is not its net class via (the crossers of dp_bus_crossing, the GND vias between P and N: the
    smallest the Board Setup allows, rule 6) needs ``[vias] size = "board_min"`` (engine vias.rs rule_size: the
    smallest Board Setup via with pi x drill >= the trace width; pair nets keep the pair via in that mode too), so
    the declaration names that option and :func:`pair_via` must give the same size for that net. Without it the
    router has only the net class via there (no pre-defined vias on these boards; before 2026-11-12 the engine's
    hidden JLCPCB via list offered 0.45 / 0.25, which made such a test look valid). O(nets + vias)."""
    from fnmatch import fnmatchcase  # noqa: PLC0415

    from toporlib.diffpair import pair_nets  # noqa: PLC0415

    by_name = {c["name"]: c for c in pat.classes}
    cls_of = {}
    for np_ in pat.net_patterns:  # KiCad netclass_patterns: wildcard match, first match wins
        for n in pat.nets:
            if n not in cls_of and fnmatchcase(n, np_["pattern"]):
                cls_of[n] = by_name[np_["netclass"]]
    pn = pair_nets(pat.nets)
    nets = {n: {"vias": [list(pair_via(pat, cls_of[n]))]} for n in sorted(pn) if n in cls_of}
    out = {"authority": "board", "margin": 0.0, "nets": nets}
    default = by_name.get("Default") or {}
    for v in pat.ref.vias:
        if v.net in pn:
            continue
        cls = cls_of.get(v.net, default)
        if (v.dia, v.drill) != (cls.get("via_diameter"), cls.get("via_drill")):
            # rule_size of the net's own track width (pair_via with that width in place of the pair width)
            assert (v.dia, v.drill) == pair_via(pat, {**cls, "diff_pair_width": cls["track_width"]}), (pat.name, v)
            out["options"] = {"vias.size": "board_min"}
    return out


def build(name: str) -> DPat:
    pat = BUILD[name]()
    FINISH[name](pat)
    CHECK.get(name, check)(pat)
    return pat


ZOOMS = {
    "dp_gateway": [((2.3, 5.9, 6.3, 8.1), "gateway G1 at J1 (0.5 mm pads)"), ((16.2, 2.9, 19.8, 5.3), "gateway G2 at U1 (0.65 mm pads)")],
    "dp_swap_needed": [((7.4, 4.0, 11.9, 8.6), "swap site: up to via pair A (turn back), down on B.Cu, via pair B")],
    "dp_swap_no_room": [((10.2, 6.6, 14.2, 9.4), "single-track hop in the corridor (2 vias)")],
    "dp_uturn_parity": [((2.3, 2.3, 7.3, 5.3), "J1 and via pair A (turn back)"), ((2.3, 8.7, 6.3, 11.3), "arrival at J2 (B.Cu pads)")],
}
ZOOMS["dp_gnd_between"] = [((1.7, 2.3, 5.8, 7.3), "J1: GND vias behind the GND pads, gateways right past the pads"),
                           ((1.9, 8.7, 5.6, 12.2), "U1 (QFP, P N adjacent): fans and gateways")]
ZOOMS["dp_esd_flowthrough"] = [((1.8, 5.9, 5.8, 10.4), "U2 top row: pairs straight through the pads, GND via 0.45/0.2 "
                                                       "between P and N, no widening"),
                               ((1.8, 1.6, 5.8, 7.6), "J1: GND vias behind, tracks enter 0.05 east of the pad centres")]
ZOOMS["dp_bus_crossing"] = [((9.6, 2.0, 14.4, 5.0), "pad fans and gateways of pairs 0 and 1 at U1"),
                            ((9.8, 4.2, 18.2, 13.4), "B.Cu crossers under the F.Cu bus: vias outside, 90 and 45 degrees")]


# ----------------------------------------------------------------------------------------------- board files
def write_pat(view, path: Path, *, copper: bool) -> Path:
    from harness.fit_patterns import LAYER_IDS  # noqa: F401

    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    b = build_board(view, copper=copper)
    for p in b.GetPads():  # back-side SMD pads: copper, mask and paste on B (build_board puts the mask on F)
        if p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD and p.IsOnLayer(pcbnew.B_Cu):
            ls = pcbnew.LSET()
            for ly in (pcbnew.B_Cu, pcbnew.B_Mask, pcbnew.B_Paste):
                ls.AddLayer(ly)
            p.SetLayerSet(ls)
    for zd in view.zones:  # copper pours (GND plane on B.Cu): a terminal of its net
        z = pcbnew.ZONE(b)
        z.SetLayer(LAYER_IDS[zd["layer"]])
        z.SetNet(b.FindNet(zd["net"]))
        z.SetLocalClearance(int(round(CLEAR * 1e6)))
        x0, y0, x1, y1 = zd["rect"]
        ol = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)):
            ol.Append(pcbnew.VECTOR2I(int(round(x * 1e6)), int(round(y * 1e6))))
        ol.SetClosed(True)
        z.AddPolygon(ol)
        b.Add(z)
    if view.zones:
        pcbnew.ZONE_FILLER(b).Fill(b.Zones())
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


def _pad_geo(p: Pad):
    return box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2, p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2)


def draw_board(ax, pat: DPat, sol: Sol | None, *, ratsnest=False, window=None, small=False, title="", labels=True, gateways=True):
    from matplotlib.patches import Circle, Polygon as MPoly, Rectangle

    W, H = pat.size
    ax.add_patch(Rectangle((0, 0), W, H, fc="#fbfbf6", ec="k", lw=1.0, zorder=0))
    for x0, y0, x1, y1 in ((0, 0, W, EDGE), (0, H - EDGE, W, H), (0, EDGE, EDGE, H - EDGE), (W - EDGE, EDGE, W, H - EDGE)):
        ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="#f3d9d9", ec="#c06060", hatch="xxxx", lw=0.3, zorder=0.7))
    for w in pat.walls:
        x0, y0, x1, y1 = w["rect"]
        ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="#d8d8d8", ec="#555555", hatch="..", lw=0.8, zorder=1))
        if labels:
            ax.text((x0 + x1) / 2, (y0 + y1) / 2, f"{w['label']}\nTHT body\n(F.Cu + B.Cu)", fontsize=5.5, ha="center",
                    va="center", color="#444444", zorder=1.2)
    for ol in pat.extra.get("outlines", []):  # component bodies (no copper): dashed grey outline
        x0, y0, x1, y1 = ol["rect"]
        ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="none", ec="#777777", lw=0.9, ls=(0, (5, 3)), zorder=1.1))
        if labels and not small:
            ax.text(x1 + 0.1, (y0 + y1) / 2, ol["label"], fontsize=5.6, rotation=90, ha="left", va="center",
                    color="#555555", zorder=1.2)
    for zd in pat.zones:  # copper pour (terminal of its net): hatched, pale, in the net colour, inside the edge band
        x0, y0, x1, y1 = (max(v, EDGE) if i < 2 else v for i, v in enumerate(zd["rect"]))
        x1, y1 = min(x1, W - EDGE), min(y1, H - EDGE)
        ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc=COLORS[zd["net"]], ec=COLORS[zd["net"]], alpha=0.13,
                               hatch="\\\\", lw=0.6, zorder=0.8))
        if labels and not small:
            ax.text(x1 - 0.15, y0 + 0.15, f"{zd['layer']} {zd['net']} pour (whole board, a terminal)", fontsize=5.8,
                    ha="right", va="top", color="#6b4f06", zorder=1.2)
    for f in pat.fps:
        for p in f.pads:
            if f.role == "obstacle":
                continue
            col = COLORS.get(p.net, "#9a9a9a")
            back = p.layer == "B.Cu"
            g = _pad_geo(p)
            ax.add_patch(MPoly(_poly(g), closed=True, fc=col, ec="k", lw=0.4, alpha=0.8 if not back else 0.45,
                               hatch="////" if back else None, zorder=2))
            if labels and p.net:
                txt = f"{f.ref}.{p.num} {p.net}" + (" (B.Cu)" if back else "")
                fs = 5.0 if small else 5.8
                if p.size[1] > p.size[0] and pat.name in BUSLIKE:  # bus pads: short rotated labels
                    up = f.ref == "U1" if pat.name == "dp_bus_crossing" else p.pos[1] < H / 2
                    off = p.size[1] / 2 + (0.1 if pat.name == "dp_bus_crossing" else 0.6)  # GND patterns: past the vias
                    ax.text(p.pos[0], p.pos[1] + (-1 if up else 1) * off, p.net, fontsize=4.6,
                            rotation=90, ha="center", va="bottom" if up else "top", zorder=6)
                elif p.size[1] > p.size[0]:  # vertical pad: label above, P to the left, N to the right
                    ax.text(p.pos[0] + (-0.12 if p.net == "USB_P" else 0.12), p.pos[1] - p.size[1] / 2 - 0.15, txt,
                            fontsize=fs, ha="right" if p.net == "USB_P" else "left", va="bottom", zorder=6, rotation=0)
                else:
                    ax.text(p.pos[0] + (-0.9 if f.ref[0] == "J" else 0.9), p.pos[1], txt, fontsize=fs,
                            ha="right" if f.ref[0] == "J" else "left", va="center", zorder=6, color="#222222")
    # copper: B.Cu first, hatched; F.Cu solid
    if sol is not None:
        for layer in ("B.Cu", "F.Cu"):
            for t in sol.tracks:
                if t.layer != layer:
                    continue
                g = LineString(t.pts).buffer(t.width / 2, cap_style="round", join_style="round", quad_segs=8)
                top = layer == "F.Cu"
                for geom in getattr(g, "geoms", [g]):
                    ax.add_patch(MPoly(_poly(geom), closed=True, fc=COLORS[t.net], ec="k" if top else COLORS[t.net],
                                       lw=0.25 if top else 0.7, ls="-" if top else (0, (2, 1)), alpha=0.85 if top else 0.45,
                                       hatch=None if top else "////", zorder=4.5 if top else 4))
        for i, v in enumerate(sol.vias, 1):
            ax.add_patch(Circle(v.pos, v.dia / 2, fc=COLORS[v.net], ec="k", lw=0.5, zorder=5))
            ax.add_patch(Circle(v.pos, v.drill / 2, fc="white", ec="k", lw=0.25, zorder=5.1))
            if labels and len(sol.vias) <= 8:
                ax.text(v.pos[0] + 0.32, v.pos[1] + (-0.3 if v.net == "USB_P" else 0.3), f"V{i}", fontsize=5.5, color="k",
                        ha="left", va="center", zorder=9, bbox=dict(fc="#fff6c8", ec="none", pad=0.1, alpha=0.9))
        for mk in sol.marks:
            ax.add_patch(Circle(mk["pt"], 0.45, fc="none", ec=BAD, lw=1.4, ls="--", zorder=8))
            ax.text(mk["pt"][0] + 0.6, mk["pt"][1] - 0.6, mk["label"], fontsize=6, color=BAD, zorder=9)
        for bx in sol.boxes:
            x0, y0, x1, y1 = bx["rect"]
            ax.add_patch(Rectangle((x0, y0), x1 - x0, y1 - y0, fc="none", ec="#7a3db8", lw=1.0, ls=(0, (4, 2)), zorder=8))
            if labels:
                ax.text(x0, y0 - 0.1, bx["label"], fontsize=6, color="#7a3db8", ha="left", va="bottom", zorder=9)
    nb = pat.extra.get("need_box")
    if nb and sol is None:
        ax.add_patch(Rectangle((nb[0], nb[1]), nb[2] - nb[0], nb[3] - nb[1], fc="none", ec="#7a3db8", lw=1.2,
                               ls=(0, (4, 2)), zorder=8))
        ax.text(nb[0] + 0.1, nb[1] - 0.1, f"area one swap U needs: {nb[2] - nb[0]:.2f} x {8.0 - nb[1]:.2f} above the line "
                f"(corridor: 0.70 above, door: 1.8 wide)", fontsize=6, color="#7a3db8", ha="left", va="bottom", zorder=9)
    if ratsnest:
        pos = {f"{f.ref}.{p.num}": p.pos for f in pat.fps for p in f.pads}
        for net, a, b in pat.connections:
            if b in pos:
                ax.plot(*zip(pos[a], pos[b]), ls=(0, (2, 2)), lw=0.9, color=COLORS[net], zorder=3)
            else:  # to a pour: a dashed ring on the pad = 'needs a via to the plane'
                ax.add_patch(Circle(pos[a], 0.32, fc="none", ec=COLORS[net], lw=0.9, ls=(0, (2, 2)), zorder=3))
    if sol is not None and labels and gateways:
        for g in pat.gateways:
            ax.plot([g["pt"][0]], [g["pt"][1]], marker="+", ms=7, color="k", mew=1.0, zorder=9)
            ax.text(g["pt"][0] + 0.12, g["pt"][1] - 0.3, g["id"], fontsize=6.5, weight="bold", zorder=9)
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


def _legend(fig, rect_y, bus=False, gnd=False, via_txt="via 0.45 / 0.25 (white drill), Vn"):
    from matplotlib.lines import Line2D
    from matplotlib.patches import Patch

    h = ([Patch(fc=COLORS["TX0_P"], ec="k", label="pair TXk: P dark / N light (TX0 blue, TX1 green)"),
          Patch(fc=COLORS[GND], ec="k", label="GND (pads, stubs, vias)"),
          Patch(fc=COLORS[GND], ec=COLORS[GND], alpha=0.25, hatch="\\\\", label="B.Cu GND pour (plane = terminal)"),
          Line2D([], [], marker="o", ls="", mfc="none", mec=COLORS[GND], label="dashed ring: GND pad needs a via (problem)")]
         if gnd else
         [Patch(fc=COLORS["TX0_P"], ec="k", label="pair TXk: P dark / N light (k = 0..3 hue)"),
          Patch(fc=COLORS["TX1_N"], ec="k", label="TX1 N"),
          Patch(fc=COLORS["SIG_A"], ec="k", label="crossers SIG_A / SIG_B (cyan) / SIG_C (olive)")] if bus else
         [Patch(fc=COLORS["USB_P"], ec="k", label="USB_P (net colour; pads, tracks, vias)"),
          Patch(fc=COLORS["USB_N"], ec="k", label="USB_N")]) + [
         Patch(fc="#888888", ec="k", alpha=0.85, label="F.Cu copper (solid, black outline)"),
         Patch(fc="#888888", ec="#888888", alpha=0.45, hatch="////", label="B.Cu copper (hatched, dashed outline)"),
         Line2D([], [], marker="o", ls="", mfc="#888888", mec="k", label=via_txt),
         Line2D([], [], ls=(0, (2, 2)), color="#444444", lw=0.9, label="ratsnest (problem)"),
         Patch(fc="#f3d9d9", ec="#c06060", hatch="xxxx", label="copper-to-edge band 0.50: no copper"),
         Line2D([], [], marker="+", ls="", color="k", mew=1.0, label="Gn gateway = end of the pad fan-out")] + (
        [Patch(fc="#9a9a9a", ec="k", label="grey pad: other pin (no net)"),
         Line2D([], [], marker="o", ls="", mfc="none", mec=BAD, mew=1.2, label="red dashed ring: the careless choice")]
        if gnd else
        [Patch(fc="#d8d8d8", ec="#555555", hatch="..", label="THT body: copper on both layers (no net)"),
         Line2D([], [], ls=(0, (4, 2)), color="#7a3db8", lw=1.0, label="swap-site footprint incl. clearances")])
    fig.legend(handles=h, loc="lower center", ncol=3, fontsize=6.3, frameon=True, bbox_to_anchor=(0.5, rect_y))


def render(pat: DPat, doc: dict, out: Path) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    W, H = pat.size
    pw = 9.4
    ph = pw * (H + 1.0) / (W + 0.8)
    third = pat.alt or pat.careless
    n_pan = 3 if third else 2
    top, leg_h, foot_h, gap = 0.5, 1.1, 1.6, 0.15
    portrait = H > W  # tall boards: the panels side by side (problem | expected | careless), titles wrapped
    if portrait:
        cw = 5.0
        ph = cw * (H + 1.0) / (W + 0.8)
        top = 1.35
        fw = n_pan * cw + 0.6
        fh = top + ph + leg_h + foot_h
        fig = plt.figure(figsize=(fw, fh))
        axs = [fig.add_axes([(0.3 + k * cw + 0.35) / fw, (leg_h + foot_h + 0.2) / fh, (cw - 0.5) / fw, (ph - 0.25) / fh])
               for k in range(n_pan)]
    else:
        fh = top + n_pan * (ph + gap) + leg_h + foot_h
        fig = plt.figure(figsize=(pw + 0.4, fh))
        axs = [fig.add_axes([0.05, 1 - (top + (k + 1) * (ph + gap) - gap * 0.5) / fh, 0.9, (ph - 0.45) / fh])
               for k in range(n_pan)]
    def draw(ax, *a, title="", **kw):  # portrait: titles wrapped for the narrow columns
        if portrait:
            title = "\n".join(textwrap.fill(t, 62) for t in title.split("\n"))
        return draw_board(ax, *a, title=title, **kw)

    bus = pat.name in BUSLIKE
    c = doc["expected"]["cost"]
    if bus:
        c = dict(c, length_P=c["pair_lengths"]["TX0"][0], length_N=c["pair_lengths"]["TX0"][1])
    draw(axs[0], pat, None, ratsnest=True, title="problem: pads, ratsnest (dashed), nothing routed; "
               + f"{{width: {DP_W}, gap: {DP_GAP}, via_gap: {DP_VIA_GAP}}}")
    unc = f", uncoupled {c['uncoupled_sum']:.2f} mm" if pat.name in ("dp_swap_no_room",) else ""
    npairs = 2 if pat.name in GND_PATS else 4
    pl = f"per pair-track {c['length_P']:.2f} / {c['length_N']:.2f} mm (all {npairs} pairs equal)" if bus else (
        f"per-track {c['length_P']:.2f} / {c['length_N']:.2f} mm")
    draw(axs[1], pat, pat.ref, title=f"EXPECTED = reference copper (KiCad DRC {doc['kicad']['reference_drc']['violations']}"
               f" violations / {doc['kicad']['reference_drc']['unconnected']} unconnected): {pat.ref.label}\n"
               f"{c['vias']} vias, {pl}, price {c['cost']:.1f} mm{unc}")
    if third is not None:
        sol = third
        cc = COSTF[pat.name](pat, sol) if bus else cost(sol)
        if pat.alt and pat.name in GND_PATS:
            ttl = (f"CARELESS (ruled out, DRC-clean copper): {sol.label}\n{cc['vias']} vias, per pair-track "
                   f"{cc['pair_lengths']['TX0'][0]:.2f} mm, uncoupled {cc['uncoupled_pairs']:.2f} mm, price {cc['cost']:.1f} "
                   f"mm (+{cc['cost'] - c['cost']:.2f}) (DRC {doc['kicad']['alternative_drc']['violations']} / "
                   f"{doc['kicad']['alternative_drc']['unconnected']})")
        elif pat.alt and bus:
            ttl = (f"CARELESS (ruled out, DRC-clean copper): {sol.label}\n{cc['vias']} vias, price {cc['cost']:.1f} mm "
                   f"(+{cc['cost'] - c['cost']:.1f}); each pair changes layer twice "
                   f"(DRC {doc['kicad']['alternative_drc']['violations']} / {doc['kicad']['alternative_drc']['unconnected']})")
        elif pat.alt:
            ttl = (f"ALTERNATIVE (not expected): {sol.label}\n{cc['vias']} vias, per-track {cc['length_P']:.2f} / "
                   f"{cc['length_N']:.2f} mm, uncoupled {cc['uncoupled_sum']:.2f} mm, price {cc['cost']:.1f} mm "
                   f"(DRC {doc['kicad']['alternative_drc']['violations']} / {doc['kicad']['alternative_drc']['unconnected']})")
        else:
            ttl = f"CARELESS (ruled out): {sol.label}\n" + textwrap.shorten(sol.text or doc["expected"]["careless"]["description"], 150)
        draw(axs[2], pat, sol, title=ttl, gateways=bool(pat.alt))
        # hatched-copper marker for the careless crossing is a mark in sol.marks
    vt = ", ".join(f"{a} / {b}" for a, b in sorted({(v.dia, v.drill) for sol in (pat.ref, pat.alt) if sol for v in sol.vias}))
    _legend(fig, (foot_h + 0.05) / fh, bus=pat.name == 'dp_bus_crossing', gnd=pat.name in GND_PATS,
            via_txt=f"via {vt} (white drill), Vn" if vt else "via 0.45 / 0.25 (white drill), Vn")
    fig.suptitle(f"{pat.name}: {pat.title}", fontsize=11, y=1 - 0.15 / fh)
    txt = "Expected: " + pat.answer + (("  " + READING) if pat.name in ("dp_swap_needed", "dp_swap_no_room") else "")
    fig.text(0.02, 0.005, "\n".join(textwrap.wrap(txt, 230 if portrait else 175)[:11]), fontsize=6.6, ha="left",
             va="bottom")
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=120)
    plt.close(fig)
    return out


def render_zoom(pat: DPat, out: Path) -> Path:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    zs = ZOOMS[pat.name]
    fig, axs = plt.subplots(1, len(zs), figsize=(7.2 * len(zs), 6.0), squeeze=False)
    for ax, (win, label) in zip(axs[0], zs):
        draw_board(ax, pat, pat.ref, window=win, small=True, title=label)
    fig.suptitle(f"{pat.name}: zoom on the expected copper (mm; 0.2 wide tracks, gap 0.15)", fontsize=10)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)
    return out


# ----------------------------------------------------------------------------------------------- generate
def generate(name: str, out: Path, *, png: bool = True, drc: bool = True) -> dict:
    pat = build(name)
    d = Path(out) / name
    paths = {"problem": write_pat(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_pat(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    kicad = {"problem_drc": {"violations": 0, "unconnected": 2}, "reference_drc": {"violations": 0, "unconnected": 0}}
    if pat.alt:
        paths["alternative"] = write_pat(_View(pat, pat.alt), d / "alternative" / f"{name}.kicad_pcb", copper=True)
        kicad["alternative_drc"] = {"violations": 0, "unconnected": 0}
    if drc:
        for key, p in (("problem_drc", paths["problem"]), ("reference_drc", paths["reference"]),
                       ("alternative_drc", paths.get("alternative"))):
            if p is not None:
                r = drc_of(p)
                if r.get("available"):
                    kicad[key] = {k: r[k] for k in ("violations", "unconnected", "types")}
    pins = {f"{f.ref}.{p.num}": {"net": p.net, "pos": rp(p.pos), "layer": p.layer, "size": list(p.size)}
            for f in pat.fps for p in f.pads if p.net}
    doc = {"name": name, "title": pat.title, "kind": pat.kind, "purpose": pat.purpose, "expected_summary": pat.answer,
           "why": pat.why, "board": {"size_mm": list(pat.size), "copper_layers": 2,
                                     "bodies": [{"ref": w["label"], "rect": [r4(v) for v in w["rect"]]} for w in pat.walls],
                                     "pours": [{"net": z["net"], "layer": z["layer"], "rect": [r4(v) for v in z["rect"]]}
                                               for z in pat.zones]},
           "pins": pins, "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "expected": pat.expected,
           "reference": {"label": pat.ref.label,
                         "tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": [rp(p) for p in t.pts]}
                                    for t in pat.ref.tracks],
                         "vias": [{"net": v.net, "pos": rp(v.pos), "dia": v.dia, "drill": v.drill} for v in pat.ref.vias]},
           "alternative": ({"label": pat.alt.label, "tracks": [{"net": t.net, "layer": t.layer, "width": t.width,
                                                                "pts": [rp(p) for p in t.pts]} for t in pat.alt.tracks],
                            "vias": [{"net": v.net, "pos": rp(v.pos)} for v in pat.alt.vias]} if pat.alt else None),
           "careless": ({"label": pat.careless.label, "text": pat.careless.text,
                         "tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": [rp(p) for p in t.pts]}
                                    for t in pat.careless.tracks]} if pat.careless else None),
           "kicad": kicad, "topor": "NOT run (user 2026-11-10); TopoR Lite may not support differential pairs at all "
                                    "(pairs are a TopoR 6.x / 'Place diff pair' feature, manual pp. 171-173)",
           "literature": pat.expected.get("literature", []), "rules_assumed": rules_assumed(pat)}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        paths["png"] = render(pat, doc, d / f"{name}.png")
        paths["zoom"] = render_zoom(pat, d / f"{name}_zoom.png")
    return paths


def readme(out: Path, docs: dict[str, dict]) -> Path:
    L = ["# Diff-pair test patterns (shopping list 5.6)", "",
         "Generated by `python -m harness.diffpair_patterns` (module docstring: rules, price model). Each pattern has "
         "`problem/` (pads only), `reference/` (the expected copper, KiCad DRC measured), `alternative/` (the priced "
         "second answer, where it is real copper), `expected.json`, `<name>.png` (problem | expected | alternative or "
         "careless) and `<name>_zoom.png`.", "",
         "## Rule values used by every pattern (Board Setup / net class, written to each `.kicad_pro`)", "",
         "- Net class `USB` (nets `USB_*`; KiCad pair names `USB_P` / `USB_N`):", "  - NORMAL values: track 0.25, clearance 0.20, "
         "via 0.60 / 0.30. Never used on a coupled stretch; a router that uses them is visibly wrong.",
         "  - PAIR values: `diff_pair_width` 0.20, `diff_pair_gap` 0.15, `diff_pair_via_gap` 0.25 -> centre pitch 0.35, "
         "pair band 2 x 0.2 + 2 x 0.2 + 0.15 = 0.95 (ordering width, user rule 1).",
         "- Board Setup minimums = the supplier's capability: via diameter 0.45, through-hole drill 0.25, annular ring 0.10, "
         "track 0.20, clearance 0.15 (must not exceed the pair gap, else KiCad reports `diff_pair_gap_out_of_range`), "
         "hole to hole 0.25, copper to edge 0.50.",
         "- Vias in every reference: 0.45 / 0.25 (smallest allowed drill, pi x 0.25 = 0.785 >= 0.25 trace width; diameter = "
         "0.25 + 2 x 0.10); a via pair has centre pitch 0.45 + 0.25 = 0.70 (KiCad `EffectiveDiffPairViaGap`).",
         "- Exception `dp_esd_flowthrough`: qmtech's Board Setup (min via 0.45, min hole 0.2, annular 0.1, clearance "
         "0.09, hole clearance 0.25) and its net classes GND / HDMI_TX (clearance 0.15 / 0.127; GND class via 0.8 / 0.4), "
         "so its smallest allowed via is 0.45 / 0.2; copper to edge kept at 0.50 (qmtech 0.0).",
         "- Price: mm of copper + 25 mm per via + U (= 2.0, proposal) per uncoupled mm.", "",
         "**" + READING + "**", "",
         "TopoR was NOT run on these (user 2026-11-10). TopoR Lite may not support differential pairs at all; the pair "
         "workflow in TopoR 6.3 is 'Place diff pair' / zipping (manual pp. 154-155, 160, 171-173).", ""]
    judg: list[str] = []
    for n, doc in docs.items():
        e = doc["expected"]
        L += [f"## {n}: {doc['title']}", "", f"![{n}]({n}/{n}.png)", "", f"Zoom: `{n}/{n}_zoom.png`", "",
              f"**Expected.** {doc['expected_summary']}", "", f"**Why.** {doc['why']}", "",
              f"**KiCad DRC.** problem {doc['kicad']['problem_drc']}, reference {doc['kicad']['reference_drc']}"
              + (f", alternative {doc['kicad']['alternative_drc']}" if "alternative_drc" in doc["kicad"] else "")
              + (" (problem: the B.Cu GND pour has nothing on it yet = `isolated_copper`; the GND pads count as "
                 "unconnected to each other)" if n in GND_PATS else ""), ""]
        g = e.get("qmtech_grounding")
        if g and "stackup" not in g:  # the HDMI patterns: connector / ESD chip / GND ties as found on qmtech
            L += ["**Grounding on the real board** (`" + g["board"] + "`):", ""] + [
                f"- {k.replace('_', ' ')}: {v}" for k, v in g.items() if k != "board"] + [""]
            pt = e.get("price_table", {})
            L += ["**Price table.** " + "; ".join(
                f"{k}: {v['cost'] if isinstance(v, dict) and 'cost' in v else v}" for k, v in pt.items() if k != "model")
                + f" ({pt.get('model', '')}).", ""]
        elif g:
            L += ["**Grounding on the real board** (`" + g["board"] + "`): " + str(g["copper_layers"]) + " layers (" +
                  ", ".join(g["stackup"]) + "); pairs " + ", ".join(g["pairs"]) + ": " + g["pair_routing"] + ". Board Setup: "
                  + json.dumps(g["pair_values_in_board_setup"]) + ". Measured: " + json.dumps(g["hand_measured"]) +
                  ". Crossers: " + json.dumps(g["crossers_in_the_hand_layout"]) + ". " + g["summary"] + ".", ""]
        for j in e.get("judgement_calls", []):
            judg.append(f"- {n}: {j}")
        oq = e["interpretation"].get("open_question")
        if oq:
            judg.append(f"- **{n} (needs user confirmation):** {oq}")
    lit = list(dict.fromkeys(x for d in docs.values() for x in d["literature"]))  # union, first-seen order
    L += ["## Judgement calls needing the user", ""] + judg + ["", "## Literature", ""] + [f"- {x}" for x in lit]
    p = Path(out) / "README.md"
    p.write_text("\n".join(L) + "\n")
    return p


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "diffpair-patterns"
    docs = {}
    for n in NAMES:
        g = generate(n, out)
        docs[n] = json.loads(g["expected"].read_text())
        print(n, {k: str(v) for k, v in g.items()}, docs[n]["kicad"])
    print(readme(out, docs))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
