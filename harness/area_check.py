"""Whole-area check of routed copper (M3 merge gate, user 2026-11-14): the measures that see what a crop image shows,
not only what one connection is charged with.

Per board, per connection and per crop (any shapely region):

- **DRC near**: every KiCad DRC violation located in the area, whoever it is charged to. The place of a violation is
  the midpoint of the closest points of its two items (parsed from the board by uuid), else the items' positions.
  Copper-related types only (``ROUTING_TYPES``; silk, courtyard and library checks are left out).
- **Clearance deficit** (graded): every track is sampled every ``STEP`` mm; at each sample the gap to the nearest
  other-net copper on its layer (tracks centre line less both half widths, pads and vias as shapes) against the
  pair's clearance (netclass, the larger of the two). ``deficit_mm2`` = the integral of ``max(0, required - gap)``
  along the tracks (mm x mm; a 0.1 mm deficit over 1 mm of track is 0.1), ``deficit_len_mm`` the length with any
  deficit. Both sides of a pair count (the area does not know who is to blame).
- **Same-net excess**: track length covered by same-net copper that is not its own neighbourhood along the copper
  (two same-net tracks laid over each other, a merged blob; ``excess_len_mm``, ``excess_mm2``), and
  ``spare_len_mm``: same-net track length on no connection's copper path (loops, dangling stubs).
- **Any-angle**: count and length of track segments whose direction is not a multiple of 45 degrees (0.5 degree
  tolerance, segments over 0.05 mm).
- **ERBS** (from the engine's ERBS dumps, :mod:`harness.erbs_accept`): length over band and reversals, per board.

Usage:

    python -m harness.area_check RUN [--vs RUN2] [--conn NAME:FROM:TO ...] [--crop x0,y0,x1,y1 ...] [--out f.json]

RUN is a score_runs seed directory (``.../s0``, routed board under ``work/*/routed``, ``reports/*/2*/result.json``).
Complexity: O(samples x neighbours) with STR trees; esp-rust-board about 10 s.
"""
from __future__ import annotations

import argparse
import heapq
import json
import math
import re
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
import shapely
import shapely.ops
from shapely.geometry import LineString, Point, Polygon, box
from shapely.strtree import STRtree

STEP = 0.025
ROUTING_TYPES = {
    "clearance", "shorting_items", "tracks_crossing", "hole_clearance", "copper_edge_clearance", "track_dangling",
    "via_dangling", "track_width", "annular_width", "via_diameter", "solder_mask_bridge", "connection_width",
    "hole_to_hole", "drill_out_of_range", "items_not_allowed", "track_angle", "track_segment_length",
}


# --------------------------------------------------------------------------- s-expressions

_TOK = re.compile(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()"]+')


def sexpr(text: str):
    """Parse a KiCad s-expression into nested lists (strings unquoted). O(len(text))."""
    stack: list = [[]]
    for m in _TOK.finditer(text):
        t = m.group(0)
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


def _rot(x, y, deg):
    """KiCad's rotation (y down, positive = counter-clockwise on screen)."""
    r = math.radians(deg)
    c, s = math.cos(r), math.sin(r)
    return x * c + y * s, -x * s + y * c


# --------------------------------------------------------------------------- board copper

class Copper:
    """Tracks, vias and pads of a board, per layer, with uuids and nets (mm, KiCad frame)."""

    def __init__(self):
        self.objs: list[dict] = []
        self.layers: list[str] = []
        self.clear: dict[str, float] = {}
        self.default_clear = 0.2

    def clearance(self, a: str, b: str) -> float:
        return max(self.clear.get(a, self.default_clear), self.clear.get(b, self.default_clear))

    @staticmethod
    def from_pcb(pcb: Path, pro: Path | None = None) -> "Copper":
        cu = Copper()
        root = sexpr(pcb.read_text())
        layers = kid(root, "layers")
        if layers:
            cu.layers = [c[1] for c in layers[1:] if isinstance(c, list) and len(c) > 1 and str(c[1]).endswith(".Cu")]
        order = {n: i for i, n in enumerate(cu.layers)}
        netname = {}
        for n in kids(root, "net"):
            if len(n) >= 3:
                netname[n[1]] = n[2]

        def net_of(node):
            n = kid(node, "net")
            if not n or len(n) < 2:
                return ""
            return n[2] if len(n) >= 3 else netname.get(n[1], n[1])

        for s in kids(root, "segment") + kids(root, "arc"):
            st, en = kid(s, "start"), kid(s, "end")
            pts = [(float(st[1]), float(st[2]))]
            if s[0] == "arc" and kid(s, "mid"):
                md = kid(s, "mid")
                pts.append((float(md[1]), float(md[2])))
            pts.append((float(en[1]), float(en[2])))
            w = float(kid(s, "width")[1])
            lay = kid(s, "layer")[1]
            uid = (kid(s, "uuid") or [None, None])[1]
            for a, b in zip(pts, pts[1:]):
                cu.objs.append(dict(kind="track", layer=lay, net=net_of(s), a=a, b=b, w=w, uuid=uid))
        for v in kids(root, "via"):
            at = kid(v, "at")
            d = float(kid(v, "size")[1])
            ls = kid(v, "layers")[1:]
            if len(ls) == 2 and ls[0] in order and ls[1] in order:
                i, j = sorted((order[ls[0]], order[ls[1]]))
                ls = cu.layers[i:j + 1]
            for lay in ls:
                cu.objs.append(dict(kind="via", layer=lay, net=net_of(v), c=(float(at[1]), float(at[2])), w=d, uuid=(kid(v, "uuid") or [None, None])[1]))
        for fp in kids(root, "footprint"):
            fat = kid(fp, "at")
            fx, fy = float(fat[1]), float(fat[2])
            frot = float(fat[3]) if len(fat) > 3 else 0.0
            ref = ""
            for p in kids(fp, "property"):
                if len(p) > 2 and p[1] == "Reference":
                    ref = p[2]
            for pd in kids(fp, "pad"):
                kind = pd[2]
                if kind == "np_thru_hole":
                    continue
                shape = pd[3]
                pat = kid(pd, "at")
                px, py = float(pat[1]), float(pat[2])
                prot = float(pat[3]) if len(pat) > 3 else 0.0
                ox, oy = _rot(px, py, frot)
                cx, cy = fx + ox, fy + oy
                sz = kid(pd, "size")
                sw, sh = float(sz[1]), float(sz[2]) if len(sz) > 2 else float(sz[1])
                ls = []
                for lay in (kid(pd, "layers") or [])[1:]:
                    if lay == "*.Cu":
                        ls += cu.layers
                    elif lay == "F&B.Cu":
                        ls += [cu.layers[0], cu.layers[-1]]
                    elif lay.endswith(".Cu"):
                        ls.append(lay)
                if shape == "circle":
                    g = Point(cx, cy).buffer(sw / 2, 16)
                elif shape == "oval":
                    r = min(sw, sh) / 2
                    hx, hy = max(0.0, sw / 2 - r), max(0.0, sh / 2 - r)
                    (ax, ay), (bx, by) = _rot(-hx, -hy, prot), _rot(hx, hy, prot)
                    g = LineString([(cx + ax, cy + ay), (cx + bx, cy + by)]).buffer(r, 16) if (hx or hy) else Point(cx, cy).buffer(r, 16)
                else:
                    corners = [_rot(x, y, prot) for x, y in ((-sw / 2, -sh / 2), (sw / 2, -sh / 2), (sw / 2, sh / 2), (-sw / 2, sh / 2))]
                    g = Polygon([(cx + x, cy + y) for x, y in corners])
                uid = (kid(pd, "uuid") or [None, None])[1]
                for lay in dict.fromkeys(ls):
                    cu.objs.append(dict(kind="pad", layer=lay, net=net_of(pd), geom=g, c=(cx, cy), uuid=uid, label=f"{ref}.{pd[1]}"))
        if pro and pro.exists():
            try:
                p = json.loads(pro.read_text())
                ns = p.get("net_settings", {})
                classes = {c["name"]: float(c.get("clearance", 0.2)) for c in ns.get("classes", [])}
                cu.default_clear = classes.get("Default", 0.2)
                assign = ns.get("netclass_assignments") or {}
                for net, cls in assign.items():
                    c = cls if isinstance(cls, str) else (cls[0] if cls else "Default")
                    cu.clear[net] = classes.get(c, cu.default_clear)
                for pt in ns.get("netclass_patterns") or []:
                    rx = re.compile("^" + re.escape(pt["pattern"]).replace("\\*", ".*").replace("\\?", ".") + "$")
                    for o in cu.objs:
                        if o["net"] not in cu.clear and rx.match(o["net"]):
                            cu.clear[o["net"]] = classes.get(pt["netclass"], cu.default_clear)
                mc = p.get("board", {}).get("design_settings", {}).get("rules", {}).get("min_clearance", 0) or 0
                cu.default_clear = max(cu.default_clear, mc)
            except (ValueError, KeyError, TypeError):
                pass
        cu._finish()
        return cu

    @staticmethod
    def from_model(model: dict, run: dict) -> "Copper":
        """Engine model (pads, rules) + a topo_route run's items (patterns of the judge). nm -> mm."""
        cu = Copper()
        names = {n["code"]: n["name"] for n in model["nets"]}
        lname = {l["id"]: l["name"] for l in model["layers"]}
        cu.layers = [lname[l["id"]] for l in model["layers"]]
        r = model.get("rules", {})
        cu.default_clear = r.get("default", {}).get("clearance", 200000) / 1e6
        for k, v in (r.get("per_net") or {}).items():
            cu.clear[names.get(int(k), k)] = v.get("clearance", 200000) / 1e6
        for it in run.get("items", []):
            pr = it["prim"]
            if pr["kind"] == "seg":
                cu.objs.append(dict(kind="track", layer=lname.get(it["layer"], str(it["layer"])), net=names.get(it["net"], str(it["net"])),
                                    a=(pr["a"][0] / 1e6, pr["a"][1] / 1e6), b=(pr["b"][0] / 1e6, pr["b"][1] / 1e6), w=it["width"] / 1e6, uuid=None))
            elif pr["kind"] == "via":
                for lay in cu.layers:
                    cu.objs.append(dict(kind="via", layer=lay, net=names.get(it["net"], str(it["net"])), c=(pr["at"][0] / 1e6, pr["at"][1] / 1e6), w=it.get("width", 600000) / 1e6, uuid=None))
        for pd in model["pads"]:
            cx, cy = pd["center"][0] / 1e6, pd["center"][1] / 1e6
            rot = pd.get("rotation_deg") or 0.0
            for lid, sh in pd["shapes"].items():
                w, h = sh.get("w", 0) / 1e6, sh.get("h", 0) / 1e6
                if not w:
                    continue
                if sh.get("kind") == "circle":
                    g = Point(cx, cy).buffer(w / 2, 16)
                else:
                    cs = [_rot(x, y, rot) for x, y in ((-w / 2, -h / 2), (w / 2, -h / 2), (w / 2, h / 2), (-w / 2, h / 2))]
                    g = Polygon([(cx + x, cy + y) for x, y in cs])
                cu.objs.append(dict(kind="pad", layer=lname.get(int(lid), lid), net=names.get(pd["net"], "") if pd["net"] else "", geom=g, c=(cx, cy), uuid=pd.get("uuid"), label=f"{pd['ref']}.{pd['number']}"))
        cu._finish()
        return cu

    def _finish(self):
        for i, o in enumerate(self.objs):
            o["i"] = i
            if o["kind"] == "track":
                o["line"] = LineString([o["a"], o["b"]]) if o["a"] != o["b"] else Point(o["a"])
                o["geom"] = o["line"].buffer(o["w"] / 2, 8)
            elif o["kind"] == "via":
                o["geom"] = Point(o["c"]).buffer(o["w"] / 2, 16)
        self.by_uuid = defaultdict(list)
        for o in self.objs:
            if o.get("uuid"):
                self.by_uuid[o["uuid"]].append(o)
        self.per_layer = defaultdict(list)
        for o in self.objs:
            self.per_layer[o["layer"]].append(o)
        self.trees = {l: STRtree([o["geom"] for o in os]) for l, os in self.per_layer.items()}
        self.lines = {l: STRtree([o.get("line", o["geom"]) for o in os]) for l, os in self.per_layer.items()}


# --------------------------------------------------------------------------- samples

def samples(cu: Copper, step: float = STEP):
    """Every track sampled: arrays of x, y, track index, along-position, layer per sample. O(total length / step)."""
    xs, ys, ti, us = [], [], [], []
    for o in cu.objs:
        if o["kind"] != "track":
            continue
        (ax, ay), (bx, by) = o["a"], o["b"]
        L = math.hypot(bx - ax, by - ay)
        n = max(1, int(math.ceil(L / step)))
        t = (np.arange(n) + 0.5) / n
        xs.append(ax + (bx - ax) * t)
        ys.append(ay + (by - ay) * t)
        ti.append(np.full(n, o["i"]))
        us.append(t * L)
    if not xs:
        z = np.zeros(0)
        return z, z, z.astype(int), z
    return np.concatenate(xs), np.concatenate(ys), np.concatenate(ti).astype(int), np.concatenate(us)


def _along(cu: Copper, reach: float = 2.0):
    """Per track end: the along-copper distance (mm) to every same-net track end node within `reach` (endpoint graph;
    the ends inside one same-net via or pad are joined). Returns (node of each track end, per track the two dicts
    node -> distance from its end a / b). O(tracks x local graph)."""
    key = lambda lay, p: (lay, round(p[0], 4), round(p[1], 4))
    g = defaultdict(list)
    tracks = [o for o in cu.objs if o["kind"] == "track"]
    for o in tracks:
        na, nb = key(o["layer"], o["a"]), key(o["layer"], o["b"])
        L = math.dist(o["a"], o["b"])
        g[na].append((nb, L))
        g[nb].append((na, L))
    for l, os in cu.per_layer.items():
        tree = cu.trees[l]
        for o in os:
            if o["kind"] != "track":
                continue
            for p in (o["a"], o["b"]):
                for j in tree.query(Point(p)):
                    q = os[j]
                    if q["kind"] in ("via", "pad") and q["net"] == o["net"] and q["geom"].covers(Point(p)):
                        jn = ("join", q["kind"], q["c"])
                        g[jn].append((key(o["layer"], p), 0.0))
                        g[key(o["layer"], p)].append((jn, 0.0))

    def dij(src):
        dist = {src: 0.0}
        heap = [(0.0, src)]
        while heap:
            d, n = heapq.heappop(heap)
            if d > dist.get(n, 1e18) or d > reach:
                continue
            for m, L in g[n]:
                nd = d + L
                if nd < dist.get(m, 1e18) and nd <= reach:
                    dist[m] = nd
                    heapq.heappush(heap, (nd, m))
        return dist

    cache = {}
    out = {}
    for o in tracks:
        na, nb = key(o["layer"], o["a"]), key(o["layer"], o["b"])
        for n in (na, nb):
            if n not in cache:
                cache[n] = dij(n)
        out[o["i"]] = (na, nb, cache[na], cache[nb])
    return out


def _along_dist(cu, al, me: int, u: float, other: int, x) -> float:
    """Along-copper distance from the point at `u` on track `me` to the point of track `other` nearest `x`."""
    na, nb, da, db = al[me]
    o = cu.objs[other]
    L = math.dist(cu.objs[me]["a"], cu.objs[me]["b"])
    oa, ob, _, _ = al[other]
    best = 1e18
    for end, e in ((oa, o["a"]), (ob, o["b"])):
        t = math.dist(e, x)
        best = min(best, u + da.get(end, 1e18) + t, (L - u) + db.get(end, 1e18) + t)
    return best


def measure_samples(cu: Copper, step: float = STEP):
    """Per sample: deficit (mm), same-net overlap (mm), any-angle flag. Returns a dict of arrays."""
    xs, ys, ti, us = samples(cu, step)
    n = len(xs)
    deficit = np.zeros(n)
    overlap = np.zeros(n)
    al = _along(cu)
    maxc = max([cu.default_clear] + list(cu.clear.values()))
    by_layer = defaultdict(list)
    for k in range(n):
        by_layer[cu.objs[ti[k]]["layer"]].append(k)
    for l, ks in by_layer.items():
        ks = np.array(ks)
        os = cu.per_layer[l]
        pts = shapely.points(xs[ks], ys[ks])
        hw = np.array([cu.objs[t]["w"] / 2 for t in ti[ks]])
        maxw = max(o.get("w", 0) for o in os) if os else 0
        si, oi = cu.lines[l].query(pts, predicate="dwithin", distance=maxc + maxw + 0.05)
        oidx = np.array([o["i"] for o in os])
        me_i = ti[ks[si]]
        keep = oidx[oi] != me_i
        si, oi, me_i = si[keep], oi[keep], me_i[keep]
        if not len(si):
            continue
        is_track = np.array([o["kind"] == "track" for o in os])[oi]
        ow = np.array([o.get("w", 0.0) for o in os])[oi]
        lines = np.array([o.get("line", o["geom"]) for o in os], dtype=object)[oi]
        geoms = np.array([o["geom"] for o in os], dtype=object)[oi]
        p = pts[si]
        dist = shapely.distance(p, lines)
        gap = np.where(is_track, dist - ow / 2, dist) - hw[si]
        # a sample inside a pad or via: as deep as it is inside
        inside = (~is_track) & (dist <= 0)
        if inside.any():
            ext = shapely.boundary(geoms[inside])
            gap[inside] = -hw[si][inside] - shapely.distance(p[inside], ext)
        mynet = np.array([cu.objs[t]["net"] for t in me_i], dtype=object)
        onet = np.array([o["net"] for o in os], dtype=object)[oi]
        other = (mynet != onet) | (mynet == "")
        need = np.array([cu.clearance(a, b) for a, b in zip(mynet[other], onet[other])]) if other.any() else np.zeros(0)
        np.maximum.at(deficit, ks[si[other]], need - gap[other])
        same = (~other) & is_track & (gap < 0)
        for s, j, g in zip(si[same], oi[same], gap[same]):
            # same net, farther along the copper than twice both widths: copper laid over copper (a joint's own
            # overlap is near it along the copper)
            k = ks[s]
            me = int(ti[k])
            o = os[j]
            if _along_dist(cu, al, me, float(us[k]), o["i"], (xs[k], ys[k])) > 2 * (2 * hw[s] + o["w"]):
                overlap[k] = max(overlap[k], min(-g, 2 * hw[s]))
    anyang = np.zeros(n, dtype=bool)
    for o in cu.objs:
        if o["kind"] != "track":
            continue
        (ax, ay), (bx, by) = o["a"], o["b"]
        L = math.hypot(bx - ax, by - ay)
        if L < 0.05:
            continue
        a = math.degrees(math.atan2(by - ay, bx - ax)) % 45.0
        if min(a, 45.0 - a) > 0.5:
            anyang[ti == o["i"]] = True
    return dict(x=xs, y=ys, t=ti, u=us, deficit=deficit, overlap=overlap, anyang=anyang, step=step)


# --------------------------------------------------------------------------- DRC places

def drc_places(cu: Copper, drc: dict):
    """(type, x, y, descriptions) per violation: the midpoint of the closest points of its first two items."""
    out = []
    for v in drc.get("violations", []):
        items = v.get("items", [])
        geoms = []
        for it in items[:2]:
            os = cu.by_uuid.get(it.get("uuid"), [])
            lay = re.search(r" on ([A-Za-z0-9.]+\.Cu)", it.get("description", ""))
            os2 = [o for o in os if not lay or o["layer"] == lay.group(1)] or os
            geoms.append(shapely.union_all([o["geom"] for o in os2]) if os2 else None)
        if len(geoms) == 2 and geoms[0] is not None and geoms[1] is not None:
            p, q = shapely.ops.nearest_points(geoms[0], geoms[1])
            x, y = (p.x + q.x) / 2, (p.y + q.y) / 2
        else:
            ps = [(it["pos"]["x"], it["pos"]["y"]) for it in items if "pos" in it]
            if not ps:
                continue
            x, y = ps[0]
        out.append((v["type"], x, y, " / ".join(i.get("description", "")[:60] for i in items)))
    return out


# --------------------------------------------------------------------------- areas

def _net_graph(cu: Copper, net: str):
    """Copper graph of one net (cached): track end nodes per layer, via / pad terminal nodes (shared by the layers),
    T-joins (an end on another track's interior)."""
    cache = cu.__dict__.setdefault("_graphs", {})
    if net in cache:
        return cache[net]
    key = lambda p: (round(p[0], 4), round(p[1], 4))
    tracks = [o for o in cu.objs if o["kind"] == "track" and o["net"] == net]
    g = defaultdict(list)
    for o in tracks:
        a, b = ("n", o["layer"], key(o["a"])), ("n", o["layer"], key(o["b"]))
        L = math.dist(o["a"], o["b"])
        g[a].append((b, L, o["i"]))
        g[b].append((a, L, o["i"]))
    terms = {}
    if tracks:
        ends = [(t, p) for t in tracks for p in (t["a"], t["b"])]
        etree = STRtree([Point(p) for _, p in ends])
        ltree = STRtree([t["line"] for t in tracks])
        for o in cu.objs:
            if o["net"] != net or o["kind"] not in ("via", "pad"):
                continue
            nid = ("t", o["kind"], key(o["c"]))
            gg = o["geom"].buffer(1e-4)
            for j in etree.query(gg, predicate="covers"):
                t, p = ends[j]
                if t["layer"] != o["layer"]:
                    continue
                n = ("n", t["layer"], key(p))
                g[nid].append((n, 0.0, None))
                g[n].append((nid, 0.0, None))
            terms[nid] = o
        for t, p in ends:
            for j in ltree.query(Point(p), predicate="dwithin", distance=1e-3):
                o = tracks[j]
                if o is t or o["layer"] != t["layer"] or key(p) in (key(o["a"]), key(o["b"])):
                    continue
                n = ("n", t["layer"], key(p))
                for q in (o["a"], o["b"]):
                    m = ("n", o["layer"], key(q))
                    d = math.dist(p, q)
                    g[n].append((m, d, o["i"]))
                    g[m].append((n, d, o["i"]))
    cache[net] = (g, terms)
    return cache[net]


def conn_copper(cu: Copper, net: str, fr, to, vias=()):
    """The tracks of one connection: the shortest same-net copper path between the two terminal points (mm)."""
    g, terms = _net_graph(cu, net)
    if not g:
        return []

    def start_nodes(pt):
        if pt is None:
            return []
        P = Point(pt)
        hits = [n for n, o in terms.items() if o["geom"].buffer(1e-3).covers(P)]
        if hits:
            return hits
        best = min(g, key=lambda n: math.dist(n[2] if n[0] == "n" else n[2], pt))
        return [best] if math.dist(best[2], pt) < 0.05 else []

    src, dst = start_nodes(fr), set(start_nodes(to))
    for v in vias:
        dst |= set(start_nodes(v))
    if not src or not dst:
        return []
    dist = {s: 0.0 for s in src}
    prev = {}
    heap = [(0.0, s) for s in src]
    end = None
    while heap:
        d, n = heapq.heappop(heap)
        if d > dist.get(n, 1e18):
            continue
        if n in dst and n not in src:
            end = n
            break
        for m, L, ti in g[n]:
            nd = d + L
            if nd < dist.get(m, 1e18):
                dist[m] = nd
                prev[m] = (n, ti)
                heapq.heappush(heap, (nd, m))
    if end is None:
        return []
    out = []
    while end in prev:
        end, ti = prev[end]
        if ti is not None:
            out.append(ti)
    return sorted(set(out))


def region_stats(cu: Copper, ms: dict, places, region) -> dict:
    """The measures inside `region` (shapely geometry, mm)."""
    inside = shapely.contains_xy(region, ms["x"], ms["y"]) if len(ms["x"]) else np.zeros(0, dtype=bool)
    st = ms["step"]
    d = ms["deficit"][inside]
    ov = ms["overlap"][inside]
    tr = ms["t"][inside]
    drc = defaultdict(int)
    for typ, x, y, _ in places:
        if typ in ROUTING_TYPES and region.contains(Point(x, y)):
            drc[typ] += 1
    segs = set(tr.tolist())
    aa = {i for i in segs if ms["anyang"][ms["t"] == i].any()}
    return dict(
        drc=sum(drc.values()), drc_types=dict(sorted(drc.items())),
        deficit_mm2=round(float((d[d > 0]).sum() * st), 4), deficit_len_mm=round(float((d > 0).sum() * st), 3),
        worst_deficit_mm=round(float(d.max()) if len(d) else 0.0, 4),
        excess_len_mm=round(float((ov > 0).sum() * st), 3), excess_mm2=round(float(ov.sum() * st), 4),
        anyangle=len(aa), anyangle_len_mm=round(float(ms["anyang"][inside].sum() * st), 3),
        track_len_mm=round(float(inside.sum() * st), 2),
    )


def spare_length(cu: Copper, conns, used: set) -> float:
    """Track length (mm) on no connection path (loops, stubs) over the nets that have connections."""
    nets = {c["name"] for c in conns}
    return round(sum(math.dist(o["a"], o["b"]) for o in cu.objs if o["kind"] == "track" and o["net"] in nets and o["i"] not in used), 3)


# --------------------------------------------------------------------------- runs

def load_run(s0: Path):
    s0 = Path(s0)
    pcb = next(s0.glob("work/*/routed/*.kicad_pcb"))
    pro = pcb.with_suffix(".kicad_pro")
    rep = sorted((s0 / "reports").glob("*/2*"), key=lambda p: p.name)
    res = json.loads((rep[-1] / "result.json").read_text()) if rep else {}
    drcf = next(iter(sorted(s0.glob("*--drc-routed.json"))), None) or (rep[-1] / "drc.json" if rep else None)
    drc = json.loads(drcf.read_text()) if drcf and drcf.exists() else {}
    return pcb, pro, res, drc


class Board:
    """One routed run: copper, per-sample measures, DRC places, connection copper."""

    def __init__(self, s0: Path):
        self.pcb, pro, self.res, drc = load_run(s0)
        self.cu = Copper.from_pcb(self.pcb, pro)
        self.ms = measure_samples(self.cu)
        self.places = drc_places(self.cu, drc)
        self.conns = self.res.get("route", {}).get("connections", [])
        self._cc = {}

    def conn(self, name, fr_label, to_label):
        for c in self.conns:
            if c.get("name") == name and c["from"].get("label") == fr_label and c["to"].get("label") == to_label:
                return c
        return None

    def copper_of(self, c) -> list[int]:
        k = c["id"]
        if k not in self._cc:
            mm = lambda p: (p[0] / 1e6, p[1] / 1e6) if p else None
            vias = [mm(v) for v in (c.get("via_sites") or [])] if c["to"].get("kind") == "pour" else []
            to = None if c["to"].get("kind") == "pour" else mm(c["to"].get("at"))
            self._cc[k] = conn_copper(self.cu, c["name"], mm(c["from"].get("at")), to, vias)
        return self._cc[k]

    def near_area(self, c, reach: float = 1.0):
        ts = self.copper_of(c)
        if not ts:
            return None
        return shapely.union_all([self.cu.objs[i]["line"].buffer(reach + self.cu.objs[i]["w"] / 2, 8) for i in ts])

    def board_totals(self) -> dict:
        allr = box(-1e4, -1e4, 1e4, 1e4)
        t = region_stats(self.cu, self.ms, self.places, allr)
        used = set()
        for c in self.conns:
            if c.get("status") == "routed":
                used |= set(self.copper_of(c))
        t["spare_len_mm"] = spare_length(self.cu, self.conns, used)
        return t


def crop_rect(c, margin_mm=2.0, frac=0.15):
    """The ladder PNG's crop round a connection (ladder.py): its terminals' box, 2 mm + 15 % of the larger side."""
    pts = [c["from"]["at"], c["to"]["at"]]
    x0, x1 = min(p[0] for p in pts) / 1e6, max(p[0] for p in pts) / 1e6
    y0, y1 = min(p[1] for p in pts) / 1e6, max(p[1] for p in pts) / 1e6
    m = margin_mm + frac * max(x1 - x0, y1 - y0)
    return box(x0 - m, y0 - m, x1 + m, y1 + m)


def tiles(b: Board, size=4.0):
    """The board in `size` mm tiles: stats per tile with any track."""
    if not len(b.ms["x"]):
        return {}
    # an absolute grid, so two runs of one board share their tiles
    out = {}
    ix = np.floor(b.ms["x"] / size).astype(int)
    iy = np.floor(b.ms["y"] / size).astype(int)
    for i, j in set(zip(ix.tolist(), iy.tolist())):
        r = box(i * size, j * size, (i + 1) * size, (j + 1) * size)
        out[(round(i * size, 3), round(j * size, 3))] = region_stats(b.cu, b.ms, b.places, r)
    return out


def badness(s: dict) -> float:
    """One graded number per area for ranking (no threshold): DRC items, plus 1 per 0.02 mm2 of clearance deficit,
    1 per 0.02 mm2 of same-net excess, 0.1 per any-angle segment."""
    return s["drc"] + s["deficit_mm2"] / 0.02 + s["excess_mm2"] / 0.02 + 0.1 * s["anyangle"]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("run")
    ap.add_argument("--vs", help="second run (same board) to compare")
    ap.add_argument("--conn", action="append", default=[], help="NAME:FROM:TO (labels)")
    ap.add_argument("--crop", action="append", default=[], help="x0,y0,x1,y1 mm")
    ap.add_argument("--tiles", type=int, default=0, help="list the N worst tiles (4 mm) by badness difference")
    ap.add_argument("--out")
    a = ap.parse_args(argv)
    runs = [Board(Path(a.run))] + ([Board(Path(a.vs))] if a.vs else [])
    rep = {"runs": [str(r.pcb) for r in runs], "board": [r.board_totals() for r in runs], "conns": [], "crops": []}
    for spec in a.conn:
        name, fr, to = spec.split(":")
        row = {"conn": spec}
        for k, r in enumerate(runs):
            c = r.conn(name, fr, to)
            if not c:
                continue
            na = r.near_area(c)
            row[f"near{k}"] = region_stats(r.cu, r.ms, r.places, na) if na is not None else None
            row[f"crop{k}"] = region_stats(r.cu, r.ms, r.places, crop_rect(c))
        rep["conns"].append(row)
    for spec in a.crop:
        x0, y0, x1, y1 = map(float, spec.split(","))
        rep["crops"].append({"crop": spec, **{f"r{k}": region_stats(r.cu, r.ms, r.places, box(x0, y0, x1, y1)) for k, r in enumerate(runs)}})
    if a.tiles and len(runs) == 2:
        t0, t1 = tiles(runs[0]), tiles(runs[1])
        keys = set(t0) | set(t1)
        z = dict(drc=0, drc_types={}, deficit_mm2=0.0, deficit_len_mm=0.0, worst_deficit_mm=0.0, excess_len_mm=0.0, excess_mm2=0.0, anyangle=0, anyangle_len_mm=0.0, track_len_mm=0.0)
        diff = sorted(keys, key=lambda k: -(badness(t1.get(k, z)) - badness(t0.get(k, z))))
        rep["tiles"] = [{"at": k, "r0": t0.get(k, z), "r1": t1.get(k, z), "diff": round(badness(t1.get(k, z)) - badness(t0.get(k, z)), 2)} for k in diff[:a.tiles]]
    txt = json.dumps(rep, indent=1, default=str)
    if a.out:
        Path(a.out).write_text(txt)
    print(txt)
    return 0


if __name__ == "__main__":
    sys.exit(main())
