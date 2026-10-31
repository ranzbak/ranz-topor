"""Rule resolution without pcbnew (M4-R1): board floors, netclasses, custom-rule (.kicad_dru) subset, widths,
JLCPCB profile. ``resolve_rules_from_project`` is the pure core; ``harness.rules.resolve_rules`` (SWIG) and the
plugin (IPC) both feed it.

Inputs come from stable sources only: the ``.kicad_pro`` JSON (``board.design_settings.rules.*``,
``net_settings``), the ``.kicad_dru`` text, and the effective net class per net (IPC
``get_netclass_for_nets``; tests/FakeBoard use :func:`netclass_of_from_pro`).

Complexity: O((N + K) * R) for N nets, K rules, R DRU constraints per net (each net scans every rule once);
``netclass_of_from_pro`` is O(N * P) for P net-class patterns.
"""

from __future__ import annotations

import fnmatch
import re
from dataclasses import dataclass

from .dru import DruRule, _disallow_areas, match_condition, parse_dru

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

_POWER_RE = re.compile(r"^(\+|-)?\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)")
_REFUSE = ("track_angle", "track_segment_length")

#: KiCad's own defaults for a project file that does not set a design rule (mm in the JSON, nm here).
_RULE_DEFAULTS_NM = {
    "min_clearance": 0,
    "min_track_width": 200_000,
    "min_via_diameter": 500_000,
    "min_through_hole_diameter": 300_000,
    "min_via_annular_width": 100_000,
    "min_hole_clearance": 250_000,
    "min_hole_to_hole": 250_000,
    "min_copper_edge_clearance": 500_000,
    "max_error": 5_000,
}
#: KiCad's built-in values of a net class that does not set them (the "Default" class of an empty project).
_CLASS_DEFAULTS_NM = {"clearance": 200_000, "track_width": 200_000, "via_diameter": 600_000, "via_drill": 300_000}
_CLASS_FIELDS = tuple(_CLASS_DEFAULTS_NM)


@dataclass(frozen=True)
class NetClassVals:
    """Net-class values in nm (``None`` = not set by the class). Duck-compatible with
    ``topor.ports.NetClassValues``: any object with these attributes works."""
    name: str
    clearance: int | None = None
    track_width: int | None = None
    via_diameter: int | None = None
    via_drill: int | None = None


def _nz(v, fallback: int) -> int:
    v = int(v)
    return v if v > 0 else fallback


def _nm(mm) -> int:
    return int(round(float(mm) * 1e6))


def net_codes_by_name(names) -> dict[str, int]:
    """Deterministic net codes for a board that has none: sorted unique non-empty names -> 1..N. The IPC
    model exporter and the rule resolver share this, so ``rules["per_net"]`` keys match ``model["nets"]``.
    O(N log N)."""
    return {n: i + 1 for i, n in enumerate(sorted({n for n in names if n}))}


# ------------------------------------------------------------------ project JSON
def classes_from_pro(pro: dict) -> dict[str, NetClassVals]:
    """``net_settings.classes`` of a .kicad_pro as ``{name: NetClassVals}`` (a missing key -> None). O(C)."""
    out: dict[str, NetClassVals] = {}
    for c in (pro.get("net_settings") or {}).get("classes") or []:
        vals = {k: (_nm(c[k]) if c.get(k) is not None else None) for k in _CLASS_FIELDS}
        out[str(c["name"])] = NetClassVals(str(c["name"]), **vals)
    return out


def _wildcard_or_regex(pattern: str, name: str) -> bool:
    if fnmatch.fnmatchcase(name, pattern):
        return True
    try:
        return re.fullmatch(pattern, name) is not None
    except re.error:
        return False


def netclass_of_from_pro(pro: dict, net_names) -> dict[str, str]:
    """Effective class name per net as KiCad computes it, from the project JSON alone: explicit
    ``netclass_assignments`` win, else every matching ``netclass_patterns`` entry (wildcard or regex);
    several matches (plus ``Default``) compose to ``"a,b,Default"`` ordered by class priority. Always
    contains ``""`` -> ``Default``. O(N * P)."""
    ns = pro.get("net_settings") or {}
    prio = {str(c["name"]): int(c.get("priority", 2147483647)) for c in ns.get("classes") or []}
    assign = ns.get("netclass_assignments") or {}
    patterns = [(str(p["pattern"]), str(p["netclass"])) for p in ns.get("netclass_patterns") or []]
    out = {"": "Default"}
    for n in net_names:
        if not n:
            continue
        got = assign.get(n)
        names = [got] if isinstance(got, str) else list(got or [])
        if not names:
            names = [c for pat, c in patterns if _wildcard_or_regex(pat, n)]
        names = [c for c in dict.fromkeys(names) if c in prio and c != "Default"]
        names.sort(key=lambda c: prio[c])
        out[n] = ",".join(names + ["Default"])
    return out


def rule_values_from_pro(pro: dict) -> dict[str, int]:
    """``board.design_settings.rules`` as nm, KiCad defaults for unset keys. ``max_error`` is read from
    ``rules`` and falls back to ``design_settings``. O(1)."""
    ds = (pro.get("board") or {}).get("design_settings") or {}
    rules = ds.get("rules") or {}
    out = {}
    for k, d in _RULE_DEFAULTS_NM.items():
        v = rules.get(k, ds.get(k) if k == "max_error" else None)
        out[k] = _nm(v) if v is not None else d
    return out


# ------------------------------------------------------------------ resolution
def resolve_rules_from_project(
    pro: dict,
    dru_text: str | None,
    *,
    netclass_of: dict[str, str],
    classes: dict[str, NetClassVals],
    widths_before: dict,
    settings_table: dict[str, int] | None = None,
    profile: dict = JLCPCB_1OZ,
    apply_fab_floors: bool = True,
    copper_layer_count: int = 2,
    net_codes: dict[str, int] | None = None,
) -> dict:
    """Resolve design rules; manufacturer minimums (the profile) are FLOORS.

    `pro`: parsed .kicad_pro (design-rule floors). `netclass_of`: net name -> effective class name (``""``
    optional -> Default; a composite ``"Power,Default"`` is resolved from `classes` unless it is itself a
    key of `classes`). `classes`: class name -> values (None = unset; falls back to the Default class, then
    KiCad's built-ins). `widths_before`: net name -> object with ``median_nm``. `net_codes`: net name ->
    code for the ``per_net`` keys (default: :func:`net_codes_by_name` of `netclass_of`).

    With `apply_fab_floors` (default) every design value below the profile minimum is raised to it and
    reported in `floors_applied` (`[{"name", "design", "floor"}]`, nm) and in `warnings`
    (`floor: <name> <design> -> <floor> mm`); values above the floor are used as-is. A design value of 0
    means "unset" and is silently filled from the profile. `apply_fab_floors=False` uses the board's own
    values (the profile only fills unset zeros).
    """
    settings_table = settings_table or {}
    dv = rule_values_from_pro(pro)
    warnings: list[str] = []
    floors_applied: list[dict] = []
    annular_floor = profile["annular_2l"] if copper_layer_count <= 2 else profile["annular_4l"]

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
        "min_clearance": floored("min_clearance", dv["min_clearance"], profile["clearance"]),
        "min_track_width": floored("min_track_width", dv["min_track_width"], profile["width"]),
        "min_via_diameter": floored("min_via_diameter", dv["min_via_diameter"], profile["via_d_min_auto"]),
        "min_via_drill": floored("min_via_drill", dv["min_through_hole_diameter"], profile["via_drill_min_auto"]),
        "min_annular": floored("min_annular", dv["min_via_annular_width"], annular_floor),
        "hole_clearance": floored("hole_clearance", dv["min_hole_clearance"], profile["pth_hole_to_track"]),
        "hole_to_hole": floored("hole_to_hole", dv["min_hole_to_hole"], profile["hole_to_hole_via"]),
        "edge_clearance": floored("edge_clearance", dv["min_copper_edge_clearance"], profile["edge"]),
        "max_error": _nz(dv["max_error"], 5_000),
    }

    rules: list[DruRule] = parse_dru(dru_text) if dru_text else []
    for r in rules:
        for c in _REFUSE:
            if c in r.constraints and r.severity != "ignore":
                warnings.append(f"REFUSE: rule {r.name} restricts {c.replace('_', ' ')}")

    for r in rules:
        if ({"track", "via"} & set(r.disallow) and r.severity not in ("ignore", "exclude")
                and _disallow_areas(r.condition) is None):
            warnings.append(f"REFUSE: rule {r.name} disallows {' '.join(r.disallow)} without a supported "
                            "area condition (A.insideArea('name'))")

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

    def eff(ncname: str, field: str) -> int:
        """First class of the (composite) name that sets `field`, else Default, else KiCad's built-in."""
        parts = [ncname] if ncname in classes else ncname.split(",")
        for p in parts + ["Default"]:
            c = classes.get(p)
            v = getattr(c, field, None) if c is not None else None
            if v is not None:
                return int(v)
        return _CLASS_DEFAULTS_NM[field]

    def net_rule(net_name: str) -> dict:
        ncname = netclass_of.get(net_name, "Default")
        clearance = max(eff(ncname, "clearance"), floors["min_clearance"])
        clearance, custom_w = apply_dru(net_name, ncname, clearance)
        class_w = eff(ncname, "track_width")
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
            "via_d": max(_nz(eff(ncname, "via_diameter"), profile["via_d"]), floors["min_via_diameter"]),
            "via_drill": max(_nz(eff(ncname, "via_drill"), profile["via_drill"]), floors["min_via_drill"]),
            "netclass": ncname,
            "width_source": src,
        }

    default = net_rule("")
    default["width_source"] = "default"
    codes = net_codes if net_codes is not None else net_codes_by_name(netclass_of)
    per_net: dict[str, dict] = {}
    for name, code in sorted(codes.items(), key=lambda kv: kv[1]):
        if int(code) == 0:
            continue
        per_net[str(int(code))] = net_rule(name)

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
