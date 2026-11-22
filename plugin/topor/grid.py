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
import sys
from collections.abc import Mapping
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


_VERSION_DIR = re.compile(r"^(\d+)\.(\d+)$")


def kicad_settings_dirs(version: tuple[int, int] | None, env: Mapping[str, str], platform: str | None = None,
                        home: Path | None = None) -> list[Path]:
    """The folders KiCad keeps its user settings (pcbnew.json) in, best first, by KiCad's own rule
    (common/paths.cpp ``PATHS::CalculateUserSettingsPath``): the base is ``$KICAD_CONFIG_HOME`` when set and
    not empty, else the platform's user config folder (libs/kiplatform ``KIPLATFORM::ENV::GetUserConfigPath``)
    plus ``kicad``; then the settings version ``<major>.<minor>`` (``SETTINGS_MANAGER::GetSettingsVersion``:
    "9.0", "10.0", a nightly "9.99"). The user config folder: Linux ``g_get_user_config_dir()`` =
    ``$XDG_CONFIG_HOME`` when set and not empty, else ``$HOME/.config``; macOS ``~/Library/Preferences``;
    Windows ``%APPDATA%`` (wx ``GetUserConfigDir``). `version` None (KiCad did not say): every
    ``<major>.<minor>`` folder under the base, newest first. Pure: the file system is not read for a known
    version; for an unknown one only the base folder is listed."""
    platform = platform or sys.platform
    home = home or Path(env.get("HOME") or Path.home())
    if env.get("KICAD_CONFIG_HOME"):
        base = Path(env["KICAD_CONFIG_HOME"])
    elif platform == "darwin":
        base = home / "Library" / "Preferences" / "kicad"
    elif platform.startswith("win"):
        base = Path(env.get("APPDATA") or home / "AppData" / "Roaming") / "kicad"
    else:
        base = Path(env.get("XDG_CONFIG_HOME") or home / ".config") / "kicad"
    if version is not None:
        return [base / f"{version[0]}.{version[1]}"]
    try:
        found = [(tuple(map(int, m.groups())), d) for d in base.iterdir() if (m := _VERSION_DIR.match(d.name))]
    except OSError:
        return []
    return [d for _, d in sorted(found, reverse=True)]


def pick_settings_dir(dirs: list[Path]) -> Path | None:
    """The first of `dirs` that holds a pcbnew.json, else the first (so the warning names where KiCad keeps it),
    None for none."""
    return next((d for d in dirs if (d / "pcbnew.json").is_file()), dirs[0] if dirs else None)


def _find(grid_mm, settings_dir: Path | None) -> tuple:
    """(track grid, via grid, source text) in mm, first hit wins (see the module doc); (None, None, "") for none."""
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
    return track, via, src


def describe(grid_mm, pcb_text: str, settings_dir: Path | None) -> str:
    """One line for the pre-flight dialog: the grid ``resolve`` would use (the same lookup, ``_find``): size in mm,
    where it comes from, and the origin (from the board's grid origin). O(file size)."""
    track, via, src = _find(grid_mm, settings_dir)
    if track is None:
        return "no grid found (neither [drawing] grid_mm nor KiCad's grid settings)"
    via = via or track
    ox, oy = (v / 1e6 for v in grid_origin_nm(pcb_text))
    what = f"{track[0]:g} mm" if track[0] == track[1] else f"{track[0]:g} x {track[1]:g} mm"
    if via != track:
        what += f" (vias {via[0]:g} x {via[1]:g} mm)"
    where = "topor.toml" if src.startswith("topor.toml") else "KiCad settings"
    return f"{what} from {where}; origin ({ox:g}, {oy:g}) mm from the board"


def resolve(grid_mm, pcb_text: str, settings_dir: Path | None) -> tuple[dict, list[str]]:
    """Engine options ``grid_track_nm`` / ``grid_via_nm`` / ``grid_origin_nm`` and the log lines (``warning:``
    lines for a missing or coarse grid). `grid_mm`: the ``[drawing] grid_mm`` value (a number or "auto");
    `settings_dir`: the running KiCad's settings folder (pcbnew.json), None when unknown (headless)."""
    lines: list[str] = []
    track, via, src = _find(grid_mm, settings_dir)
    origin = grid_origin_nm(pcb_text)
    if track is None:
        where = f" (looked in {Path(settings_dir) / 'pcbnew.json'})" if settings_dir is not None else ""
        lines.append("warning: grid snap: no grid found (neither [drawing] grid_mm nor KiCad's grid settings"
                     f"{where}); the copper is drawn without the grid")
        return {"grid_track_nm": [0, 0], "grid_via_nm": [0, 0], "grid_origin_nm": list(origin)}, lines
    via = via or track
    nm = lambda g: [int(round(g[0] * 1e6)), int(round(g[1] * 1e6))]  # noqa: E731
    lines.append(f"grid snap: track grid {track[0]:g} x {track[1]:g} mm, via grid {via[0]:g} x {via[1]:g} mm, "
                 f"origin ({origin[0] / 1e6:g}, {origin[1] / 1e6:g}) mm, from {src}")
    if src.startswith("KiCad "):
        # The IPC API (kipy 0.8) has the grid origin but not the grid size; KiCad writes pcbnew.json when it saves
        # its settings (on exit, or OK in Preferences), so a grid picked in the running session is not seen yet.
        lines.append("warning: grid snap: the grid is KiCad's last saved setting (pcbnew.json); a grid changed "
                     "since KiCad last saved its settings is not seen: set [drawing] grid_mm, or close and reopen "
                     "KiCad after changing the grid")
    if max(track + via) > COARSE_MM:
        lines.append(f"warning: grid snap: the grid ({max(track + via):g} mm) is coarser than {COARSE_MM:g} mm; "
                     "bends and vias prefer it where room allows, the rest stays off it")
    return {"grid_track_nm": nm(track), "grid_via_nm": nm(via), "grid_origin_nm": list(origin)}, lines
