"""Small pure geometry helpers shared by the harness exporter and the IPC exporter (integer nm points).

Complexity: ``hull`` O(n log n); ``find_dangling`` O(sum over (net, layer) buckets of items^2) (linear for
the normal case of small nets); ``flatten_arc`` / ``circle_ring`` and their circumscribed ``*_outward`` twins
O(1/sqrt(max_err)); ``point_in_rings`` O(ring points).

``flatten_arc`` / ``circle_ring`` / ``capsule_ring`` are INSCRIBED (vertices on the curve, chord error inward);
the harness and the preview use them. The ``*_outward`` functions and ``arc_stroke_rings`` are CIRCUMSCRIBED
(the polygon contains the true curve): the IPC exporter uses them for obstacles (M4-R8, ruling C25).
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


# ---------------------------------------------------------------- circumscribed (outward) variants
# M4-R8 / ruling C25: an obstacle polygon must CONTAIN KiCad's true shape. The functions below place every
# arc / cap / circle vertex so the polygon contains the true curve (tangent construction: each edge lies on a
# tangent of the circle), with at most `max_err` (+ 1 nm rounding) of outward excess. Integer rounding is
# absorbed by growing the construction radius by 1 nm. Concave arc sides (the inner side of an arc stroke)
# keep their vertices ON the circle: the chords then lie inside the circle, which is outside the shape.
# Complexity: O(sweep / sqrt(max_err / r)) points per arc, as the inscribed versions.

def _tangent_step(r: float, max_err: int) -> float:
    """Largest angular step whose tangent polygon stays within `max_err` of a circle of radius `r`."""
    e = max(int(max_err), 1)
    return 2 * math.acos(r / (r + e)) if r > 0 else math.pi / 2


def _chord_step(r: float, max_err: int) -> float:
    e = max(int(max_err), 1)
    return 2 * math.acos(max(-1.0, min(1.0, 1 - e / r))) if r > e else math.pi / 2


def tangent_arc_pts(c, r: float, a0: float, sweep: float, max_err: int) -> list[list[int]]:
    """Polyline OUTSIDE the arc of circle (`c`, `r`) from angle `a0` over `sweep` (radians, signed): starts and
    ends at the arc's end points (radius r + 1 nm), every edge on a tangent of the circle, interior vertices at
    (r + 1) / cos(step / 2) <= r + max_err + 1. Together with the centre side it contains the circular sector.
    O(|sweep| / step)."""
    rr = r + 1.0
    n = max(1, int(math.ceil(abs(sweep) / max(_tangent_step(rr, max_err), 1e-3))))
    if n == 1 and abs(sweep) > math.pi / 2:
        n = 2
    d = sweep / n
    ro = rr / math.cos(d / 2)
    out = [[c[0] + rr * math.cos(a0), c[1] + rr * math.sin(a0)]]
    for k in range(n):
        a = a0 + d * (k + 0.5)
        out.append([c[0] + ro * math.cos(a), c[1] + ro * math.sin(a)])
    out.append([c[0] + rr * math.cos(a0 + sweep), c[1] + rr * math.sin(a0 + sweep)])
    return [[int(round(x)), int(round(y))] for x, y in out]


def chord_arc_pts(c, r: float, a0: float, sweep: float, max_err: int) -> list[list[int]]:
    """Polyline with every vertex ON the arc (inscribed, chord error <= max_err), end points included. Used for
    concave sides, where chords lie outside the shape. O(|sweep| / step)."""
    n = max(1, int(math.ceil(abs(sweep) / max(_chord_step(r, max_err), 1e-3))))
    return [[int(round(c[0] + r * math.cos(a0 + sweep * k / n))), int(round(c[1] + r * math.sin(a0 + sweep * k / n)))]
            for k in range(n + 1)]


def arc_geometry(s, m, e):
    """(centre, radius, start angle, signed sweep) of the arc s->m->e; None when collinear. A closed arc
    (s == e) sweeps a full turn."""
    c = arc_center(s, m, e)
    if c is None:
        return None
    r = math.hypot(s[0] - c[0], s[1] - c[1])
    a_s, a_m, a_e = (math.atan2(q[1] - c[1], q[0] - c[0]) for q in (s, m, e))
    ccw = (a_m - a_s) % TAU <= (a_e - a_s) % TAU
    sweep = ((a_e - a_s) % TAU) if ccw else -((a_s - a_e) % TAU)
    if sweep == 0:
        sweep = TAU if ccw else -TAU
    return c, r, a_s, sweep


def circle_ring_outward(center, r: float, max_err: int) -> list[list[int]]:
    """Regular polygon CIRCUMSCRIBED about the circle (contains the disc), vertex excess <= max_err + 1 nm,
    >= 8 sides. O(n)."""
    rr = max(float(r), 1.0) + 1.0
    n = max(8, int(math.ceil(TAU / max(_tangent_step(rr, max_err), 1e-3))))
    ro = rr / math.cos(math.pi / n)
    return [[int(round(center[0] + ro * math.cos(TAU * i / n))), int(round(center[1] + ro * math.sin(TAU * i / n)))]
            for i in range(n)]


def capsule_ring_outward(a, b, w: float, max_err: int) -> list[list[int]]:
    """Outline CONTAINING the stroke of segment a-b with width `w` and round caps (tangent caps, excess <=
    max_err + 1 nm). O(1/sqrt(max_err))."""
    r = max(w / 2.0, 1.0)
    dx, dy = b[0] - a[0], b[1] - a[1]
    base = math.atan2(dy, dx) if (dx or dy) else 0.0
    return (tangent_arc_pts(b, r, base - math.pi / 2, math.pi, max_err)
            + tangent_arc_pts(a, r, base + math.pi / 2, math.pi, max_err))


def arc_stroke_rings(s, m, e, w: float, max_err: int) -> list[list[list[int]]]:
    """Rings CONTAINING the stroke of arc s->m->e with width `w` and round caps, outward excess <= max_err
    + 1 nm: one annular-sector polygon (outer side tangent, inner side on the inner circle) with tangent caps.
    A collinear arc is a capsule; an arc whose radius is not larger than half the width (no inner side) falls
    back to capsules around its polyline, each grown by the chord error. O(sweep / step)."""
    g = arc_geometry(s, m, e)
    hw = max(w / 2.0, 1.0)
    if g is None:
        return [capsule_ring_outward(s, e, w, max_err)]
    c, r, a0, sw = g
    if r - hw <= max_err or abs(sw) >= TAU - 1e-9:
        e3 = max(int(max_err) // 3, 1)
        return polyline_stroke_rings(chord_arc_pts(c, r, a0, sw, e3), w, e3, grow=e3)
    sg = 1.0 if sw > 0 else -1.0
    a1 = a0 + sw
    p_end = (c[0] + r * math.cos(a1), c[1] + r * math.sin(a1))
    p_start = (c[0] + r * math.cos(a0), c[1] + r * math.sin(a0))
    outer = tangent_arc_pts(c, r + hw, a0, sw, max_err)
    cap_e = tangent_arc_pts(p_end, hw, a1, sg * math.pi, max_err)  # outer side -> inner side around the end
    inner = chord_arc_pts(c, r - hw - 1.0, a1, -sw, max_err)
    cap_s = tangent_arc_pts(p_start, hw, a0 + sg * math.pi, sg * math.pi, max_err)
    return [outer + cap_e[1:-1] + inner + cap_s[1:-1]]


def polyline_stroke_rings(pts, w: float, max_err: int, *, grow: float = 0.0, closed: bool = False) -> list:
    """Capsules CONTAINING the stroke (width `w`, round caps) of the polyline `pts`, each capsule's half width
    grown by `grow` (the distance by which the true centre line may deviate from `pts`, e.g. a chord or Bezier
    flattening error). Outward excess <= grow + (grow + max_err + 1 nm) on the concave side. O(len(pts))."""
    n = len(pts) if closed else len(pts) - 1
    if n <= 0:
        return [capsule_ring_outward(pts[0], pts[0], w + 2 * grow, max_err)] if pts else []
    return [capsule_ring_outward(pts[i], pts[(i + 1) % len(pts)], w + 2 * grow, max_err) for i in range(n)]


def bezier_stroke_rings(p0, p1, p2, p3, w: float, max_err: int, kicad_err: int) -> list[list[list[int]]]:
    """Rings CONTAINING KiCad's stroke (width `w`) of the cubic Bezier p0..p3, where KiCad strokes its OWN
    flattened polyline (vertices on the curve, chord error <= `kicad_err`, which bows to the concave side of
    the bend). Our polyline (uniform parameter steps, error <= max_err / 3) is offset per chord by
    w/2 + max_err/3 on the convex side and w/2 + kicad_err on the concave side (rectangles), with a two-radius
    disc at every interior vertex and a plain disc at both ends (tangent construction). Outward excess <= max_err (+ 1 nm) on the convex side and
    <= kicad_err + max_err on the concave side, where KiCad's own polygon already reaches kicad_err further.
    O(n) for the n = O(sqrt(curvature / max_err)) chords."""
    e3 = max(int(max_err) // 3, 1)
    hw = max(w / 2.0, 1.0)
    P = (p0, p1, p2, p3)
    dd = max(math.hypot(p0[0] - 2 * p1[0] + p2[0], p0[1] - 2 * p1[1] + p2[1]),
             math.hypot(p1[0] - 2 * p2[0] + p3[0], p1[1] - 2 * p2[1] + p3[1]))
    n = max(2, int(math.ceil(math.sqrt(0.75 * dd / e3))))

    def at(t):
        u = 1 - t
        return (u ** 3 * P[0][0] + 3 * u * u * t * P[1][0] + 3 * u * t * t * P[2][0] + t ** 3 * P[3][0],
                u ** 3 * P[0][1] + 3 * u * u * t * P[1][1] + 3 * u * t * t * P[2][1] + t ** 3 * P[3][1])

    pts = [at(i / n) for i in range(n + 1)]
    chords = []  # (angle, left offset, right offset) per chord; left = +90 degrees of the chord direction
    for i in range(n):
        a, b = pts[i], pts[i + 1]
        dx, dy = b[0] - a[0], b[1] - a[1]
        q = at((i + 0.5) / n)
        side = dx * (q[1] - a[1]) - dy * (q[0] - a[0])  # > 0: the curve bulges to the left (convex side)
        conv, conc = hw + e3 + 1.0, hw + max(int(kicad_err), 0) + e3 + 1.0
        lo, ro = (conv, conc) if side > 0 else (conc, conv) if side < 0 else (conc, conc)
        chords.append((math.atan2(dy, dx) if (dx or dy) else 0.0, lo, ro))
    out = []
    for i, (phi, lo, ro) in enumerate(chords):
        a, b = pts[i], pts[i + 1]
        nx, ny = -math.sin(phi), math.cos(phi)
        out.append([[int(round(a[0] + lo * nx)), int(round(a[1] + lo * ny))],
                    [int(round(b[0] + lo * nx)), int(round(b[1] + lo * ny))],
                    [int(round(b[0] - ro * nx)), int(round(b[1] - ro * ny))],
                    [int(round(a[0] - ro * nx)), int(round(a[1] - ro * ny))]])
    for j, p in enumerate(pts):
        adj = [chords[k] for k in (j - 1, j) if 0 <= k < n]
        sx = sum(math.cos(c[0]) for c in adj)
        sy = sum(math.sin(c[0]) for c in adj)
        phi = math.atan2(sy, sx)
        rl, rr = max(c[1] for c in adj), max(c[2] for c in adj)
        if j in (0, n):  # end cap: KiCad's cap is a plain half disc of radius w/2 at the (exact) end point
            rl = rr = hw + e3 + 1.0
        out.append(tangent_arc_pts(p, rl - 1.0, phi, math.pi, e3)[:-1]
                   + tangent_arc_pts(p, rr - 1.0, phi + math.pi, math.pi, e3)[:-1])
    return out


def flatten_arc_outward(s, m, e, max_err: int) -> list[list[int]]:
    """Polyline from s to e lying OUTSIDE the arc s->m->e (on its convex side; tangent construction), vertex
    excess <= max_err + 1 nm; the end points are s and e exactly. A collinear arc gives [s, e]."""
    g = arc_geometry(s, m, e)
    if g is None:
        return [list(s), list(e)]
    c, r, a0, sw = g
    pts = tangent_arc_pts(c, r, a0, sw, max_err)
    return [list(map(int, s))] + pts[1:-1] + [list(map(int, e))]


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
