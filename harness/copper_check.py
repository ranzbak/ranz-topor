#!/usr/bin/env python3
"""copper_check: read numbers, not pictures. One review tool for a ROUTED .kicad_pcb.

  python3 harness/copper_check.py <routed.kicad_pcb> --out DIR [--pro X.kicad_pro] [--result result.json]
          [--drc drc.json | --no-drc] [--ratio 1.3] [--spots harness/copper_check_spots.json]

Output DIR/copper-check.json + copper-check.md. Rows have the shape of the kicad-cli DRC JSON
({"type","severity","description","items":[{"description","pos":{"x","y"},"uuid"}]}) plus "net", "connection",
"size_mm", "size_label", "minutes" (estimated hand-cleanup), "source" ("kicad" = a stock DRC row, "rule" = a row of the
generated review rules, "own" = our own check), so one pipeline reads both. Ranked by minutes, total per run.

Two parts:
 1. Review rules (.kicad_dru, severity warning) generated per run into a TEMP COPY of the board (never into the user's
    project) and read back from kicad-cli DRC (isolated KiCad home, harness.kicad_env). Constraints that exist in
    KiCad 10.0.8 and work (probed 2026-11-27): track_width (necks, one rule per net class), track_segment_length
    (jogs), length (per net, ratio x direct, whole-net not per-connection), physical_clearance on same-net via/pad
    pairs (`disallow via` cannot see the pad, `clearance` skips same-net items, physical_clearance does not; it sees
    outlines that touch, a via fully inside a bigger pad has a positive outline distance: the own check covers it). DROPPED: track_angle parses but never fires on any value (probed 0..179 deg, with and without condition):
    acute / free-angle are own checks.
 2. Own checks (pure python + shapely, no pcbnew): see CHECKS. Everything is a number from the copper.

Local, one process, no routing. Minutes per kind are the MINUTES table below (a guess to rank by, not a measurement).
"""
from __future__ import annotations

import argparse
import fnmatch
import heapq
import json
import math
import re
import shutil
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

import shapely
from shapely import affinity
from shapely.geometry import LineString, Point, Polygon, box
from shapely.strtree import STRtree

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

# ---------------------------------------------------------------- knobs (documented defaults)
RATIO = 1.3            # per-connection max length = RATIO x direct pad distance
RATIO_MIN_EXTRA = 0.5  # mm: a detour must also be this much longer than direct
NECK_TOL = 0.004       # mm below class width that counts as a neck
PIECE = 0.4            # mm: sampling piece along a track for room measurements
ROOM_R = 2.0           # mm: how far beside a trace we look for foreign copper
HUG_TOL = 0.01         # mm: gap over the required clearance that counts as "exactly at minimum"
HUG_MIN_LEN = 0.6      # mm: shortest reported run at minimum clearance
BEND_DEG = 15.0        # turns below this are not bends
ACUTE_DEG = 90.0       # interior angle at or below this is a spike
FREE_ANGLE_TOL = 0.5   # deg off the 45 deg grid
FREE_ANGLE_MIN = 0.3   # mm of free-angle copper in a connection to report
DECAP_MAX = 6.0        # mm cap-pad to IC pin before it is reported
SKEW_MAX = 0.5         # mm length mismatch of a diff pair
CENTRE_TOL = 0.01      # mm
DOUBLE_TOL = 0.05      # mm between two trace ends inside one pad
POWER_PIN_FRAC = 0.7   # a pad narrower than this fraction of the net width cannot carry the chain
OFFSET_FRAC = 0.4      # the entering track's line may miss the pad centre by this fraction of the pad short side (hand-calibrated 2026-11-28)
AXIS_DEG = 12.0        # pad entry heading tolerance to the pad long axis
FINE_PAD = 0.6         # entry checks only for pads at most this wide (fine pitch: the trace is as wide as the pad)
ASPECT_MIN = 1.3       # pads below this aspect have no axis
SIDE_DOT = 0.5         # entry direction . outward direction above this = wrong side

# estimated hand cleanup minutes per row (kind -> (base, per_unit, cap)); unit named in UNIT
MINUTES = {
    "neck_room_fits": (3.0, 0.4, 12.0),      # per mm of neck run: the corridor fits the class width
    "neck_room_partial": (2.0, 0.2, 6.0),    # corridor fits wider than drawn, not the class width
    "neck_forced": (0.5, 0.0, 0.5),          # no more room measured
    "via_in_pad": (4.0, 0.0, 4.0),
    "via_overlaps_pad": (2.0, 0.0, 2.0),
    "detour": (3.0, 0.8, 15.0),              # per mm over direct
    "end_off_centre": (1.5, 2.0, 5.0),       # per mm off the centre
    "double_entry": (3.0, 0.0, 3.0),
    "off_axis_entry": (1.5, 6.0, 3.0),
    "wrong_side_entry": (4.0, 0.0, 4.0),
    "power_chain_narrow_pin": (10.0, 0.0, 10.0),
    "bend_excess": (0.2, 0.1, 2.0),          # info only: the hand layout has as many (calibration 2026-11-28)
    "acute_angle": (2.5, 0.0, 2.5),
    "free_angle": (1.0, 0.0, 1.0),
    "min_clearance_run": (0.2, 0.15, 2.0),    # per mm
    "decap_far": (3.0, 0.5, 8.0),            # per mm over DECAP_MAX
    "diffpair_skew": (3.0, 1.5, 10.0),       # per mm
    # stock KiCad DRC rows (flat)
    "clearance": (3.0, 0, 3.0), "shorting_items": (8.0, 0, 8.0), "tracks_crossing": (6.0, 0, 6.0),
    "track_dangling": (1.0, 0, 1.0), "unconnected_items": (5.0, 0, 5.0), "hole_clearance": (3.0, 0, 3.0),
    "via_dangling": (1.0, 0, 1.0), "copper_edge_clearance": (3.0, 0, 3.0), "annular_width": (2.0, 0, 2.0),
}
DEFAULT_MIN = (1.0, 0, 1.0)
#: KiCad DRC types that are not copper review items (library / silk housekeeping)
IGNORE_DRC = {"lib_footprint_issues", "lib_footprint_mismatch", "silk_over_copper", "silk_overlap", "silk_edge_clearance",
              "footprint_symbol_mismatch", "footprint_filters_mismatch", "text_height", "text_thickness", "copper_sliver",
              "solder_mask_bridge", "extra_footprint", "missing_footprint", "duplicate_footprints", "item_on_disabled_layer",
              "footprint_type_mismatch", "courtyards_overlap", "missing_courtyard", "malformed_courtyard"}
SEV_RANK = {"error": 0, "warning": 1, "info": 2}


def minutes(kind: str, size: float = 0.0) -> float:
    base, per, cap = MINUTES.get(kind, DEFAULT_MIN)
    return round(min(cap, base + per * max(0.0, size)), 2)


# ---------------------------------------------------------------- s-expression reader
_TOK = re.compile(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()"]+')


def parse_sexpr(text: str) -> list:
    stack: list[list] = [[]]
    for m in _TOK.finditer(text):
        t = m.group()
        if t == "(":
            stack.append([])
        elif t == ")":
            x = stack.pop()
            stack[-1].append(x)
        elif t[0] == '"':
            stack[-1].append(t[1:-1].replace('\\"', '"'))
        else:
            stack[-1].append(t)
    return stack[0][0]


def kids(node, name):
    return [c for c in node if isinstance(c, list) and c and c[0] == name]


def kid(node, name):
    for c in node:
        if isinstance(c, list) and c and c[0] == name:
            return c
    return None


def num(node, name, default=0.0):
    k = kid(node, name)
    return float(k[1]) if k and len(k) > 1 else default


def xy(node, name):
    k = kid(node, name)
    return (float(k[1]), float(k[2])) if k else (0.0, 0.0)


# ---------------------------------------------------------------- board model
class Pad:
    __slots__ = ("ref", "num", "net", "x", "y", "w", "h", "ang", "shape", "layers", "poly", "drill", "fp", "smd", "key",
                 "fp_idx")


class Seg:
    __slots__ = ("a", "b", "w", "layer", "net", "uuid", "arc")


class Via:
    __slots__ = ("x", "y", "size", "drill", "net", "uuid")


class Board:
    pass


def _rot(x, y, ang_deg):
    a = math.radians(ang_deg)
    c, s = math.cos(a), math.sin(a)
    return x * c + y * s, -x * s + y * c


def _pad_poly(shape, w, h, rratio, ang):
    if shape == "circle":
        p = Point(0, 0).buffer(w / 2, 16)
    elif shape == "oval":
        r = min(w, h) / 2
        if w >= h:
            p = LineString([(-(w / 2 - r), 0), (w / 2 - r, 0)]).buffer(r, 8)
        else:
            p = LineString([(0, -(h / 2 - r)), (0, h / 2 - r)]).buffer(r, 8)
    elif shape == "roundrect" and rratio > 0:
        r = min(w, h) * rratio
        p = box(-w / 2 + r, -h / 2 + r, w / 2 - r, h / 2 - r).buffer(r, 4)
    else:
        p = box(-w / 2, -h / 2, w / 2, h / 2)
    c, s = math.cos(math.radians(ang)), math.sin(math.radians(ang))
    return affinity.affine_transform(p, [c, s, -s, c, 0, 0])


def load_board(pcb: Path, pro: Path | None = None) -> Board:
    root = parse_sexpr(Path(pcb).read_text())
    b = Board()
    b.path = Path(pcb)
    lay = kid(root, "layers")
    b.copper = [l[1] for l in lay[1:] if isinstance(l, list) and len(l) > 2 and str(l[1]).endswith(".Cu")]
    b.pads, b.segs, b.vias, b.fps = [], [], [], []
    for fi, fp in enumerate(kids(root, "footprint")):
        fx, fy = xy(fp, "at")
        at = kid(fp, "at")
        fang = float(at[3]) if at and len(at) > 3 else 0.0
        ref = ""
        for pr in kids(fp, "property"):
            if pr[1] == "Reference":
                ref = pr[2]
        if not ref:
            ft = [t for t in kids(fp, "fp_text") if t[1] == "reference"]
            ref = ft[0][2] if ft else f"FP{fi}"
        info = {"ref": ref, "x": fx, "y": fy, "ang": fang, "pads": [], "layer": (kid(fp, "layer") or [0, "F.Cu"])[1],
                "value": ""}
        for pr in kids(fp, "property"):
            if pr[1] == "Value":
                info["value"] = pr[2]
        for pd in kids(fp, "pad"):
            p = Pad()
            p.ref, p.num, p.fp_idx = ref, pd[1], fi
            kind, p.shape = pd[2], pd[3]
            p.smd = kind == "smd"
            lx, ly = xy(pd, "at")
            a = kid(pd, "at")
            p.ang = float(a[3]) if a and len(a) > 3 else 0.0
            gx, gy = _rot(lx, ly, fang)
            p.x, p.y = fx + gx, fy + gy
            sz = kid(pd, "size")
            p.w, p.h = float(sz[1]), float(sz[2] if len(sz) > 2 else sz[1])
            nk = kid(pd, "net")
            p.net = nk[1] if nk and len(nk) > 1 else ""
            ls = kid(pd, "layers")
            names = ls[1:] if ls else []
            p.layers = set()
            for n in names:
                if n == "*.Cu":
                    p.layers |= set(b.copper)
                elif str(n).endswith(".Cu"):
                    p.layers.add(n)
            dr = kid(pd, "drill")
            p.drill = float(dr[1]) if dr and len(dr) > 1 and dr[1] not in ("oval",) else (float(dr[2]) if dr and len(dr) > 2 else 0.0)
            rr = kid(pd, "roundrect_rratio")
            poly = _pad_poly(p.shape, p.w, p.h, float(rr[1]) if rr else 0.25, p.ang)
            p.poly = affinity.translate(poly, p.x, p.y)
            p.key = (ref, p.num)
            b.pads.append(p)
            info["pads"].append(len(b.pads) - 1)
        b.fps.append(info)
    for s in kids(root, "segment") + kids(root, "arc"):
        sg = Seg()
        sg.w = num(s, "width")
        sg.layer = (kid(s, "layer") or [0, ""])[1]
        sg.net = (kid(s, "net") or [0, ""])[1]
        uu = kid(s, "uuid")
        sg.uuid = uu[1] if uu else ""
        sg.arc = s[0] == "arc"
        if sg.arc:
            st, md, en = xy(s, "start"), xy(s, "mid"), xy(s, "end")
            for q0, q1 in _arc_chords(st, md, en):
                s2 = Seg()
                s2.a, s2.b, s2.w, s2.layer, s2.net, s2.uuid, s2.arc = q0, q1, sg.w, sg.layer, sg.net, sg.uuid, True
                b.segs.append(s2)
        else:
            sg.a, sg.b = xy(s, "start"), xy(s, "end")
            b.segs.append(sg)
    for v in kids(root, "via"):
        vv = Via()
        vv.x, vv.y = xy(v, "at")
        vv.size, vv.drill = num(v, "size"), num(v, "drill")
        vv.net = (kid(v, "net") or [0, ""])[1]
        uu = kid(v, "uuid")
        vv.uuid = uu[1] if uu else ""
        b.vias.append(vv)
    b.net_set = {p.net for p in b.pads if p.net} | {s.net for s in b.segs if s.net}
    b.nets = sorted({p.net for p in b.pads if p.net} | {s.net for s in b.segs if s.net})
    b.classes, b.assign, b.patterns, b.rules = _load_pro(pro)
    return b


def _arc_chords(st, md, en, n=8):
    ax, ay = st
    bx, by = md
    cx, cy = en
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if abs(d) < 1e-12:
        return [(st, en)]
    ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d
    uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d
    a0, am, a1 = math.atan2(ay - uy, ax - ux), math.atan2(by - uy, bx - ux), math.atan2(cy - uy, cx - ux)
    r = math.hypot(ax - ux, ay - uy)

    def norm(a):
        while a < 0:
            a += 2 * math.pi
        while a >= 2 * math.pi:
            a -= 2 * math.pi
        return a
    ccw = norm(am - a0) < norm(a1 - a0)
    sweep = norm(a1 - a0) if ccw else -norm(a0 - a1)
    pts = [(ux + r * math.cos(a0 + sweep * i / n), uy + r * math.sin(a0 + sweep * i / n)) for i in range(n + 1)]
    pts[0], pts[-1] = st, en
    return list(zip(pts[:-1], pts[1:]))


def _load_pro(pro):
    classes = {"Default": {"track_width": 0.2, "clearance": 0.2, "via_diameter": 0.6, "via_drill": 0.3}}
    assign, patterns, rules = {}, [], {"min_clearance": 0.0, "min_track_width": 0.0}
    if pro and Path(pro).is_file():
        d = json.loads(Path(pro).read_text())
        ns = d.get("net_settings", {})
        for c in ns.get("classes", []):
            classes[c["name"]] = {k: c.get(k) for k in ("track_width", "clearance", "via_diameter", "via_drill", "diff_pair_width")}
        assign = dict(ns.get("netclass_assignments") or {})
        patterns = [(p["pattern"], p["netclass"]) for p in ns.get("netclass_patterns", [])]
        ds = d.get("board", {}).get("design_settings", {}).get("rules", {})
        rules = {"min_clearance": ds.get("min_clearance", 0.0), "min_track_width": ds.get("min_track_width", 0.0)}
    return classes, assign, patterns, rules


def net_class(b: Board, net: str) -> str:
    if net in b.assign:
        a = b.assign[net]
        return a[0] if isinstance(a, list) else a
    for pat, cls in b.patterns:
        if fnmatch.fnmatchcase(net, pat):
            return cls
    return "Default"


def class_val(b: Board, net: str, key: str) -> float:
    c = b.classes.get(net_class(b, net)) or b.classes["Default"]
    v = c.get(key)
    if v is None:
        v = b.classes["Default"].get(key) or 0.0
    return float(v)


_PAIR = re.compile(r"^(.*?)(?:([_]?)([PN])|([+-]))$")


def pair_partner(net: str) -> str | None:
    m = _PAIR.match(net)
    if not m:
        return None
    base = m.group(1)
    if m.group(4):
        return base + ("-" if m.group(4) == "+" else "+")
    if m.group(2) == "_":
        return base + "_" + ("N" if m.group(3) == "P" else "P")
    return None


def expected_width(b: Board, net: str) -> float:
    """Width the net is meant to be drawn at: the class diff_pair_width for a member of a diff pair (both halves exist
    and the class has its own pair width), else the class track width."""
    w = class_val(b, net, "track_width")
    c = b.classes.get(net_class(b, net)) or {}
    dw = c.get("diff_pair_width")
    q = pair_partner(net)
    if dw and q and q in b.net_set:
        return float(dw)
    return w


def clearance(b: Board, n1: str, n2: str) -> float:
    return max(class_val(b, n1, "clearance"), class_val(b, n2, "clearance"), b.rules.get("min_clearance", 0.0))


# ---------------------------------------------------------------- copper graph per net
class NetGraph:
    """Nodes: track end points (x, y, layer), vias, pads. Edges: track pieces (T joins split) and zero-length links."""

    def __init__(self, b: Board, net: str):
        self.net = net
        self.nid: dict = {}
        self.nodes: list[dict] = []
        self.edges: list[dict] = []
        self.adj: dict[int, list[int]] = defaultdict(list)
        segs = [s for s in b.segs if s.net == net]
        pads = [p for p in b.pads if p.net == net]
        vias = [v for v in b.vias if v.net == net]
        by_layer = defaultdict(list)
        for s in segs:
            by_layer[s.layer].append(s)
        pts_layer = defaultdict(list)
        for s in segs:
            pts_layer[s.layer] += [s.a, s.b]
        for s in segs:
            cuts = [(0.0, s.a), (1.0, s.b)]
            if len(segs) < 1500:
                dx, dy = s.b[0] - s.a[0], s.b[1] - s.a[1]
                L2 = dx * dx + dy * dy
                if L2 > 1e-12:
                    for p in pts_layer[s.layer]:
                        t = ((p[0] - s.a[0]) * dx + (p[1] - s.a[1]) * dy) / L2
                        if 0.002 < t < 0.998:
                            qx, qy = s.a[0] + t * dx, s.a[1] + t * dy
                            if math.hypot(p[0] - qx, p[1] - qy) <= s.w / 2:
                                cuts.append((t, p))
            cuts.sort(key=lambda c: c[0])
            for (t0, p0), (t1, p1) in zip(cuts[:-1], cuts[1:]):
                n0, n1 = self.node(p0, s.layer), self.node(p1, s.layer)
                if n0 != n1:
                    self.edge(n0, n1, math.dist(p0, p1), s.w, s.layer, s)
        self.vias = []
        for v in vias:
            vn = self.node((v.x, v.y), "V", kind="via", obj=v)
            self.vias.append(vn)
            for key, i in list(self.nid.items()):
                if key[0] != "P" and key[2] != "V" and math.hypot(self.nodes[i]["x"] - v.x, self.nodes[i]["y"] - v.y) <= v.size / 2 + 1e-4:
                    self.link(vn, i)
        self.pad_nodes: dict = {}
        for p in pads:
            pn = self.node(("P", p.key), "P", kind="pad", obj=p, xy_=(p.x, p.y))
            self.pad_nodes[p.key] = pn
            bounds = p.poly.bounds
            pp = p.poly.buffer(1e-3)
            for key, i in list(self.nid.items()):
                if key[0] == "P":
                    continue
                n = self.nodes[i]
                if n["kind"] == "end":
                    if n["layer"] not in p.layers:
                        continue
                elif n["kind"] != "via":
                    continue
                if bounds[0] - 0.01 <= n["x"] <= bounds[2] + 0.01 and bounds[1] - 0.01 <= n["y"] <= bounds[3] + 0.01 and pp.contains(Point(n["x"], n["y"])):
                    self.link(pn, i)

    def node(self, p, layer, kind="end", obj=None, xy_=None):
        key = (round(p[0], 3), round(p[1], 3), layer) if kind == "end" else (("P", p[1]) if kind == "pad" else ("V", round(p[0], 3), round(p[1], 3)))
        if kind == "pad":
            key = ("P", p[1])
        elif kind == "via":
            key = ("V", round(p[0], 3), round(p[1], 3), "V")
        if key in self.nid:
            return self.nid[key]
        i = len(self.nodes)
        x, y = (xy_ or p)[0], (xy_ or p)[1]
        self.nodes.append({"x": x, "y": y, "layer": layer, "kind": kind, "obj": obj})
        self.nid[key] = i
        return i

    def edge(self, a, b, length, w, layer, seg):
        e = len(self.edges)
        self.edges.append({"a": a, "b": b, "len": length, "w": w, "layer": layer, "seg": seg, "link": False})
        self.adj[a].append(e)
        self.adj[b].append(e)

    def link(self, a, b):
        e = len(self.edges)
        self.edges.append({"a": a, "b": b, "len": 0.0, "w": 0.0, "layer": "", "seg": None, "link": True})
        self.adj[a].append(e)
        self.adj[b].append(e)

    def other(self, e, n):
        ed = self.edges[e]
        return ed["b"] if ed["a"] == n else ed["a"]

    def track_degree(self, n):
        return sum(1 for e in self.adj[n] if not self.edges[e]["link"])

    def dijkstra(self, src, through_pads=False):
        dist, prev = {src: 0.0}, {}
        h = [(0.0, src)]
        while h:
            d, n = heapq.heappop(h)
            if d > dist.get(n, 1e18):
                continue
            if n != src and self.nodes[n]["kind"] == "pad" and not through_pads:
                continue
            for e in self.adj[n]:
                m = self.other(e, n)
                nd = d + self.edges[e]["len"]
                if nd < dist.get(m, 1e18) - 1e-12:
                    dist[m], prev[m] = nd, (n, e)
                    heapq.heappush(h, (nd, m))
        return dist, prev

    def path(self, prev, dst):
        nodes, edges = [dst], []
        while nodes[-1] in prev:
            n, e = prev[nodes[-1]]
            edges.append(e)
            nodes.append(n)
        nodes.reverse()
        edges.reverse()
        return nodes, edges


class Conn:
    pass


def connections(g: NetGraph, max_pads=60):
    """MST of pad pairs by routed path length (paths do not pass through a third pad)."""
    pads = list(g.pad_nodes.items())
    if len(pads) < 2 or len(pads) > max_pads:
        return []
    cand = []
    for key, pn in pads:
        dist, prev = g.dijkstra(pn)
        for key2, pn2 in pads:
            if key2 > key and pn2 in dist:
                cand.append((dist[pn2], key, key2, pn, pn2, prev))
    cand.sort(key=lambda c: c[0])
    uf = {k: k for k, _ in pads}

    def f(k):
        while uf[k] != k:
            uf[k] = uf[uf[k]]
            k = uf[k]
        return k
    out = []
    for d, k1, k2, n1, n2, prev in cand:
        if f(k1) == f(k2):
            continue
        uf[f(k1)] = f(k2)
        c = Conn()
        c.net, c.a, c.b, c.length = g.net, k1, k2, d
        # prev holds the tree from n1; path n1 -> n2
        c.nodes, c.edges = g.path(prev, n2)
        pa, pb = g.nodes[n1]["obj"], g.nodes[n2]["obj"]
        c.direct = math.hypot(pa.x - pb.x, pa.y - pb.y)
        c.name = f"{k1[0]}.{k1[1]}->{k2[0]}.{k2[1]}"
        out.append(c)
    return out


def conn_points(g: NetGraph, c: Conn):
    return [(g.nodes[n]["x"], g.nodes[n]["y"], g.nodes[n]["layer"]) for n in c.nodes]


def headings(pts):
    """per-layer polyline turn list: [(idx, turn_deg, p)] for consecutive distinct points on one layer."""
    out = []
    clean = []
    for p in pts:
        if clean and math.dist(clean[-1][:2], p[:2]) < 1e-6:
            if clean[-1][2] != p[2]:
                clean.append(p)
            continue
        clean.append(p)
    for i in range(1, len(clean) - 1):
        a, b, c = clean[i - 1], clean[i], clean[i + 1]
        if not (a[2] == b[2] == c[2]):
            continue
        h1 = math.atan2(b[1] - a[1], b[0] - a[0])
        h2 = math.atan2(c[1] - b[1], c[0] - b[0])
        t = abs(math.degrees((h2 - h1 + math.pi) % (2 * math.pi) - math.pi))
        out.append((i, t, b))
    return out


def leg_lengths(pts):
    """lengths of the straight legs of a path between bends (turn >= BEND_DEG on one layer)."""
    clean = []
    for p in pts:
        if clean and math.dist(clean[-1][:2], p[:2]) < 1e-6 and clean[-1][2] == p[2]:
            continue
        clean.append(p)
    legs, cur = [], 0.0
    for i in range(1, len(clean)):
        cur += math.dist(clean[i - 1][:2], clean[i][:2])
        bend = False
        if i < len(clean) - 1 and clean[i - 1][2] == clean[i][2] == clean[i + 1][2]:
            a, b, c = clean[i - 1], clean[i], clean[i + 1]
            h1, h2 = math.atan2(b[1] - a[1], b[0] - a[0]), math.atan2(c[1] - b[1], c[0] - b[0])
            bend = abs(math.degrees((h2 - h1 + math.pi) % (2 * math.pi) - math.pi)) >= BEND_DEG
        if bend or i == len(clean) - 1:
            legs.append(cur)
            cur = 0.0
    return legs


def short_leg_run(legs, lim):
    """longest run of consecutive inner legs shorter than lim (the pad-end legs are exits, not part of a staircase)."""
    best = cur = 0
    for l in legs[1:-1]:
        cur = cur + 1 if l < lim else 0
        best = max(best, cur)
    return best


# ---------------------------------------------------------------- rows
def row(kind, severity, desc, items, net="", connection="", size=None, size_label="", source="own", minutes_=None, extra=None):
    r = {"type": kind, "severity": severity, "description": desc,
         "items": [{"description": d, "pos": {"x": round(x, 4), "y": round(y, 4)}, "uuid": u} for d, x, y, u in items],
         "net": net, "connection": connection, "source": source}
    if size is not None:
        r["size_mm"] = round(size, 4)
        r["size_label"] = size_label
    r["minutes"] = minutes(kind, size or 0.0) if minutes_ is None else minutes_
    if extra:
        r.update(extra)
    return r


def pos(r):
    it = r["items"][0] if r["items"] else {"pos": {"x": 0, "y": 0}}
    return it["pos"]["x"], it["pos"]["y"]


# ---------------------------------------------------------------- own checks
class Ctx:
    def __init__(self, b: Board):
        self.b = b
        self.graphs = {n: NetGraph(b, n) for n in b.nets}
        self.conns = {n: connections(g) for n, g in self.graphs.items()}
        self.obst = []   # (geom, net, layers, what)
        for p in b.pads:
            self.obst.append((p.poly, p.net, p.layers, f"pad {p.ref}.{p.num}"))
        for s in b.segs:
            self.obst.append((LineString([s.a, s.b]).buffer(s.w / 2, 4), s.net, {s.layer}, f"track {s.net}"))
        for v in b.vias:
            self.obst.append((Point(v.x, v.y).buffer(v.size / 2, 12), v.net, set(b.copper), f"via {v.net}"))
        self.trees = {}
        for L in b.copper:
            idx = [i for i, o in enumerate(self.obst) if L in o[2]]
            self.trees[L] = (STRtree([self.obst[i][0] for i in idx]), idx)
        self._room = {}

    def room_edge(self, net, g: NetGraph, ei):
        """per-piece [(mid, gapL, gapR, netL, netR)] for a track edge (gap = free copper edge to foreign copper edge)."""
        if (net, ei) in self._room:
            return self._room[(net, ei)]
        e = g.edges[ei]
        n0, n1 = g.nodes[e["a"]], g.nodes[e["b"]]
        L = e["len"]
        npieces = max(1, int(math.ceil(L / PIECE)))
        res = []
        dx, dy = (n1["x"] - n0["x"]) / max(L, 1e-9), (n1["y"] - n0["y"]) / max(L, 1e-9)
        tree, idx = self.trees.get(e["layer"], (None, []))
        for k in range(npieces):
            t0, t1 = k / npieces, (k + 1) / npieces
            p0 = (n0["x"] + (n1["x"] - n0["x"]) * t0, n0["y"] + (n1["y"] - n0["y"]) * t0)
            p1 = (n0["x"] + (n1["x"] - n0["x"]) * t1, n0["y"] + (n1["y"] - n0["y"]) * t1)
            mid = ((p0[0] + p1[0]) / 2, (p0[1] + p1[1]) / 2)
            ln = LineString([p0, p1])
            gl, gr, nl, nr = 9.0, 9.0, "", ""
            if tree is not None and L > 1e-6:
                qb = ln.buffer(ROOM_R)
                for j in tree.query(qb):
                    g2, n2, _, what = self.obst[idx[j]]
                    if n2 == net and n2 != "":
                        continue
                    if g2.distance(ln) > ROOM_R:
                        continue
                    pa, pb = shapely.shortest_line(g2, ln).coords[:2]
                    d = g2.distance(ln)
                    side = dx * (pa[1] - mid[1]) - dy * (pa[0] - mid[0])  # >0: right side in y-down coords
                    gap = d - e["w"] / 2 - clearance(self.b, net, n2)
                    if side >= 0:
                        if gap < gr:
                            gr, nr = gap, what
                    else:
                        if gap < gl:
                            gl, nl = gap, what
            res.append((mid, gl, gr, nl, nr, L / npieces))
        self._room[(net, ei)] = res
        return res


def check_necks(cx: Ctx):
    b, rows = cx.b, []
    for net, g in cx.graphs.items():
        cw = expected_width(b, net)
        neck = [i for i, e in enumerate(g.edges) if not e["link"] and e["w"] < cw - NECK_TOL]
        if not neck:
            continue
        uf = {i: i for i in neck}

        def f(i):
            while uf[i] != i:
                uf[i] = uf[uf[i]]
                i = uf[i]
            return i
        by_node = defaultdict(list)
        for i in neck:
            by_node[g.edges[i]["a"]].append(i)
            by_node[g.edges[i]["b"]].append(i)
        for lst in by_node.values():
            for j in lst[1:]:
                uf[f(j)] = f(lst[0])
        groups = defaultdict(list)
        for i in neck:
            groups[f(i)].append(i)
        for gi in groups.values():
            length = sum(g.edges[i]["len"] for i in gi)
            wmin = min(g.edges[i]["w"] for i in gi)
            fit = 9.0
            worst = None
            for i in gi:
                for mid, gl, gr, nl, nr, pl in cx.room_edge(net, g, i):
                    w = g.edges[i]["w"]
                    fw = w + max(gl, 0.0) + max(gr, 0.0)
                    if fw < fit:
                        fit, worst = fw, mid
            if length < 0.15:
                continue
            fit = min(fit, 9.0)
            if fit >= cw - NECK_TOL:
                kind, sev = "neck_room_fits", "warning"
            elif fit >= wmin + 0.02:
                kind, sev = "neck_room_partial", "warning"
            else:
                kind, sev = "neck_forced", "info"
            ei = gi[len(gi) // 2]
            n0 = g.nodes[g.edges[ei]["a"]]
            where = (n0["x"], n0["y"])
            desc = (f"Neck {wmin:.3f} of class {cw:.3f} mm over {length:.2f} mm; free corridor fits {min(fit, cw + 5):.3f} mm"
                    f" ({'room exists' if kind == 'neck_room_fits' else 'room for more' if kind == 'neck_room_partial' else 'no more room'})")
            rows.append(row(kind, sev, desc, [(f"Track [{net}] width {wmin:.3f}", where[0], where[1], g.edges[ei]["seg"].uuid)],
                            net=net, size=length, size_label="neck length mm",
                            extra={"width_mm": round(wmin, 4), "class_width_mm": cw, "corridor_fit_mm": round(fit, 4)}))
    return rows


def check_hug(cx: Ctx):
    b, rows = cx.b, []
    for net, g in cx.graphs.items():
        runs = defaultdict(float)
        where = {}
        for i, e in enumerate(g.edges):
            if e["link"] or e["len"] < 0.05:
                continue
            for mid, gl, gr, nl, nr, pl in cx.room_edge(net, g, i):
                for gap, other in ((gl, nl), (gr, nr)):
                    if other and abs(gap) <= HUG_TOL or (other and gap < 0):
                        key = other.split()[0:2][-1] if other.startswith("pad") else other
                        runs[key] += pl
                        where.setdefault(key, mid)
        for key, ln in runs.items():
            if ln >= HUG_MIN_LEN:
                m = where[key]
                rows.append(row("min_clearance_run", "info", f"{ln:.2f} mm at exactly the minimum clearance next to {key}",
                                [(f"Track [{net}]", m[0], m[1], "")], net=net, size=ln, size_label="run length mm"))
    return rows


def check_vias(cx: Ctx):
    b, rows = cx.b, []
    for v in b.vias:
        for p in b.pads:
            if p.net != v.net or not p.smd:
                continue
            bx = p.poly.bounds
            if not (bx[0] - v.size <= v.x <= bx[2] + v.size and bx[1] - v.size <= v.y <= bx[3] + v.size):
                continue
            inside = p.poly.contains(Point(v.x, v.y))
            overlaps = p.poly.intersects(Point(v.x, v.y).buffer(v.size / 2))
            if inside or overlaps:
                kind = "via_in_pad" if inside else "via_overlaps_pad"
                rows.append(row(kind, "warning" if inside else "info",
                                f"Via {v.size:.2f}/{v.drill:.2f} {'centre inside' if inside else 'overlaps'} SMD pad {p.ref}.{p.num}",
                                [(f"Via [{v.net}]", v.x, v.y, v.uuid), (f"Pad {p.num} of {p.ref}", p.x, p.y, "")],
                                net=v.net, connection=f"{p.ref}.{p.num}",
                                size=math.hypot(v.x - p.x, v.y - p.y), size_label="via centre to pad centre mm"))
    return rows


def check_detours(cx: Ctx, ratio):
    """Per piece of every connection: pad-to-pad, or pad/junction-to-junction where the net branches (a T shares copper,
    so only the piece between two terminals/junctions has an honest direct distance)."""
    rows = []
    seen = set()
    for net, cl in cx.conns.items():
        g = cx.graphs[net]
        for c in cl:
            cuts = [0] + [i for i, n in enumerate(c.nodes) if 0 < i < len(c.nodes) - 1 and g.nodes[n]["kind"] == "end" and g.track_degree(n) >= 3] + [len(c.nodes) - 1]
            for i0, i1 in zip(cuts[:-1], cuts[1:]):
                es = c.edges[i0:i1]
                key = frozenset(es)
                if key in seen:
                    continue
                seen.add(key)
                na, nb = g.nodes[c.nodes[i0]], g.nodes[c.nodes[i1]]
                direct = math.hypot(na["x"] - nb["x"], na["y"] - nb["y"])
                length = sum(g.edges[e]["len"] for e in es)
                if direct < 0.3:
                    continue
                r, extra = length / direct, length - direct
                if r > ratio and extra > RATIO_MIN_EXTRA:
                    pts = conn_points(g, c)[i0:i1 + 1]
                    ln = LineString([(na["x"], na["y"]), (nb["x"], nb["y"])])
                    apex = max(pts, key=lambda q: ln.distance(Point(q[0], q[1])))
                    rows.append(row("detour", "warning", f"Connection {c.name} piece is {length:.2f} mm for {direct:.2f} mm direct ({r:.2f}x)",
                                    [(f"Connection {c.name} [{net}] apex", apex[0], apex[1], "")], net=net, connection=c.name,
                                    size=extra, size_label="mm over direct",
                                    extra={"ratio": round(r, 3), "length_mm": round(length, 3), "direct_mm": round(direct, 3)}))
    return rows


def _axis(p: Pad):
    """pad long axis unit vector (global) and (long, short)."""
    c, s = math.cos(math.radians(p.ang)), math.sin(math.radians(p.ang))
    ux, uy = (c, -s)  # local +x in global
    vx, vy = (s, c)
    if p.w >= p.h:
        return (ux, uy), max(p.w, p.h), min(p.w, p.h)
    return (vx, vy), max(p.w, p.h), min(p.w, p.h)


def _entry_crossings(g, p, inside, fpc, net):
    """off-axis / wrong-side checks on every track that crosses the outline of pad p (one end in, one out)."""
    rows = []
    (ux, uy), long_, short = _axis(p)
    if long_ / max(short, 1e-6) < ASPECT_MIN or short > FINE_PAD or p.num == "":
        return rows
    conn = f"{p.ref}.{p.num}"
    ring = False
    if fpc and fpc[2] >= 6:
        dx, dy = p.x - fpc[0], p.y - fpc[1]
        dl = math.hypot(dx, dy)
        ring = dl > 1e-6 and abs(dx * ux + dy * uy) / dl > 0.7
    for n in inside:
        for e in g.adj[n]:
            ed = g.edges[e]
            if ed["link"]:
                continue
            m = g.other(e, n)
            if g.nodes[m]["kind"] != "end" or p.poly.contains(Point(g.nodes[m]["x"], g.nodes[m]["y"])):
                continue
            qin = (g.nodes[n]["x"], g.nodes[n]["y"])
            qout = (g.nodes[m]["x"], g.nodes[m]["y"])
            inter = LineString([qout, qin]).intersection(p.poly.boundary)
            if inter.is_empty:
                continue
            cp = inter.centroid
            ex, ey = qin[0] - qout[0], qin[1] - qout[1]
            el = math.hypot(ex, ey) or 1.0
            ex, ey = ex / el, ey / el
            dev = math.degrees(math.acos(min(1.0, abs(ex * ux + ey * uy))))
            # how far the entering track's own line passes from the pad centre (0 for a lane on the axis AND for a
            # 45 deg leg aimed at the centre; > 0 for a lane beside the axis or a corner entry)
            off = abs((p.x - qin[0]) * ey - (p.y - qin[1]) * ex)
            if off > max(0.04, OFFSET_FRAC * short):
                rows.append(row("off_axis_entry", "warning",
                                f"Entry into pad {conn}: the track line misses the pad centre by {off:.3f} mm (heading {dev:.0f} deg off the pad axis)",
                                [(f"Entry [{net}] {conn}", cp.x, cp.y, "")], net=net, connection=conn,
                                size=off, size_label="mm line misses centre",
                                extra={"heading_dev_deg": round(dev, 1), "axis_offset_mm": round(off, 4)}))
            if ring:
                sx = 1 if (p.x - fpc[0]) * ux + (p.y - fpc[1]) * uy >= 0 else -1
                sd = ex * ux * sx + ey * uy * sx
                if sd > SIDE_DOT:
                    straight = sd > 0.95   # straight out of the inner tip: a legal tip exit, listed but nearly free
                    rows.append(row("wrong_side_entry", "info" if straight else "warning", f"Pad {conn} is entered from the body side ({math.degrees(math.acos(min(1.0, sd))):.0f} deg off straight in)",
                                    [(f"Entry [{net}] {conn}", cp.x, cp.y, "")], net=net, connection=conn, extra={"side_dot": round(sd, 3)},
                                    minutes_=0.5 if straight else None))
    return rows


def check_pad_entries(cx: Ctx):
    b, rows = cx.b, []
    cent = {}
    for fi, fp in enumerate(b.fps):
        ps = [b.pads[i] for i in fp["pads"]]
        if ps:
            cent[fi] = (sum(p.x for p in ps) / len(ps), sum(p.y for p in ps) / len(ps), len(ps))
    for net, g in cx.graphs.items():
        for key, pn in g.pad_nodes.items():
            p = g.nodes[pn]["obj"]
            inside = [g.other(e, pn) for e in g.adj[pn] if g.edges[e]["link"]]
            inside = [n for n in inside if g.nodes[n]["kind"] == "end"]
            terms = [n for n in inside if g.track_degree(n) == 1]
            allpts = [(g.nodes[n]["x"], g.nodes[n]["y"]) for n in inside]
            if not inside:
                continue
            rows += _entry_crossings(g, p, inside, cent.get(p.fp_idx), net)
            if not terms:
                continue
            d_centre = min(math.hypot(x - p.x, y - p.y) for x, y in allpts)
            tp = [(g.nodes[n]["x"], g.nodes[n]["y"]) for n in terms]
            seen = []
            for q in tp:
                if all(math.dist(q, s) > DOUBLE_TOL for s in seen):
                    seen.append(q)
            conn = f"{p.ref}.{p.num}"
            if d_centre > CENTRE_TOL:
                rows.append(row("end_off_centre", "warning", f"Trace end {d_centre:.3f} mm off the centre of pad {conn}",
                                [(f"Pad {p.num} of {p.ref}", p.x, p.y, "")], net=net, connection=conn, size=d_centre, size_label="mm off centre"))
            if len(seen) >= 2:
                rows.append(row("double_entry", "warning", f"{len(seen)} trace ends inside pad {conn} (second end {max(math.dist((p.x, p.y), q) for q in seen):.2f} mm off the centre)",
                                [(f"Pad {p.num} of {p.ref}", p.x, p.y, "")], net=net, connection=conn, size=len(seen), size_label="ends"))
    return rows


def check_power_chain(cx: Ctx):
    b, rows = cx.b, []
    for net, g in cx.graphs.items():
        for key, pn in g.pad_nodes.items():
            p = g.nodes[pn]["obj"]
            inside = [g.other(e, pn) for e in g.adj[pn] if g.edges[e]["link"]]
            ws = []
            for n in inside:
                for e in g.adj[n]:
                    if not g.edges[e]["link"]:
                        ws.append(g.edges[e]["w"])
            if not ws:
                continue
            short = min(p.w, p.h)
            wref = max(max(ws), class_val(b, net, "track_width"))
            if short >= POWER_PIN_FRAC * wref:
                continue
            # does removing the pad (and its in-pad ends) split the net with other pads on both sides?
            blob = {pn, *inside}
            seen, comps = set(), []
            starts = [g.other(e, k) for k in blob for e in g.adj[k] if not g.edges[e]["link"] and g.other(e, k) not in blob]
            for n in starts:
                if n in seen:
                    continue
                comp, st = set(), [n]
                while st:
                    k = st.pop()
                    if k in comp or k in blob:
                        continue
                    comp.add(k)
                    for e in g.adj[k]:
                        st.append(g.other(e, k))
                seen |= comp
                if any(g.nodes[k]["kind"] == "pad" for k in comp):
                    comps.append(comp)
            if len(comps) >= 2:
                conn = f"{p.ref}.{p.num}"
                rows.append(row("power_chain_narrow_pin", "warning",
                                f"Net {net} (width {wref:.2f} mm) is chained through pin {conn} that is {short:.2f} mm wide",
                                [(f"Pad {p.num} of {p.ref}", p.x, p.y, "")], net=net, connection=conn, size=wref - short, size_label="trace minus pin width mm"))
    return rows


def check_shape(cx: Ctx):
    """acute spikes, free-angle copper, bends vs minimal estimate."""
    rows = []
    for net, cl in cx.conns.items():
        g = cx.graphs[net]
        for c in cl:
            pts = conn_points(g, c)
            tl = headings(pts)
            for i, t, p in tl:
                if t >= 180.0 - ACUTE_DEG:
                    rows.append(row("acute_angle", "warning", f"Turn of {t:.0f} deg (interior {180 - t:.0f} deg) in {c.name}",
                                    [(f"Vertex [{net}]", p[0], p[1], "")], net=net, connection=c.name, size=t, size_label="turn deg"))
            fl = 0.0
            fpos = None
            for e in c.edges:
                ed = g.edges[e]
                if ed["link"] or ed["seg"] is None or ed["seg"].arc:
                    continue
                a, b2 = g.nodes[ed["a"]], g.nodes[ed["b"]]
                h = math.degrees(math.atan2(b2["y"] - a["y"], b2["x"] - a["x"])) % 45.0
                off = min(h, 45.0 - h)
                if off > FREE_ANGLE_TOL and ed["len"] > 0.02:
                    fl += ed["len"]
                    fpos = fpos or ((a["x"] + b2["x"]) / 2, (a["y"] + b2["y"]) / 2)
            if fl >= FREE_ANGLE_MIN:
                rows.append(row("free_angle", "info", f"{fl:.2f} mm of free-angle copper in {c.name}", [(f"Connection {c.name} [{net}]", fpos[0], fpos[1], "")],
                                net=net, connection=c.name, size=fl, size_label="mm"))
            nb = sum(1 for i, t, p in tl if t >= BEND_DEG)
            pa, pb = g.nodes[c.nodes[0]], g.nodes[c.nodes[-1]]
            dx, dy = abs(pb["x"] - pa["x"]), abs(pb["y"] - pa["y"])
            aligned = min(dx, dy) < 0.02 or abs(dx - dy) < 0.02
            vias = sum(1 for n in c.nodes if g.nodes[n]["kind"] == "via")
            minb = (0 if aligned else 1) + 1 + 2 * vias
            exc = nb - max(minb, 2) - 3
            if exc >= 1:
                mid = pts[len(pts) // 2]
                rows.append(row("bend_excess", "info", f"{nb} bends in {c.name}; a minimal route needs about {max(minb, 2)}",
                                [(f"Connection {c.name} [{net}]", mid[0], mid[1], "")], net=net, connection=c.name, size=exc, size_label="bends over estimate",
                                extra={"bends": nb, "bends_min_est": max(minb, 2)}))
    return rows


def check_decap(cx: Ctx):
    b, rows = cx.b, []
    for fp in b.fps:
        if not re.match(r"^C\d+", fp["ref"]) or len(fp["pads"]) != 2:
            continue
        ps = [b.pads[i] for i in fp["pads"]]
        gnd = [p for p in ps if "GND" in p.net.upper()]
        pw = [p for p in ps if p not in gnd and p.net]
        if len(gnd) != 1 or len(pw) != 1:
            continue
        p = pw[0]
        ics = [q for q in b.pads if q.net == p.net and re.match(r"^(U|IC)\d+", q.ref)]
        if not ics:
            continue
        q = min(ics, key=lambda q: math.hypot(q.x - p.x, q.y - p.y))
        d = math.hypot(q.x - p.x, q.y - p.y)
        if d > DECAP_MAX:
            rows.append(row("decap_far", "info", f"Capacitor {fp['ref']} is {d:.1f} mm from its nearest {p.net} pin {q.ref}.{q.num}",
                            [(f"Pad {p.num} of {p.ref}", p.x, p.y, "")], net=p.net, connection=f"{p.ref}.{p.num}->{q.ref}.{q.num}",
                            size=d - DECAP_MAX, size_label="mm over limit", extra={"distance_mm": round(d, 2)}))
    return rows


def check_diffpairs(cx: Ctx):
    b, rows = cx.b, []
    tot = {n: sum(math.dist(s.a, s.b) for s in b.segs if s.net == n) for n in b.nets}
    done = set()
    for n in b.nets:
        m = _PAIR.match(n)
        if not m or tot.get(n, 0) <= 0:
            continue
        base = m.group(1)
        if m.group(4):
            other = base + ("-" if m.group(4) == "+" else "+")
        elif m.group(2) == "_":
            other = base + "_" + ("N" if m.group(3) == "P" else "P")
        else:
            continue
        if other not in tot or tuple(sorted((n, other))) in done:
            continue
        done.add(tuple(sorted((n, other))))
        sk = abs(tot[n] - tot[other])
        if sk > SKEW_MAX:
            s0 = next(s for s in b.segs if s.net == n)
            rows.append(row("diffpair_skew", "info", f"Pair {n} / {other}: lengths {tot[n]:.2f} / {tot[other]:.2f} mm (skew {sk:.2f})",
                            [(f"Track [{n}]", s0.a[0], s0.a[1], s0.uuid)], net=n, size=sk, size_label="skew mm"))
    return rows


def own_checks(b: Board, ratio=RATIO):
    cx = Ctx(b)
    rows = []
    for fn in (check_necks, check_hug, check_vias, check_pad_entries, check_power_chain, check_shape, check_decap, check_diffpairs):
        rows += fn(cx)
    rows += check_detours(cx, ratio)
    return rows, cx


# ---------------------------------------------------------------- generated review rules + kicad-cli
def review_rules(b: Board, cx: Ctx, ratio=RATIO) -> str:
    out = ["(version 1)", "# generated review rules: warning only, review only, never in the user's project"]
    for cname, c in b.classes.items():
        w = c.get("track_width")
        if w and c.get("diff_pair_width"):
            w = min(w, c["diff_pair_width"])   # pair members are meant to be drawn at the pair width
        if w and any(net_class(b, n) == cname for n in b.nets):
            out.append(f'(rule "rv_neck_{cname.replace(" ", "_")}" (constraint track_width (min {w:g}mm)) '
                       f'(condition "A.NetClass == \'{cname}\'") (severity warning))')
    out.append('(rule "rv_short_seg" (constraint track_segment_length (min 0.1mm)) (severity warning))')
    out.append('(rule "rv_via_in_pad" (constraint physical_clearance (min 0.001mm)) '
               '(condition "A.Type == \'Via\' && B.Type == \'Pad\' && A.NetName == B.NetName") (severity warning))')
    for net, g in cx.graphs.items():
        if "'" in net or len(g.pad_nodes) < 2 or not any(not e["link"] for e in g.edges):
            continue
        cl = cx.conns.get(net, [])
        if not cl or len(g.pad_nodes) > 12:
            continue
        direct = sum(c.direct for c in cl)
        if direct < 0.3:
            continue
        out.append(f'(rule "rv_len_{len(out)}" (constraint length (max {direct * ratio:.3f}mm)) '
                   f'(condition "A.NetName == \'{net}\'") (severity warning))')
    return "\n".join(out) + "\n"


def run_kicad_drc(pcb: Path, pro: Path | None, dru_text: str, work: Path, keep_dru: Path | None = None):
    from harness import kicad_env
    work.mkdir(parents=True, exist_ok=True)
    shutil.copy(pcb, work / "board.kicad_pcb")
    if pro and pro.is_file():
        shutil.copy(pro, work / "board.kicad_pro")
    user_dru = pcb.with_suffix(".kicad_dru")
    pre = ""
    if user_dru.is_file():
        pre = user_dru.read_text().replace("(version 1)", "") + "\n"
    (work / "board.kicad_dru").write_text(dru_text.replace("(version 1)\n", "(version 1)\n" + pre, 1))
    out = work / "drc.json"
    r = kicad_env.run(["kicad-cli", "pcb", "drc", "--format", "json", "--severity-all", "--units", "mm", "-o", str(out),
                       str(work / "board.kicad_pcb")], capture_output=True, text=True)
    if not out.is_file():
        raise RuntimeError(f"kicad-cli drc failed rc={r.returncode}: {r.stderr[-400:]}")
    return json.loads(out.read_text())


_RULE_KIND = [("rv_neck", "track_width_rule"), ("rv_short_seg", "short_segment_rule"), ("rv_via_in_pad", "via_in_pad_rule"),
              ("rv_len", "length_rule")]
_NET = re.compile(r"\[([^\]]+)\]")


def drc_rows(d: dict) -> list[dict]:
    out = []
    for v in d.get("violations", []):
        t = v["type"]
        if t in IGNORE_DRC or v.get("excluded"):
            continue
        desc = v["description"]
        src = "kicad"
        m = re.search(r"rule '(rv_[A-Za-z0-9_]+)'", desc)
        kind = t
        if m:
            src = "rule"
            for pre, k in _RULE_KIND:
                if m.group(1).startswith(pre):
                    kind = k
            if kind == "via_in_pad_rule":
                a = re.search(r"actual ([0-9.,]+) mm", desc)
                if a and float(a.group(1).replace(",", ".")) > 0.0005:
                    continue
        nets = []
        for it in v.get("items", []):
            nets += _NET.findall(it.get("description", ""))
        r = {"type": kind, "severity": v["severity"], "description": desc, "items": v.get("items", []),
             "net": nets[0] if nets else "", "nets": sorted(set(nets)), "connection": "", "source": src}
        a = re.search(r"actual (-?[0-9.,]+) mm", desc)
        if a and src == "kicad":
            r["size_mm"] = float(a.group(1).replace(",", "."))
            r["size_label"] = "actual mm"
        r["minutes"] = 0.0 if src == "rule" else minutes(kind)
        out.append(r)
    return out


def dedupe_rule_rows(rows: list[dict]) -> None:
    """Rule rows that restate an own row (same net, within 1 mm): minutes 0 (own row carries the cost)."""
    own = defaultdict(list)
    for r in rows:
        if r["source"] == "own":
            own[(r["net"], r["type"])].append(pos(r))
    pair = {"track_width_rule": ("neck_room_fits", "neck_room_partial", "neck_forced"), "via_in_pad_rule": ("via_in_pad", "via_overlaps_pad"),
            "length_rule": ("detour",), "short_segment_rule": ()}
    for r in rows:
        if r["source"] != "rule":
            continue
        x, y = pos(r)
        r["restates_own"] = any(math.hypot(x - qx, y - qy) < 1.5 for k in pair.get(r["type"], ()) for qx, qy in own.get((r["net"], k), []))
        if r["type"] == "length_rule":
            r["restates_own"] = any(own.get((r["net"], k)) for k in pair["length_rule"])
        r["minutes"] = 0.0 if r["restates_own"] else 0.5


# ---------------------------------------------------------------- result.json naming
def name_connections(rows, result: dict | None):
    if not result:
        return
    conns = (result.get("route") or {}).get("connections") or []
    by = {}
    for c in conns:
        f, t = (c.get("from") or {}).get("label"), (c.get("to") or {}).get("label")
        if f and t:
            by[frozenset((f, t))] = c.get("id")
    for r in rows:
        m = re.match(r"^(\S+)\.(\S+)->(\S+)\.(\S+)$", r.get("connection", "") or "")
        if m:
            cid = by.get(frozenset((f"{m.group(1)}.{m.group(2)}", f"{m.group(3)}.{m.group(4)}")))
            if cid is not None:
                r["topor_conn"] = cid


# ---------------------------------------------------------------- recall against ground-truth spots
def recall(rows, spots) -> dict:
    res = []
    for sp in spots["spots"]:
        hits = []
        for r in rows:
            if r["minutes"] <= 0 and r["source"] == "rule":
                continue
            x, y = pos(r)
            if r["type"] not in sp["kinds"] and not (r["source"] == "rule" and r["type"] + "" in sp["kinds"]):
                continue
            near = any(math.hypot(it["pos"]["x"] - sp["x"], it["pos"]["y"] - sp["y"]) <= sp["r"] for it in r["items"]) if r["items"] else False
            if not near:
                continue
            nets = r.get("nets") or [r.get("net", "")]
            if sp.get("net") and not any(sp["net"] in n for n in nets):
                continue
            hits.append(r)
        res.append({"id": sp["id"], "what": sp["what"], "found": bool(hits), "kinds": sorted({h["type"] for h in hits}),
                    "rows": len(hits)})
    return {"spots": res, "recall": sum(s["found"] for s in res) / max(1, len(res))}


# ---------------------------------------------------------------- driver
def run(pcb: Path, out: Path, pro: Path | None = None, result: Path | None = None, drc_json: Path | None = None,
        no_drc: bool = False, ratio: float = RATIO, spots: Path | None = None, threads_note: str = "") -> dict:
    pcb = Path(pcb)
    if pro is None:
        for cand in (pcb.with_suffix(".kicad_pro"), *sorted(pcb.parent.glob("*.kicad_pro"))[:1]):
            if cand.is_file():
                pro = cand
                break
    b = load_board(pcb, pro)
    rows, cx = own_checks(b, ratio)
    notes = []
    drc = None
    if drc_json:
        drc = json.loads(Path(drc_json).read_text())
    elif not no_drc:
        dru = review_rules(b, cx, ratio)
        out.mkdir(parents=True, exist_ok=True)
        (out / "review.kicad_dru").write_text(dru)
        drc = run_kicad_drc(pcb, pro, dru, Path(tempfile.mkdtemp(prefix="copper-check-")))
    if drc:
        dr = drc_rows(drc)
        capped = sorted({r["type"] for r in dr if sum(1 for q in dr if q["type"] == r["type"]) >= 199})
        if capped:
            notes.append(f"KiCad DRC caps a type at 199 rows: {capped}")
        rows += dr
    res = None
    if result and Path(result).is_file():
        res = json.loads(Path(result).read_text())
    name_connections(rows, res)
    dedupe_rule_rows(rows)
    rows.sort(key=lambda r: (-r["minutes"], SEV_RANK.get(r["severity"], 3), r["type"]))
    for i, r in enumerate(rows):
        r["rank"] = i + 1
    by = defaultdict(lambda: [0, 0.0])
    for r in rows:
        by[r["type"]][0] += 1
        by[r["type"]][1] += r["minutes"]
    doc = {"board": str(pcb), "total_cleanup_min": round(sum(r["minutes"] for r in rows), 1), "rows_total": len(rows),
           "by_type": {k: {"rows": v[0], "minutes": round(v[1], 1)} for k, v in sorted(by.items(), key=lambda kv: -kv[1][1])},
           "notes": notes, "violations": rows,
           "dropped_constraints": ["track_angle: parses in KiCad 10.0.8, never fires (probed 0-179 deg)"]}
    if spots:
        doc["recall"] = recall(rows, json.loads(Path(spots).read_text()))
    out.mkdir(parents=True, exist_ok=True)
    (out / "copper-check.json").write_text(json.dumps(doc, indent=1))
    (out / "copper-check.md").write_text(to_md(doc))
    return doc


def to_md(doc, top=60) -> str:
    L = [f"# copper-check: {Path(doc['board']).name}", "",
         f"Estimated hand cleanup: **{doc['total_cleanup_min']} min** over {doc['rows_total']} rows (minutes are ranking guesses, MINUTES table in harness/copper_check.py).", "",
         "| kind | rows | minutes |", "|---|---|---|"]
    for k, v in doc["by_type"].items():
        L.append(f"| {k} | {v['rows']} | {v['minutes']} |")
    L += ["", f"## Top {top}", "", "| # | kind | sev | net | connection | x | y | size | min | src |", "|---|---|---|---|---|---|---|---|---|---|"]
    for r in doc["violations"][:top]:
        x, y = pos(r)
        sz = f"{r['size_mm']:.3f} {r.get('size_label', '')}" if "size_mm" in r else ""
        L.append(f"| {r['rank']} | {r['type']} | {r['severity']} | {r['net']} | {r['connection']} | {x:.3f} | {y:.3f} | {sz} | {r['minutes']} | {r['source']} |")
    if doc.get("recall"):
        L += ["", f"## Recall {doc['recall']['recall']:.0%}", "", "| spot | found | kinds | rows |", "|---|---|---|---|"]
        for s in doc["recall"]["spots"]:
            L.append(f"| {s['id']} {s['what']} | {'YES' if s['found'] else 'NO'} | {', '.join(s['kinds'])} | {s['rows']} |")
    for n in doc.get("notes", []):
        L.append(f"\nNote: {n}")
    return "\n".join(L) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("pcb")
    ap.add_argument("--out", required=True)
    ap.add_argument("--pro")
    ap.add_argument("--result")
    ap.add_argument("--drc")
    ap.add_argument("--no-drc", action="store_true")
    ap.add_argument("--ratio", type=float, default=RATIO)
    ap.add_argument("--spots")
    a = ap.parse_args(argv)
    doc = run(Path(a.pcb), Path(a.out), Path(a.pro) if a.pro else None, Path(a.result) if a.result else None,
              Path(a.drc) if a.drc else None, a.no_drc, a.ratio, Path(a.spots) if a.spots else None)
    print(f"copper-check: {doc['rows_total']} rows, ~{doc['total_cleanup_min']} min cleanup -> {a.out}/copper-check.md")
    if doc.get("recall"):
        print(f"recall {doc['recall']['recall']:.0%}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
