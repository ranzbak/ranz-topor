"""Rule C (edge-clear, user 2026-11-17): the copper-to-edge clearance tracks and vias keep when the board's resolved
copper-to-edge clearance is 0 (Board Setup 0, no custom ``edge_clearance`` rule raising it), ``[rules]
edge_clearance_zero``:

* ``"zone_clearance"`` (default, user decision 2026-11-17): the biggest clearance of the board's copper pours, one value
  for all layers. KiCad itself does not use a zone's clearance for the edge: its filler knocks Edge.Cuts out at
  ``max(PHYSICAL_CLEARANCE, EDGE_CLEARANCE)`` (pcbnew/zone_filler.cpp, tag 10.0.8,
  ``ZONE_FILLER::buildCopperItemClearances``, lambda ``knockoutGraphicClearance``), and the DRC judges the routed copper
  with 0; the zone clearance is the board's own statement of how far copper of the pours keeps from other copper, so the
  router keeps that far from the edge too (stricter than KiCad, never looser).
* ``"pours"``: how close the board's filled pours actually come to the edge: per copper layer the least distance of a
  fill vertex to the Edge.Cuts centre line among the fills that come within :data:`POUR_WINDOW_NM` of it (a pour that
  stays far inside, e.g. a local power island, says nothing about the edge), the biggest over the layers (one value);
  0 without such fills. Measured on the fills as the board file has them (refilled by the exporters).
* ``"board"``: 0, exactly as KiCad's DRC (copper may reach the Edge.Cuts centre line).

A non-zero resolved value always wins (the option is not consulted). Every mode keeps the EDGE_CLEARANCE_ZERO warning
pointing to KiCad → File → Board Setup → Design Rules → Constraints → Copper to edge clearance.

Complexity: ``zone_clearance`` O(pours); ``pours`` O(fill vertices x outline segments within a grid cell).
"""
from __future__ import annotations

import math

MODES = ("zone_clearance", "board", "pours")
DEFAULT_MODE = "zone_clearance"
#: ``pours``: only fills that come this close to the edge (nm) count.
POUR_WINDOW_NM = 1_000_000
_CELL = 1_000_000
_SAG_NM = 1_000.0


def _outline_segments(outline: list[dict]) -> list[tuple[float, float, float, float]]:
    """Model outline prims (KiCad frame, nm) as segments: ``seg`` as is, ``arc`` as chords within :data:`_SAG_NM` of it.
    O(output)."""
    out = []
    for p in outline or []:
        if p.get("kind") == "seg":
            out.append((*p["a"], *p["b"]))
        elif p.get("kind") == "arc":
            (x0, y0), (xm, ym), (x1, y1) = p["start"], p["mid"], p["end"]
            d = 2 * (x0 * (ym - y1) + xm * (y1 - y0) + x1 * (y0 - ym))
            if abs(d) < 1e-9:
                out.append((x0, y0, x1, y1))
                continue
            s0, sm, s1 = x0 * x0 + y0 * y0, xm * xm + ym * ym, x1 * x1 + y1 * y1
            cx = (s0 * (ym - y1) + sm * (y1 - y0) + s1 * (y0 - ym)) / d
            cy = (s0 * (x1 - xm) + sm * (x0 - x1) + s1 * (xm - x0)) / d
            r = math.hypot(x0 - cx, y0 - cy)
            a0, am, a1 = (math.atan2(y - cy, x - cx) for x, y in ((x0, y0), (xm, ym), (x1, y1)))
            span = (a1 - a0) % (2 * math.pi)
            if (am - a0) % (2 * math.pi) > span:  # the arc runs the other way round
                span -= 2 * math.pi
            # chords within 1 um of the arc (sagitta), so a distance is at most that short
            n = max(1, math.ceil(abs(span) / (2 * math.acos(max(-1.0, 1 - _SAG_NM / max(r, 1.0))))))
            pts = [(cx + r * math.cos(a0 + span * k / n), cy + r * math.sin(a0 + span * k / n)) for k in range(n + 1)]
            out += [(*pts[k], *pts[k + 1]) for k in range(n)]
    return out


def _seg_dist(px, py, ax, ay, bx, by) -> float:
    dx, dy = bx - ax, by - ay
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 <= 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / l2))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def pour_setback(model: dict) -> int:
    """The ``pours`` value (nm): see the module doc."""
    segs = _outline_segments(model.get("outline") or [])
    if not segs:
        return 0
    grid: dict[tuple[int, int], list[int]] = {}
    for i, (ax, ay, bx, by) in enumerate(segs):
        for gx in range(int(min(ax, bx) // _CELL) - 1, int(max(ax, bx) // _CELL) + 2):
            for gy in range(int(min(ay, by) // _CELL) - 1, int(max(ay, by) // _CELL) + 2):
                grid.setdefault((gx, gy), []).append(i)
    per_layer: dict[int, float] = {}
    for pour in model.get("pours") or []:
        for f in pour.get("fills") or []:
            for ring in [f.get("outer") or []] + list(f.get("holes") or []):
                for x, y in ring:
                    near = grid.get((int(x // _CELL), int(y // _CELL)))
                    if not near:
                        continue
                    d = min(_seg_dist(x, y, *segs[i]) for i in near)
                    if d <= POUR_WINDOW_NM:
                        per_layer[f["layer"]] = min(per_layer.get(f["layer"], math.inf), d)
    return int(round(max(per_layer.values()))) if per_layer else 0


def zone_clearance(model: dict) -> int:
    """The ``zone_clearance`` value (nm): the biggest clearance of the model's copper pours, 0 without pours."""
    return max((int(p.get("clearance") or 0) for p in model.get("pours") or []), default=0)


def resolve(model: dict, mode: str) -> dict | None:
    """What rule C changes for `model` (its ``rules.edge_clearance`` as resolved): ``None`` when the board's value is
    not 0 (it wins) or there is no resolved value; else ``{"mode", "value_nm", "board_nm": 0}`` (``value_nm`` 0 for
    ``board``, or when the mode finds nothing). Unknown mode: ValueError. Pure."""
    if mode not in MODES:
        raise ValueError(f"unknown [rules] edge_clearance_zero {mode!r} (known: {', '.join(MODES)})")
    r = model.get("rules") or {}
    if r.get("edge_clearance") is None or int(r["edge_clearance"]) > 0:
        return None
    v = 0 if mode == "board" else zone_clearance(model) if mode == "zone_clearance" else pour_setback(model)
    return {"mode": mode, "value_nm": int(v), "board_nm": int(r["edge_clearance"])}


def apply(model: dict, mode: str) -> tuple[dict, dict | None]:
    """`model` with ``rules.edge_clearance`` replaced by rule C's value (a new dict; the input is not changed), and the
    :func:`resolve` record (``None``: unchanged)."""
    rec = resolve(model, mode)
    if rec is None or rec["value_nm"] <= 0:
        return model, rec
    return {**model, "rules": {**model["rules"], "edge_clearance": rec["value_nm"]}}, rec


def text(rec: dict | None) -> str:
    """The report / log text of a :func:`resolve` record ('' for None). Every mode and value has its sentence."""
    if rec is None:
        return ""
    v = rec["value_nm"] / 1e6
    if rec["mode"] == "board":
        return "copper-to-edge 0 used as it is ([rules] edge_clearance_zero = \"board\", as KiCad's DRC)"
    what = ("the biggest clearance of the board's copper pours" if rec["mode"] == "zone_clearance"
            else "the biggest distance the board's filled pours keep from the edge")
    if rec["value_nm"] <= 0:
        return (f"copper-to-edge 0 used as it is: [rules] edge_clearance_zero = \"{rec['mode']}\" found no value "
                f"({what}: none)")
    return (f"tracks and vias keep {v:.3f} mm from the board edge ([rules] edge_clearance_zero = \"{rec['mode']}\": "
            f"{what}); KiCad's DRC still checks 0")
