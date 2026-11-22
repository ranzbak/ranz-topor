"""grid-snap: how much of a routed board's copper lies on a grid, and why the rest does not.

    python tools/gridsnap_measure.py <routed.kicad_pcb> --grid-mm 0.254 [--json out.json]

Bends = points where exactly two segments of one net meet at an angle (> 1 deg); T points, pad ends and via
centres are not bends. A bend or via is on the grid when both coordinates lie within 1 um of
origin + k x grid (origin = the board's (setup (grid_origin)), else 0, 0). Each bend off the grid gets one reason,
first match wins:
  pad_leg    one of its two segments ends at a pad centre that is off the grid (the leg through it is fixed;
             KiCad's pad snap wins over the grid)
  via_leg    one of its two segments ends at a via that is off the grid
  any_angle  one of its two segments is not at 0/45/90 degrees
  packed     one of its two segments runs parallel to copper of another net at the clearance (+ 5 um)
  other      none of these (room the plan keeps for others, a fused branch, a forced run)
Output: counts, fractions and the survivors (net, x, y, reason). O(segments^2) for the packed test (boards here are
small enough).
"""
from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path

TOL = 0.001  # mm


def _on(p, g, o) -> bool:
    return all(abs((c - oc) / g - round((c - oc) / g)) * g <= TOL for c, oc in zip(p, o))


def _octi(a, b) -> bool:
    dx, dy = abs(b[0] - a[0]), abs(b[1] - a[1])
    return dx < 1e-4 or dy < 1e-4 or abs(dx - dy) <= 1e-4 + 1e-3 * max(dx, dy)


def _seg_dist(a, b, c, d) -> float:
    def pt(p, q, r):
        e = (r[0] - q[0], r[1] - q[1])
        l2 = e[0] ** 2 + e[1] ** 2
        t = 0.0 if l2 == 0 else max(0.0, min(1.0, ((p[0] - q[0]) * e[0] + (p[1] - q[1]) * e[1]) / l2))
        return math.dist(p, (q[0] + e[0] * t, q[1] + e[1] * t))
    return min(pt(a, c, d), pt(b, c, d), pt(c, a, b), pt(d, a, b))


def measure(pcb: Path, grid: float, clearance: float | None = None) -> dict:
    import harness  # noqa: F401,PLC0415  (KiCad isolated before pcbnew)
    import pcbnew  # noqa: PLC0415

    text = pcb.read_text()
    m = re.search(r"\(grid_origin\s+(-?[\d.]+)\s+(-?[\d.]+)\s*\)", text)
    org = (float(m.group(1)), float(m.group(2))) if m else (0.0, 0.0)
    b = pcbnew.LoadBoard(str(pcb))
    if clearance is None:
        clearance = b.GetDesignSettings().m_MinClearance / 1e6
    pads = {}
    for fp in b.GetFootprints():
        for p in fp.Pads():
            c = p.GetPosition()
            pads[(round(c.x / 1e6, 4), round(c.y / 1e6, 4))] = p.GetNetname()
    segs, vias = [], []
    for t in b.GetTracks():
        if t.GetClass() == "PCB_VIA":
            c = t.GetPosition()
            vias.append(((c.x / 1e6, c.y / 1e6), t.GetNetname()))
        elif t.GetClass() == "PCB_TRACK":
            s, e = t.GetStart(), t.GetEnd()
            segs.append(((s.x / 1e6, s.y / 1e6), (e.x / 1e6, e.y / 1e6), t.GetNetname(), t.GetWidth() / 1e6, t.GetLayerName()))
    via_at = {(round(p[0], 4), round(p[1], 4)): p for p, _n in vias}
    ends: dict = {}
    for i, (a, c, net, _w, layer) in enumerate(segs):
        if math.dist(a, c) < 1e-4:
            continue
        for p, q in ((a, c), (c, a)):
            ends.setdefault((net, layer, round(p[0], 4), round(p[1], 4)), []).append((i, p, q))
    bends = []
    for (net, layer, _x, _y), v in ends.items():
        if len(v) != 2:
            continue
        (i, p, q1), (j, _p, q2) = v
        a1, a2 = math.atan2(q1[1] - p[1], q1[0] - p[0]), math.atan2(q2[1] - p[1], q2[0] - p[0])
        d = abs((a1 - a2 + math.pi) % (2 * math.pi) - math.pi)
        if abs(d - math.pi) > math.radians(1):
            bends.append((net, p, (i, q1), (j, q2)))
    surv, on = [], 0
    for net, p, (i, q1), (j, q2) in bends:
        if _on(p, grid, org):
            on += 1
            continue
        reason = "other"
        far = [(round(q[0], 4), round(q[1], 4)) for q in (q1, q2)]
        if any(f in pads and not _on(f, grid, org) for f in far):
            reason = "pad_leg"
        elif any(f in via_at and not _on(via_at[f], grid, org) for f in far):
            reason = "via_leg"
        elif not (_octi(p, q1) and _octi(p, q2)):
            reason = "any_angle"
        else:
            for k in (i, j):
                a, c, _n, w, layer = segs[k]
                for x, y, n2, w2, l2 in segs:
                    if n2 == net or l2 != layer:
                        continue
                    if not (_octi(a, c) and _octi(x, y)):
                        continue
                    u = (c[0] - a[0], c[1] - a[1])
                    v2 = (y[0] - x[0], y[1] - x[1])
                    if abs(u[0] * v2[1] - u[1] * v2[0]) > 1e-3 * math.hypot(*u) * math.hypot(*v2):
                        continue
                    if _seg_dist(a, c, x, y) <= (w + w2) / 2 + clearance + 0.005:
                        reason = "packed"
                        break
                if reason == "packed":
                    break
        surv.append({"net": net, "x": round(p[0], 4), "y": round(p[1], 4), "reason": reason})
    # lead 2026-11-21: does the grid add bends or widen bundles? bends per net, and the packed length: the length of
    # octilinear segments that run parallel to another net's segment on their layer at the clearance (+ 5 um)
    by_net: dict = {}
    for net, *_rest in bends:
        by_net[net] = by_net.get(net, 0) + 1
    cell: dict = {}
    for k, (a, c, _n, _w, _l) in enumerate(segs):
        for cx in range(int(min(a[0], c[0])) - 1, int(max(a[0], c[0])) + 2):
            for cy in range(int(min(a[1], c[1])) - 1, int(max(a[1], c[1])) + 2):
                cell.setdefault((cx, cy), set()).add(k)
    packed = 0.0
    for k, (a, c, net, w, layer) in enumerate(segs):
        if not _octi(a, c) or math.dist(a, c) < 1e-4:
            continue
        near = set()
        for cx in range(int(min(a[0], c[0])), int(max(a[0], c[0])) + 1):
            for cy in range(int(min(a[1], c[1])), int(max(a[1], c[1])) + 1):
                near |= cell.get((cx, cy), set())
        u = (c[0] - a[0], c[1] - a[1])
        for q in near:
            x, y, n2, w2, l2 = segs[q]
            if n2 == net or l2 != layer or not _octi(x, y) or math.dist(x, y) < 1e-4:
                continue
            v2 = (y[0] - x[0], y[1] - x[1])
            if abs(u[0] * v2[1] - u[1] * v2[0]) > 1e-3 * math.hypot(*u) * math.hypot(*v2):
                continue
            if _seg_dist(a, c, x, y) <= (w + w2) / 2 + clearance + 0.005:
                packed += math.dist(a, c)
                break
    von = sum(1 for p, _n in vias if _on(p, grid, org))
    by = {}
    for s in surv:
        by[s["reason"]] = by.get(s["reason"], 0) + 1
    return {"pcb": str(pcb), "grid_mm": grid, "origin": org, "bends": len(bends), "bends_on": on,
            "bends_on_frac": round(on / len(bends), 4) if bends else None, "vias": len(vias), "vias_on": von,
            "vias_on_frac": round(von / len(vias), 4) if vias else None, "off_by_reason": dict(sorted(by.items())),
            "packed_mm": round(packed, 3), "bends_by_net": dict(sorted(by_net.items())),
            "survivors": surv}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pcb", type=Path)
    ap.add_argument("--grid-mm", type=float, required=True)
    ap.add_argument("--json", type=Path)
    a = ap.parse_args(argv)
    r = measure(a.pcb, a.grid_mm)
    if a.json:
        a.json.write_text(json.dumps(r, indent=1) + "\n")
    print(json.dumps({k: v for k, v in r.items() if k not in ("survivors", "bends_by_net")}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
