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
