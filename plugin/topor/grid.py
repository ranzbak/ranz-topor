"""grid-snap: the user's KiCad grid for the drawn copper (docs/dev/design/grid-snap.md).

KiCad keeps the grid *size* in its own app settings, not in the project: ``pcbnew.json`` ``window.grid``
(``sizes``, ``last_size``; KiCad 9+ also the per-item overrides ``overrides_enabled``, ``override_wires`` /
``override_wires_idx``, ``override_vias`` / ``override_vias_idx``, which its router uses for tracks and vias:
``pcb_grid_helper.cpp`` ``GetItemGrid`` / ``GetGridSize``). The grid *origin* is in the board file
(``(setup (grid_origin x y))``, absent = 0, 0). Source order, first hit wins: ``[drawing] grid_mm`` (a number),
then (``"auto"``) KiCad's pcbnew.json of the running KiCad (read only, never written), then none: warning
"no grid", grid snap off. A grid above 0.5 mm is used and warned about (coarse grid).

Complexity: O(file size) for the board text scan, O(#grid sizes) for the settings.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

COARSE_MM = 0.5
_UNITS = {"mm": 1.0, "mil": 0.0254, "in": 25.4, "inch": 25.4, "um": 0.001, "µm": 0.001}
_ORIGIN = re.compile(r"\(grid_origin\s+(-?[\d.]+)\s+(-?[\d.]+)\s*\)")


def parse_size(text) -> float | None:
    """A KiCad grid size string (``"10 mil"``, ``"0.25 mm"``, ``"0,254 mm"``, a bare number = mm) in mm;
    None when it does not parse or is not positive."""
    if isinstance(text, (int, float)) and not isinstance(text, bool):
        v = float(text)
        return v if v > 0 else None
    if not isinstance(text, str):
        return None
    m = re.fullmatch(r"\s*(-?[\d.,]+)\s*([a-zA-Zµ]*)\s*", text)
    if not m:
        return None
    try:
        v = float(m.group(1).replace(",", "."))
    except ValueError:
        return None
    unit = m.group(2).lower() or "mm"
    if unit not in _UNITS or v <= 0:
        return None
    return v * _UNITS[unit]


def grid_origin_nm(pcb_text: str) -> tuple[int, int]:
    """The board's grid origin (nm, KiCad frame) from the .kicad_pcb text; (0, 0) when the board has none."""
    m = _ORIGIN.search(pcb_text or "")
    if not m:
        return (0, 0)
    return (int(round(float(m.group(1)) * 1e6)), int(round(float(m.group(2)) * 1e6)))


def _entry(sizes: list, idx) -> tuple[float, float] | None:
    if not isinstance(idx, int) or not 0 <= idx < len(sizes):
        return None
    e = sizes[idx]
    if isinstance(e, dict):
        x, y = parse_size(e.get("x")), parse_size(e.get("y"))
    else:  # KiCad 6/7: a plain string per size
        x = y = parse_size(e)
    return (x, y) if x and y else None


def kicad_grids(pcbnew_json: dict) -> tuple[tuple[float, float] | None, tuple[float, float] | None, str]:
    """(track grid, via grid, how) in mm from a pcbnew.json dict: the current grid, replaced per item by an
    enabled override (wires for tracks, vias for vias); a KiCad 6/7 "User Grid" entry is ``user_grid_x/y``."""
    g = (pcbnew_json.get("window") or {}).get("grid") or {}
    sizes = g.get("sizes") or []
    cur = _entry(sizes, g.get("last_size"))
    if cur is None and g.get("last_size") == len(sizes):  # KiCad 6/7: the index after the list = the user grid
        x, y = parse_size(g.get("user_grid_x")), parse_size(g.get("user_grid_y"))
        cur = (x, y) if x and y else None
    track, via, how = cur, cur, "current grid"
    if g.get("overrides_enabled"):
        if g.get("override_wires"):
            track = _entry(sizes, g.get("override_wires_idx")) or track
            how = "wire override"
        if g.get("override_vias"):
            via = _entry(sizes, g.get("override_vias_idx")) or via
            how += " / via override"
    return track, via, how


def resolve(grid_mm, pcb_text: str, settings_dir: Path | None) -> tuple[dict, list[str]]:
    """Engine options ``grid_track_nm`` / ``grid_via_nm`` / ``grid_origin_nm`` and the log lines (``warning:``
    lines for a missing or coarse grid). `grid_mm`: the ``[drawing] grid_mm`` value (a number or "auto");
    `settings_dir`: the running KiCad's settings folder (pcbnew.json), None when unknown (headless)."""
    lines: list[str] = []
    track = via = None
    src = ""
    if grid_mm != "auto" and parse_size(grid_mm):
        track = via = (float(grid_mm), float(grid_mm))
        src = "topor.toml [drawing] grid_mm"
    elif settings_dir is not None:
        p = Path(settings_dir) / "pcbnew.json"
        try:
            d = json.loads(p.read_text())
        except (OSError, ValueError):
            d = None
        if isinstance(d, dict):
            track, via, how = kicad_grids(d)
            src = f"KiCad {p} ({how})"
    origin = grid_origin_nm(pcb_text)
    if track is None:
        lines.append("warning: grid snap: no grid found (neither [drawing] grid_mm nor KiCad's grid settings); "
                     "the copper is drawn without the grid")
        return {"grid_track_nm": [0, 0], "grid_via_nm": [0, 0], "grid_origin_nm": list(origin)}, lines
    via = via or track
    nm = lambda g: [int(round(g[0] * 1e6)), int(round(g[1] * 1e6))]  # noqa: E731
    lines.append(f"grid snap: track grid {track[0]:g} x {track[1]:g} mm, via grid {via[0]:g} x {via[1]:g} mm, "
                 f"origin ({origin[0] / 1e6:g}, {origin[1] / 1e6:g}) mm, from {src}")
    if max(track + via) > COARSE_MM:
        lines.append(f"warning: grid snap: the grid ({max(track + via):g} mm) is coarser than {COARSE_MM:g} mm; "
                     "bends and vias prefer it where room allows, the rest stays off it")
    return {"grid_track_nm": nm(track), "grid_via_nm": nm(via), "grid_origin_nm": list(origin)}, lines
