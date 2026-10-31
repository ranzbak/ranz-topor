"""Detour statistics (routed length / ghost length), ruling M3-R11. Pure; no pcbnew.

detour_stats is O(n log n) in connections (sorting ratios)."""
from __future__ import annotations

MM = 1_000_000  # nm per mm


def detour_stats(connections, ref_key: str = "ghost_len_nm") -> dict | None:
    """Detour ratio (routed length / reference length) over routed connections (ruling M3-R11).

    The reference is the connection field `ref_key`: `ghost_len_nm` (the topological ghost, the
    default) or the DRC-feasible shortest path of M3-R15 (`detour_ref_len_nm`, A7).
    Returns None when no connection carries the field (old engine). Connections
    that are not routed, or have a null / non-positive reference length, count as `no_reference`.
    """
    conns = list(connections or [])
    if not any(ref_key in c for c in conns):
        return None
    rows: list[dict] = []
    no_ref = 0
    for c in conns:
        ghost = c.get(ref_key)
        length = c.get("length_nm")
        if c.get("status") != "routed" or not ghost or ghost <= 0 or length is None:
            no_ref += 1
            continue
        rows.append({
            "id": c.get("id"), "name": c.get("name"),
            "from": c.get("from"), "to": c.get("to"),
            "length_mm": round(length / MM, 3), "ghost_mm": round(ghost / MM, 3),
            "ratio": round(length / ghost, 3),
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
    }


def detour_line(stats: dict | None) -> str | None:
    if not stats or not stats.get("n"):
        return None
    return (f"detour median {stats['median']:.3f} p90 {stats['p90']:.3f} "
            f"max {stats['max']:.3f} >1.5: {stats['count_gt_1_5']}")
