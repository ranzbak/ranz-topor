"""Pure pour-reach and dangling-end geometry over BoardModel dicts (no kipy, no pcbnew): shared by the IPC model
exporter (``topor.model_ipc``, which re-exports these names) and the run flow's refill of the pour fills from the
kicad-cli refilled copy (``topor.run.refill_pours``, rulings C20 / C29). Moved here from model_ipc (M4b gate review
MINOR 3) so the port-agnostic run flow needs no kipy.

Conventions: KiCad frame, integer nm; copper layer ids F.Cu 0, B.Cu 2, In<n>.Cu 2n+2; a model pad is the dict of
``model_ipc.pad_from_kipy`` / ``harness.model.export_model`` (``center``, ``rotation_deg``, ``shapes`` per layer).

Complexity: ``pad_contains`` O(1) (O(n) for a hull), ``pad_sample_points`` O(1) (O(n) for a hull),
``pour_reach`` O(F * (pads + vias)) in fill points F, ``dangling_ends`` O(items) for small nets
(``toporlib.geom.find_dangling``).
"""

from __future__ import annotations

import math

from toporlib import geom


def _top_down(i: int) -> tuple:
    return (0, 0) if i == 0 else (2, 0) if i == 2 else (1, i)


def pad_contains(pad: dict, layer: int, p) -> bool:
    """True when point `p` lies in the copper of model pad `pad` on `layer` (rotation and shape aware). O(1)
    (O(n) for a ``poly`` hull)."""
    sh = pad["shapes"].get(str(layer))
    if sh is None:
        return False
    if sh["kind"] == "poly":
        return geom.point_in_ring(p, sh["pts"])
    th = math.radians(pad["rotation_deg"])
    dx, dy = p[0] - pad["center"][0], p[1] - pad["center"][1]
    lx = dx * math.cos(th) - dy * math.sin(th)
    ly = dx * math.sin(th) + dy * math.cos(th)
    if sh["kind"] == "circle":
        return math.hypot(lx, ly) <= sh["d"] / 2.0
    w, h = sh["w"] / 2.0, sh["h"] / 2.0
    if sh["kind"] == "rect":
        return abs(lx) <= w and abs(ly) <= h
    if sh["kind"] == "oval":
        r = min(w, h)
        qx, qy = max(abs(lx) - (w - r), 0.0), max(abs(ly) - (h - r), 0.0)
        return math.hypot(qx, qy) <= r
    r = min(sh["rc"], w, h)  # roundrect
    qx, qy = max(abs(lx) - (w - r), 0.0), max(abs(ly) - (h - r), 0.0)
    return math.hypot(qx, qy) <= r


def pad_sample_points(pad: dict, layer: int) -> list[list[float]]:
    """Probe points (absolute nm) of the pour-reach test, every one INSIDE the model pad's copper on `layer`
    (M4-R8: reach errs toward "not reached"): the centre plus, in the 8 directions of the pad's box (corners and
    edge midpoints), the point at 98 % of the way to the shape's boundary (a circle / oval / roundrect corner
    probe is pulled in onto the rounded edge, so it is never outside the pad). ``poly`` pads: the centre and the
    hull vertices / edge midpoints pulled 2 % toward the centre, kept only when inside the hull (a concave custom
    pad's hull may still exceed its copper: listed in the module limitations). O(1) (O(n) for a hull)."""
    sh = pad["shapes"].get(str(layer))
    cx, cy = pad["center"]
    if sh is None:
        return []
    if sh["kind"] == "poly":
        pts = sh["pts"]
        mids = [[(pts[i][0] + pts[(i + 1) % len(pts)][0]) / 2, (pts[i][1] + pts[(i + 1) % len(pts)][1]) / 2]
                for i in range(len(pts))]
        cand = [[cx + 0.98 * (q[0] - cx), cy + 0.98 * (q[1] - cy)] for q in [*pts, *mids]]
        return [[cx, cy]] + [q for q in cand if pad_contains(pad, layer, q)]
    w, h = (sh["d"], sh["d"]) if sh["kind"] == "circle" else (sh["w"], sh["h"])
    th = math.radians(pad["rotation_deg"])
    c, s = math.cos(th), math.sin(th)

    def place(lx, ly):
        return [cx + lx * c + ly * s, cy - lx * s + ly * c]

    out = [[float(cx), float(cy)]]
    for sx in (-1, 0, 1):
        for sy in (-1, 0, 1):
            if (sx, sy) == (0, 0):
                continue
            ux, uy = sx * w / 2.0, sy * h / 2.0
            lo, hi = 0.0, 1.0  # largest k in [0, 1] with k * (ux, uy) inside the shape (bisection, 20 steps)
            if pad_contains(pad, layer, place(ux, uy)):
                lo = 1.0
            else:
                for _ in range(20):
                    mid = (lo + hi) / 2
                    lo, hi = (mid, hi) if pad_contains(pad, layer, place(mid * ux, mid * uy)) else (lo, mid)
            k = 0.98 * lo
            if k > 0:
                out.append(place(k * ux, k * uy))
    return out


def _in_span(span: list, layer: int) -> bool:
    lo, hi = sorted(span, key=_top_down)
    return _top_down(lo) <= _top_down(layer) <= _top_down(hi)


def pour_reach(code: int, layers: list, fills: list, pads: list, vias: list) -> tuple:
    """(reached pad refs "REF.NUM", reached via uuids) of a pour of net `code` on `layers` with `fills`
    (dicts layer/outer/holes). A same-net pad is reached on a zone layer when one of its probe points lies in
    that layer's fill (even-odd over all rings); a same-net via when its centre does (over its span). Shared by
    the IPC exporter and the run flow's post-rip refill. O(F * (pads + vias))."""
    rings: dict[int, list] = {}
    for f in fills:
        rings.setdefault(f["layer"], []).extend([f["outer"], *f.get("holes", [])])
    reach_pads = sorted(f"{p['ref']}.{p['number']}" for p in pads if p["net"] == code and any(
        l in rings and any(geom.point_in_rings(pt, rings[l]) for pt in pad_sample_points(p, l))
        for l in p["layers"] if l in layers))
    reach_vias = sorted(v["uuid"] for v in vias if v["net"] == code and any(
        l in rings and geom.point_in_rings(v["at"], rings[l]) for l in layers if _in_span(v["layers"], l)))
    return reach_pads, reach_vias


def dangling_ends(kept: list, vias: list, pads: list, cu_order: list) -> list:
    """Dangling track ends of the `kept` entries (same rule as the exporter / harness). O(items)."""
    pad_by_net: dict[int, list] = {}
    for p in pads:
        pad_by_net.setdefault(p["net"], []).append(p)
    items = [geom.track_item(k["uuid"], k["net"], k["layer"], k["width"],
                             k["prim"]["a"] if k["prim"]["kind"] == "seg" else k["prim"]["start"],
                             k["prim"]["b"] if k["prim"]["kind"] == "seg" else k["prim"]["end"],
                             k["prim"].get("mid")) for k in kept]
    vias_by_net: dict[int, list] = {}
    for v in vias:
        vias_by_net.setdefault(v["net"], []).append(
            (v["at"], v["d"] / 2.0, {l for l in cu_order if _in_span(v["layers"], l)}))

    def in_pad(net, layer, pt) -> bool:
        return any(pad_contains(p, layer, pt) for p in pad_by_net.get(net, ()))

    return geom.find_dangling(items, vias_by_net, in_pad)


__all__ = ["pad_contains", "pad_sample_points", "pour_reach", "dangling_ends"]
