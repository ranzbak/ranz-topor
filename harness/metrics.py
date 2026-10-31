"""Copper metrics for a loaded board (segments, arcs, vias and lengths), nm in, mm out."""
from __future__ import annotations

from harness.paths import MM


def _is_arc(t) -> bool:
    return t.GetClass() == "PCB_ARC"


def _is_via(t) -> bool:
    return t.GetClass() == "PCB_VIA"


def copper_metrics(board, nets: set[str] | None = None) -> dict:
    """Counts and total lengths of tracks/arcs/vias; `nets` restricts to those net names."""
    segments = arcs = vias = 0
    track_nm = 0.0
    arc_nm = 0.0
    for t in board.GetTracks():
        if nets is not None and t.GetNetname() not in nets:
            continue
        if _is_via(t):
            vias += 1
        elif _is_arc(t):
            arcs += 1
            arc_nm += float(t.GetRadius()) * abs(t.GetAngle().AsRadians())
        else:
            segments += 1
            track_nm += float(t.GetLength())
    return {
        "segments": segments,
        "arcs": arcs,
        "vias": vias,
        "track_mm": round(track_nm / MM, 3),
        "arc_mm": round(arc_nm / MM, 3),
    }


def detour_stats(connections) -> dict | None:
    """Detour ratio (routed length / ghost length) over routed connections (ruling M3-R11).

    Returns None when no connection carries the `ghost_len_nm` field (old engine). Connections
    that are not routed, or have a null / non-positive ghost length, count as `no_reference`.
    """
    conns = list(connections or [])
    if not any("ghost_len_nm" in c for c in conns):
        return None
    rows: list[dict] = []
    no_ref = 0
    for c in conns:
        ghost = c.get("ghost_len_nm")
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
