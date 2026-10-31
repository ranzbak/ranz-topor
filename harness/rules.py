"""Rule resolution: board floors, netclasses, custom-rule (.kicad_dru) subset, widths, JLCPCB profile."""
from __future__ import annotations

import re
from dataclasses import dataclass, field

from harness.rip import NetWidths

JLCPCB_1OZ: dict = {
    "width": 200_000,
    "clearance": 150_000,
    "via_d": 600_000,
    "via_drill": 300_000,
    "via_d_min_auto": 450_000,
    "via_drill_min_auto": 250_000,
    "drill_list": [200_000, 250_000, 300_000, 400_000, 500_000, 800_000, 1_000_000],
    "routing_vias": [250_000, 300_000, 400_000, 500_000],
    "via_hole_to_track": 200_000,
    "pth_hole_to_track": 350_000,
    "npth_to_track": 250_000,
    "hole_to_hole_via": 250_000,
    "hole_to_hole_pad": 500_000,
    # copper-to-board-edge floor: 0.2 mm (user statement 2026-10-30, JLCPCB copper-to-edge)
    "edge": 200_000,
    "vcut": 500_000,
    # (0.45 - 0.25) / 2 for the smallest automatic via; 4-layer boards allow a thinner ring
    "annular_2l": 100_000,
    "annular_4l": 75_000,
}

from toporlib.dru import (  # noqa: E402,F401  (re-export)
    _UNITS, DruRule, _match_term, _parse_sexp, _tokenize, _value_nm, match_condition, parse_dru,
)

_POWER_RE = re.compile(r"^(\+|-)?\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)")
_REFUSE = ("track_angle", "track_segment_length")


def _nz(v, fallback: int) -> int:
    v = int(v)
    return v if v > 0 else fallback


def resolve_rules(
    board,
    *,
    dru_text: str | None,
    widths_before: dict[str, NetWidths],
    settings_table: dict[str, int] | None = None,
    profile: dict = JLCPCB_1OZ,
    apply_fab_floors: bool = True,
) -> dict:
    """Resolve design rules; manufacturer minimums (the profile) are FLOORS.

    With `apply_fab_floors` (default) every design value below the profile minimum is raised to it
    and reported in `floors_applied` (`[{"name", "design", "floor"}]`, nm) and in `warnings`
    (`floor: <name> <design> -> <floor> mm`); values above the floor are used as-is. A design
    value of 0 means "unset" and is silently filled from the profile. `apply_fab_floors=False`
    uses the board's own values (the profile only fills unset zeros).
    """
    settings_table = settings_table or {}
    ds = board.GetDesignSettings()
    ns = ds.m_NetSettings
    warnings: list[str] = []
    floors_applied: list[dict] = []
    annular_floor = profile["annular_2l"] if board.GetCopperLayerCount() <= 2 else profile["annular_4l"]

    def floored(name: str, design, floor: int) -> int:
        design = int(design)
        if design <= 0:
            return floor
        if apply_fab_floors and design < floor:
            floors_applied.append({"name": name, "design": design, "floor": floor})
            warnings.append(f"floor: {name} {design / 1e6:g} -> {floor / 1e6:g} mm")
            return floor
        return design

    floors = {
        "min_clearance": floored("min_clearance", ds.m_MinClearance, profile["clearance"]),
        "min_track_width": floored("min_track_width", ds.m_TrackMinWidth, profile["width"]),
        "min_via_diameter": floored("min_via_diameter", ds.m_ViasMinSize, profile["via_d_min_auto"]),
        "min_via_drill": floored("min_via_drill", ds.m_MinThroughDrill, profile["via_drill_min_auto"]),
        "min_annular": floored("min_annular", ds.m_ViasMinAnnularWidth, annular_floor),
        "hole_clearance": floored("hole_clearance", ds.m_HoleClearance, profile["pth_hole_to_track"]),
        "hole_to_hole": floored("hole_to_hole", ds.m_HoleToHoleMin, profile["hole_to_hole_via"]),
        "edge_clearance": floored("edge_clearance", ds.m_CopperEdgeClearance, profile["edge"]),
        "max_error": _nz(ds.m_MaxError, 5_000),
    }

    rules = parse_dru(dru_text) if dru_text else []
    for r in rules:
        for c in _REFUSE:
            if c in r.constraints and r.severity != "ignore":
                warnings.append(f"REFUSE: rule {r.name} restricts {c.replace('_', ' ')}")

    unparsed: set[str] = set()

    def apply_dru(net_name: str, netclass: str, clearance: int) -> tuple[int, int | None]:
        """Return (clearance, custom width or None) after applying matching custom rules."""
        width = None
        for r in rules:
            if r.severity in ("ignore", "exclude"):
                continue
            m = match_condition(r.condition, net_name=net_name, netclass=netclass, ref=None)
            cl = r.constraints.get("clearance", {}).get("min")
            if m is None:
                if (cl is not None and cl > 0) or "track_width" in r.constraints:
                    unparsed.add(r.name)
                if cl is not None and cl > clearance:
                    clearance = cl  # strictest fallback
            elif m:
                if cl is not None and cl > clearance:
                    clearance = cl
                tw = r.constraints.get("track_width", {})
                if "opt" in tw:
                    width = tw["opt"]  # last matching rule wins
                elif "min" in tw:
                    width = max(width or 0, tw["min"])
        return clearance, width

    def net_rule(net_name: str) -> dict:
        nc = ns.GetEffectiveNetClass(net_name)
        ncname = str(nc.GetName())
        clearance = max(int(nc.GetClearance()), floors["min_clearance"])
        clearance, custom_w = apply_dru(net_name, ncname, clearance)
        class_w = int(nc.GetTrackWidth())
        non_default = any(p != "Default" for p in ncname.split(","))
        if custom_w is not None:
            width, src = custom_w, "custom_rule"
        elif non_default and class_w > 0:
            width, src = class_w, "netclass"
        elif net_name in widths_before:
            width, src = widths_before[net_name].median_nm, "existing_copper"
        elif net_name in settings_table:
            width, src = int(settings_table[net_name]), "settings_table"
        else:
            width, src = class_w if class_w > 0 else profile["width"], "default"
        width = max(width, floors["min_track_width"])
        if src == "default" and net_name and _POWER_RE.match(net_name):
            warnings.append(f"POWER?: {net_name} uses Default width")
        return {
            "clearance": clearance,
            "width": width,
            "via_d": max(_nz(nc.GetViaDiameter(), profile["via_d"]), floors["min_via_diameter"]),
            "via_drill": max(_nz(nc.GetViaDrill(), profile["via_drill"]), floors["min_via_drill"]),
            "netclass": ncname,
            "width_source": src,
        }

    default = net_rule("")
    default["width_source"] = "default"
    per_net: dict[str, dict] = {}
    for code, n in sorted(board.GetNetsByNetcode().items()):
        if int(code) == 0:
            continue
        per_net[str(int(code))] = net_rule(str(n.GetNetname()))

    for name in sorted(unparsed):
        warnings.append(f"unparsed condition in rule {name}: using strictest fallback")

    return {
        **floors,
        "profile": "jlcpcb_1oz",
        "drill_list": list(profile["drill_list"]),
        "routing_vias": list(profile["routing_vias"]),
        "default": default,
        "per_net": per_net,
        "floors_applied": floors_applied,
        "warnings": warnings,
    }
