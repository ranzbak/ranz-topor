"""Detour statistics (routed length / ghost length), ruling M3-R11. Pure; no pcbnew.

detour_stats is O(n log n) in connections (sorting ratios)."""
from __future__ import annotations

MM = 1_000_000  # nm per mm


SHORT_REF_NM = 2 * MM  # references shorter than this inflate the ratio (caption flag)


def _mst_nm(pts: list[tuple[float, float]]) -> float:
    """Euclidean minimum spanning tree length of `pts` (Prim, O(n^2))."""
    if len(pts) < 2:
        return 0.0
    best = [float("inf")] * len(pts)
    used = [False] * len(pts)
    best[0] = 0.0
    total = 0.0
    for _ in range(len(pts)):
        i = min((k for k in range(len(pts)) if not used[k]), key=lambda k: best[k])
        used[i] = True
        total += best[i]
        for k in range(len(pts)):
            if not used[k]:
                d = ((pts[i][0] - pts[k][0]) ** 2 + (pts[i][1] - pts[k][1]) ** 2) ** 0.5
                best[k] = min(best[k], d)
    return total


def _tree_rows(conns: list[dict]) -> list[dict]:
    """Tree nets (two or more pad-to-pad connections of one net): one row per net, drawn tree
    length / Euclidean MST of the net's pads (a Steiner tree is at least sqrt(3)/2 of it, Du &
    Hwang 1990). Shown NEXT to the per-connection rows, which stay as they are: a connection may
    end at another pad or branch of its tree than the one it is labelled with (wiccon GPIO0 U1.18
    -> FLASH1.1: 39.6 mm against an 8.7 mm ghost while the net's tree is 1.68 x its MST), but a
    branch that really wanders shows only per connection (qmtech JS2_5: 5.69 per connection, its
    tree 2.79). A net with an unrouted connection gets no tree row. O(nets x pads^2)."""
    by: dict = {}
    for c in conns:
        f, t = c.get("from") or {}, c.get("to") or {}
        if c.get("net") is None or f.get("kind") != "pad" or t.get("kind") != "pad":
            continue
        by.setdefault(c["net"], []).append(c)
    rows: list[dict] = []
    for net, cs in sorted(by.items(), key=lambda x: x[0]):
        if len(cs) < 2 or any(c.get("status") != "routed" or c.get("length_nm") is None for c in cs):
            continue
        pads = sorted({tuple(e["at"]) for c in cs for e in (c["from"], c["to"]) if e.get("at")})
        ref = _mst_nm(pads)
        if ref <= 0:
            continue
        length = sum(c["length_nm"] for c in cs)
        first = min(cs, key=lambda c: c.get("id") if c.get("id") is not None else -1)
        rows.append({
            "net": net, "name": first.get("name"), "pads": len(pads), "connections": len(cs),
            "length_mm": round(length / MM, 3), "mst_mm": round(ref / MM, 3),
            "ratio": round(length / ref, 3), "ids": sorted(c.get("id") for c in cs if c.get("id") is not None),
        })
    return rows


def detour_stats(connections, field: str = "ghost_len_nm") -> dict | None:
    """Detour ratio (routed length / reference length) over routed connections (M3-R11, M3-R15).

    `field` names the reference: `ghost_len_nm` (topological ghost, default) or `ref_len_nm`
    (DRC-feasible reference; `ref_kind == "ghost"` marks a per-connection ghost fallback, counted
    in `ghost_fallbacks`). Returns None when no connection carries the field (old engine / option
    off). Connections that are not routed, or have a null / non-positive reference, count as
    `no_reference`. `short_ref` counts rows whose reference is < 2 mm (their ratio is noisy).
    `trees` is a second metric beside the per-connection one: per tree net (two or more pad-to-pad
    connections of one net) its drawn tree length over the Euclidean MST of its pads
    ([`_tree_rows`]); the per-connection rows stay, so the worst branch still shows.
    """
    conns = list(connections or [])
    if not any(field in c for c in conns):
        return None
    rows: list[dict] = []
    no_ref = 0
    for c in conns:
        ghost = c.get(field)
        length = c.get("length_nm")
        if c.get("status") != "routed" or not ghost or ghost <= 0 or length is None:
            no_ref += 1
            continue
        rows.append({
            "id": c.get("id"), "name": c.get("name"),
            "from": c.get("from"), "to": c.get("to"),
            "length_mm": round(length / MM, 3), "ghost_mm": round(ghost / MM, 3),
            "ratio": round(length / ghost, 3),
            "ref_kind": c.get("ref_kind") if field == "ref_len_nm" else None,
            "short_ref": ghost < SHORT_REF_NM,
        })
    ratios = sorted(r["ratio"] for r in rows)
    n = len(ratios)

    def pct(p: float) -> float | None:
        if not n:
            return None
        k = (n - 1) * p
        lo = int(k)
        hi = min(lo + 1, n - 1)
        return round(ratios[lo] + (ratios[hi] - ratios[lo]) * (k - lo), 3)

    top = sorted(rows, key=lambda r: (-r["ratio"], r["id"] if r["id"] is not None else -1))
    return {
        "median": pct(0.5), "p90": pct(0.9), "max": ratios[-1] if n else None,
        "count_gt_1_5": sum(1 for x in ratios if x > 1.5),
        "n": n, "no_reference": no_ref, "top10": top[:10],
        "field": field,
        "ghost_fallbacks": sum(1 for r in rows if r["ref_kind"] == "ghost"),
        "short_ref": sum(1 for r in rows if r["short_ref"]),
        "trees": _tree_summary(_tree_rows(conns)),
    }


def _tree_summary(trows: list[dict]) -> dict:
    """The tree-net rows ([`_tree_rows`]) as a second metric beside the per-connection one:
    `n`, `max`, `count_gt_1_5`, `top10` (worst first)."""
    top = sorted(trows, key=lambda r: (-r["ratio"], r["net"]))
    return {"n": len(trows), "max": top[0]["ratio"] if top else None,
            "count_gt_1_5": sum(1 for r in trows if r["ratio"] > 1.5), "top10": top[:10]}


def detour_line(stats: dict | None) -> str | None:
    if not stats or not stats.get("n"):
        return None
    line = (f"detour median {stats['median']:.3f} p90 {stats['p90']:.3f} "
            f"max {stats['max']:.3f} >1.5: {stats['count_gt_1_5']}")
    if stats.get("field") == "ref_len_nm":
        line += f" (ghost fallbacks {stats.get('ghost_fallbacks', 0)})"
    tr = stats.get("trees") or {}
    if tr.get("n"):
        line += f"; tree nets (tree / MST of pads) max {tr['max']:.3f} >1.5: {tr['count_gt_1_5']} of {tr['n']}"
    return line
