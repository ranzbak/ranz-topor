"""Small pure geometry helpers shared by the harness exporter and the IPC exporter (integer nm points).

Complexity: ``hull`` O(n log n); ``find_dangling`` O(sum over (net, layer) buckets of items^2) (linear for
the normal case of small nets); ``flatten_arc`` / ``circle_ring`` O(1/sqrt(max_err)); ``point_in_rings``
O(ring points).
"""

from __future__ import annotations

import math

TAU = 2 * math.pi


def hull(pts: list) -> list[list[int]]:
    """Convex hull (Andrew monotone chain), counter-clockwise in a y-up frame."""
    pts = sorted(set(map(tuple, pts)))
    if len(pts) <= 2:
        return [list(p) for p in pts]

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lo: list = []
    for p in pts:
        while len(lo) >= 2 and cross(lo[-2], lo[-1], p) <= 0:
            lo.pop()
        lo.append(p)
    up: list = []
    for p in reversed(pts):
        while len(up) >= 2 and cross(up[-2], up[-1], p) <= 0:
            up.pop()
        up.append(p)
    return [list(p) for p in lo[:-1] + up[:-1]]


# ---------------------------------------------------------------- arcs
def arc_center(s, m, e):
    ax, ay = s
    bx, by = m
    cx, cy = e
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if d == 0:
        return None
    a2, b2, c2 = ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy
    ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d
    uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d
    return ux, uy


def dist_pt_seg(p, a, b) -> float:
    dx, dy = b[0] - a[0], b[1] - a[1]
    L2 = dx * dx + dy * dy
    if L2 == 0:
        return math.hypot(p[0] - a[0], p[1] - a[1])
    t = max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L2))
    return math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy))


def on_arc_sweep(p, c, s, m, e) -> bool:
    """True when the ray c->p lies within the angular sweep s->m->e."""

    def ang(q):
        return math.atan2(q[1] - c[1], q[0] - c[0])

    a_s, a_m, a_e, a_p = ang(s), ang(m), ang(e), ang(p)
    if (a_m - a_s) % TAU <= (a_e - a_s) % TAU:  # counter-clockwise in atan2 frame
        return (a_p - a_s) % TAU <= (a_e - a_s) % TAU
    return (a_s - a_p) % TAU <= (a_s - a_e) % TAU


def dist_pt_item(p, it) -> float:
    if it["arc"] is None:
        return dist_pt_seg(p, it["s"], it["e"])
    c, r = it["arc"]
    if on_arc_sweep(p, c, it["s"], it["m"], it["e"]):
        return abs(math.hypot(p[0] - c[0], p[1] - c[1]) - r)
    return min(math.hypot(p[0] - it["s"][0], p[1] - it["s"][1]),
               math.hypot(p[0] - it["e"][0], p[1] - it["e"][1]))


def direction(it, end_is_e: bool):
    s, e = it["s"], it["e"]
    far = e if end_is_e else s
    near = s if end_is_e else e
    if it["arc"] is None:
        dx, dy = far[0] - near[0], far[1] - near[1]
    else:
        c, _ = it["arc"]
        dx, dy = -(far[1] - c[1]), far[0] - c[0]
        m = it["m"]
        if dx * (far[0] - m[0]) + dy * (far[1] - m[1]) < 0:
            dx, dy = -dx, -dy
    n = math.hypot(dx, dy) or 1.0
    return [dx / n, dy / n]


def flatten_arc(s, m, e, max_err: int) -> list[list[int]]:
    """Points of the arc s->m->e (both ends included), chord error <= max_err (nm). A collinear arc gives
    [s, e]. O(sweep / sqrt(max_err / r))."""
    c = arc_center(s, m, e)
    if c is None:
        return [list(s), list(e)]
    r = math.hypot(s[0] - c[0], s[1] - c[1])
    a_s, a_m, a_e = (math.atan2(q[1] - c[1], q[0] - c[0]) for q in (s, m, e))
    ccw = (a_m - a_s) % TAU <= (a_e - a_s) % TAU
    sweep = ((a_e - a_s) % TAU) if ccw else -((a_s - a_e) % TAU)
    if sweep == 0:
        sweep = TAU if ccw else -TAU
    step = 2 * math.acos(max(-1.0, min(1.0, 1 - max(max_err, 1) / r))) if r > max_err else math.pi / 2
    n = max(2, int(math.ceil(abs(sweep) / max(step, 1e-3))))
    pts = [list(s)]
    for i in range(1, n):
        a = a_s + sweep * i / n
        pts.append([int(round(c[0] + r * math.cos(a))), int(round(c[1] + r * math.sin(a)))])
    pts.append(list(e))
    return pts


def circle_ring(center, r: int, max_err: int) -> list[list[int]]:
    """Regular polygon inscribed in the circle with chord error <= max_err (>= 8 sides). O(n)."""
    r = max(int(r), 1)
    step = 2 * math.acos(max(-1.0, min(1.0, 1 - max(max_err, 1) / r))) if r > max_err else math.pi / 4
    n = max(8, int(math.ceil(TAU / max(step, 1e-3))))
    return [[int(round(center[0] + r * math.cos(TAU * i / n))), int(round(center[1] + r * math.sin(TAU * i / n)))]
            for i in range(n)]


def flatten_bezier(p0, p1, p2, p3, max_err: int) -> list[list[int]]:
    """Cubic Bezier as a polyline (>= 2 points); the segment count is chosen from the control polygon so the
    flatness error is <= max_err. O(n)."""
    dd = max(math.hypot(p0[0] - 2 * p1[0] + p2[0], p0[1] - 2 * p1[1] + p2[1]),
             math.hypot(p1[0] - 2 * p2[0] + p3[0], p1[1] - 2 * p2[1] + p3[1]))
    n = max(2, int(math.ceil(math.sqrt(0.75 * dd / max(max_err, 1)))))
    out = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        out.append([int(round(u ** 3 * p0[k] + 3 * u * u * t * p1[k] + 3 * u * t * t * p2[k] + t ** 3 * p3[k]))
                    for k in (0, 1)])
    return out


def capsule_ring(a, b, w: int, max_err: int) -> list[list[int]]:
    """Outline of the stroke of segment a-b with width w and round caps (inscribed, chord error <=
    max_err). O(1/sqrt(max_err))."""
    r = max(w / 2.0, 1.0)
    dx, dy = b[0] - a[0], b[1] - a[1]
    base = math.atan2(dy, dx) if (dx or dy) else 0.0
    step = 2 * math.acos(max(-1.0, min(1.0, 1 - max(max_err, 1) / r))) if r > max_err else math.pi / 2
    n = max(2, int(math.ceil(math.pi / max(step, 1e-3))))
    pts = []
    for i in range(n + 1):  # cap at b, from -90 deg to +90 deg of the direction
        t = base - math.pi / 2 + math.pi * i / n
        pts.append([int(round(b[0] + r * math.cos(t))), int(round(b[1] + r * math.sin(t)))])
    for i in range(n + 1):  # cap at a
        t = base + math.pi / 2 + math.pi * i / n
        pts.append([int(round(a[0] + r * math.cos(t))), int(round(a[1] + r * math.sin(t)))])
    return pts


# ---------------------------------------------------------------- containment
def point_in_ring(p, ring) -> bool:
    """Even-odd ray cast; boundary points count as inside only by accident. O(len(ring))."""
    x, y = p
    inside = False
    n = len(ring)
    for i in range(n):
        x1, y1 = ring[i]
        x2, y2 = ring[(i + 1) % n]
        if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
            inside = not inside
    return inside


def point_in_rings(p, rings) -> bool:
    """Even-odd over all rings (outer rings and holes alike, as KiCad's fractured fills require)."""
    return sum(point_in_ring(p, r) for r in rings) % 2 == 1


# ---------------------------------------------------------------- dangling ends
def track_item(uuid, net, layer, width, s, e, m=None) -> dict:
    """Track / arc record for :func:`find_dangling`. `m` (arc midpoint) makes it an arc."""
    it = {"uuid": uuid, "net": int(net), "layer": int(layer), "w": int(width), "s": list(s), "e": list(e),
          "m": None, "arc": None}
    if m is not None:
        it["m"] = list(m)
        c = arc_center(it["s"], it["m"], it["e"])
        if c is not None:
            it["arc"] = (c, math.hypot(it["s"][0] - c[0], it["s"][1] - c[1]))
    return it


def find_dangling(items: list[dict], vias_by_net: dict, in_pad) -> list[dict]:
    """Track/arc endpoints touching no same-net pad, via or other same-net track/arc.

    `items`: :func:`track_item` records. `vias_by_net`: net -> [(center, radius, {layer ids})].
    `in_pad(net, layer, point) -> bool`. Same-net items are bucketed per (net, layer), so cost is linear in
    the item count for boards whose nets are small (the normal case)."""
    groups: dict[tuple, list] = {}
    for it in items:
        groups.setdefault((it["net"], it["layer"]), []).append(it)

    out = []
    for it in items:
        net, layer = it["net"], it["layer"]
        same = groups[(net, layer)]
        for end_is_e in (False, True):
            p = it["e"] if end_is_e else it["s"]
            if net and in_pad(net, layer, p):
                continue
            if any(layer in ls and math.hypot(p[0] - c[0], p[1] - c[1]) <= r
                   for c, r, ls in vias_by_net.get(net, ())):
                continue
            if any(o is not it and (p in (o["s"], o["e"]) or dist_pt_item(p, o) <= o["w"] / 2.0)
                   for o in same):
                continue
            out.append({"item": it["uuid"], "net": net, "layer": layer, "width": it["w"],
                        "end": list(p), "direction": direction(it, end_is_e)})
    return out
