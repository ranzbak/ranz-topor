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

from .custom_rules import CustomRules, canonical_layers, canonical_name
from .dru import DruRule, _disallow_areas, parse_dru

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

#: ``[rules] fab_profile`` values: the name of a fab profile (its minimums are FLOORS over Board Setup and the custom
#: rules) or ``"board"`` (no profile: Board Setup, the net classes and the .kicad_dru are the only authority, as
#: KiCad's own DRC judges them).
FAB_PROFILES: dict = {"jlcpcb_1oz": JLCPCB_1OZ}
BOARD_AUTHORITY = "board"
DEFAULT_FAB_PROFILE = "jlcpcb_1oz"

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
    "min_connection": 0,
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
#: KiCad's factory Default net class via (diameter, drill) in mm as the ``.kicad_pro`` writes it.
FACTORY_VIA_MM = (0.8, 0.4)


def predefined_vias_from_pro(pro: dict) -> list[list[int]]:
    """KiCad's pre-defined via list: ``board.design_settings.via_dimensions`` entries with a diameter > 0 (KiCad
    always writes a ``{"diameter": 0, "drill": 0}`` placeholder first), as ``[diameter_nm, drill_nm]`` in file
    order, duplicates dropped. O(V)."""
    ds = (pro.get("board") or {}).get("design_settings") or {}
    out: list[list[int]] = []
    for v in ds.get("via_dimensions") or []:
        try:
            if not isinstance(v, dict) or float(v.get("diameter") or 0) <= 0:
                continue
            e = [_nm(v["diameter"]), _nm(v.get("drill") or 0)]
        except (TypeError, ValueError, KeyError):
            continue
        if e not in out:
            out.append(e)
    return out


def via_setup_missing_from(predefined: list, classes: dict) -> bool:
    """True when the board sets no via size up: no pre-defined via and no net class (`classes`: name ->
    :class:`NetClassVals`, nm) with a via size other than KiCad's factory 0.8 / 0.4 mm (an unset class value counts
    as factory). The engine's ``via_sizes`` applies its via fallback exactly then (ruling C27 / C33). O(C)."""
    if predefined:
        return False
    factory = tuple(int(round(v * 1e6)) for v in FACTORY_VIA_MM)
    for c in classes.values():
        d, r = c.via_diameter, c.via_drill
        if d is None and r is None:
            continue
        if (d, r) != factory and not (d in (None, 0) and r in (None, 0)):
            return False
    return True


def via_setup_missing(pro: dict | None) -> bool:
    """:func:`via_setup_missing_from` for a parsed ``.kicad_pro`` (pre-defined list + ``net_settings.classes``).
    O(V + C)."""
    pro = pro or {}
    return via_setup_missing_from(predefined_vias_from_pro(pro), classes_from_pro(pro))


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


def ignored_checks_from_pro(pro: dict) -> list[str]:
    """KiCad checks whose severity is ``ignore`` (``board.design_settings.rule_severities``), sorted. O(#rules)."""
    ds = (pro.get("board") or {}).get("design_settings") or {}
    return sorted(k for k, v in (ds.get("rule_severities") or {}).items() if v == "ignore")


def _diff_base(net: str) -> str | None:
    """Base name of a differential-pair net (``inDiffPair('...')``), None for a net without a pair suffix."""
    from .diffpair import base_name, match_dp_suffix  # noqa: PLC0415

    return base_name(net) if net and match_dp_suffix(net) is not None else None


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
    layer_names: dict[int, str] | None = None,
    fab_profile: str = DEFAULT_FAB_PROFILE,
) -> dict:
    """Resolve design rules; manufacturer minimums (the profile) are FLOORS.

    Custom rules (`dru_text`) are applied as :mod:`toporlib.custom_rules` describes (per-net values, board-wide
    edge / hole clearance, a region plan in ``custom_regions``, a report in ``custom_rules``); `layer_names`:
    copper layer id -> the board's (user) layer name, so a rule's ``(layer "<user name>")`` matches.

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

    `fab_profile` (``[rules] fab_profile``): a key of :data:`FAB_PROFILES` (default ``"jlcpcb_1oz"``: `profile`
    as given) or ``"board"``: no profile at all -- no floor, a board minimum of 0 stays 0 (KiCad: no minimum), no
    fab-profile alternative vias (``routing_vias`` / ``drill_list`` empty), and a custom rule's clearance / width
    (per net or in a courtyard / rule-area region) is used as the rule gives it, also below the Board Setup
    minimum, as KiCad's DRC does (the matching custom rule wins over the implicit board rule; KiCad 10 PCB Editor
    manual, "Custom design rules"). The result carries ``"profile"`` (the profile name or ``"board"``).
    """
    board_auth = fab_profile == BOARD_AUTHORITY
    if not board_auth:
        if fab_profile not in FAB_PROFILES:
            raise ValueError(f"unknown [rules] fab_profile {fab_profile!r} (known: "
                             f"{', '.join([*FAB_PROFILES, BOARD_AUTHORITY])})")
        if fab_profile != DEFAULT_FAB_PROFILE:
            profile = FAB_PROFILES[fab_profile]
    else:
        apply_fab_floors = False
    settings_table = settings_table or {}
    dv = rule_values_from_pro(pro)
    codes = net_codes if net_codes is not None else net_codes_by_name(netclass_of)
    cr = CustomRules(parse_dru(dru_text) if dru_text else [], canonical_layers(copper_layer_count),
                     {v: canonical_name(k) for k, v in (layer_names or {}).items()})
    cr_nets = [(n, netclass_of.get(n, "Default"), _diff_base(n)) for n in [""] + sorted(codes)] if cr else []
    if cr:
        # Board-wide custom values replace the Board Setup ones before the fab floors (KiCad: the matching
        # custom rule wins over the implicit board rule).
        dv["min_copper_edge_clearance"] = cr.board("edge_clearance", dv["min_copper_edge_clearance"], cr_nets)
        dv["min_hole_clearance"] = cr.board("hole_clearance", dv["min_hole_clearance"], cr_nets)
    warnings: list[str] = []
    floors_applied: list[dict] = []
    annular_floor = profile["annular_2l"] if copper_layer_count <= 2 else profile["annular_4l"]

    def floored(name: str, design, floor: int) -> int:
        design = int(design)
        if board_auth:  # Board Setup is the authority: its value as is (0 = no minimum, as in KiCad)
            return max(design, 0)
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
        # The drill of a via the router places keeps KiCad's hole clearance floored by the fab's VIA hole to
        # track (JLCPCB 0.2 mm), not its PTH value (0.35 mm, `hole_clearance` above, for component holes).
        "via_hole_clearance": floored("via_hole_clearance", dv["min_hole_clearance"],
                                      profile.get("via_hole_to_track", profile["pth_hole_to_track"])),
        "hole_to_hole": floored("hole_to_hole", dv["min_hole_to_hole"], profile["hole_to_hole_via"]),
        "edge_clearance": floored("edge_clearance", dv["min_copper_edge_clearance"], profile["edge"]),
        "max_error": _nz(dv["max_error"], 5_000),
        # Narrowest copper connection KiCad accepts (Board Setup > Constraints > Minimum connection width): a
        # necked pad exit may not go below it. 0 = unset (no limit).
        "min_connection": int(dv["min_connection"]),
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

    # a net class value of 0 (unset): the profile's, or KiCad's built-in Default class value without a profile
    unset_via = ((_CLASS_DEFAULTS_NM["via_diameter"], _CLASS_DEFAULTS_NM["via_drill"]) if board_auth
                 else (profile["via_d"], profile["via_drill"]))
    unset_width = _CLASS_DEFAULTS_NM["track_width"] if board_auth else profile["width"]

    def eff(ncname: str, field: str) -> int:
        """First class of the (composite) name that sets `field`, else Default, else KiCad's built-in."""
        parts = [ncname] if ncname in classes else ncname.split(",")
        for p in parts + ["Default"]:
            c = classes.get(p)
            v = getattr(c, field, None) if c is not None else None
            if v is not None:
                return int(v)
        return _CLASS_DEFAULTS_NM[field]

    def base_clearance(net_name: str) -> int:
        return max(eff(netclass_of.get(net_name, "Default"), "clearance"), floors["min_clearance"])

    def net_rule(net_name: str) -> dict:
        ncname = netclass_of.get(net_name, "Default")
        clearance = base_clearance(net_name)
        class_w = eff(ncname, "track_width")
        via_d = max(_nz(eff(ncname, "via_diameter"), unset_via[0]), floors["min_via_diameter"])
        via_drill = max(_nz(eff(ncname, "via_drill"), unset_via[1]), floors["min_via_drill"])
        cu = cr.net(net=net_name, netclass=ncname, base_clearance=clearance, base_width=class_w,
                    base_via=(via_d, via_drill), diff_base=_diff_base(net_name)) if cr else None
        if cu is not None and cu.clearance is not None and board_auth:
            clearance = cu.clearance  # KiCad: the matching custom rule wins, also below the board minimum
        elif cu is not None and cu.clearance is not None:
            if cu.clearance < floors["min_clearance"]:
                cr.notes.add(f"floor: custom clearance {cu.clearance / 1e6:g} -> {floors['min_clearance'] / 1e6:g} mm "
                             f"(fab minimum, [rules] fab_profile)", net_name)
            clearance = max(cu.clearance, floors["min_clearance"])
        custom_w = cu.width if cu is not None else None
        custom_min_w = cu.min_width if cu is not None else 0
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
            width, src = class_w if class_w > 0 else unset_width, "default"
        if cu is not None and src != "custom_rule":
            # a custom min / max still bounds a width from elsewhere
            if cu.min_width and width < cu.min_width:
                width, src = cu.min_width, "custom_rule"
            if cu.max_width is not None and width > cu.max_width:
                width, src = max(cu.max_width, cu.min_width), "custom_rule"
        if cu is not None and src == "custom_rule" and width < floors["min_track_width"] and not board_auth:
            cr.notes.add(f"floor: custom track width {width / 1e6:g} -> {floors['min_track_width'] / 1e6:g} mm "
                         f"(fab minimum, [rules] fab_profile)", net_name)
        if not (board_auth and src == "custom_rule"):  # KiCad: a custom width wins over the board minimum
            width = max(width, floors["min_track_width"])
        if src == "default" and net_name and _POWER_RE.match(net_name):
            warnings.append(f"POWER?: {net_name} uses Default width")
        if cu is not None and cu.via_d is not None:
            via_d = cu.via_d if board_auth else max(cu.via_d, floors["min_via_diameter"])
        if cu is not None and cu.via_drill is not None:
            via_drill = cu.via_drill if board_auth else max(cu.via_drill, floors["min_via_drill"])
        return {
            "clearance": clearance,
            "width": width,
            "via_d": via_d,
            "via_drill": via_drill,
            "netclass": ncname,
            "width_source": src,
            "min_width": custom_min_w,
        }

    if cr:
        # the region floor: the fab / board minimum, none with the board as the authority (KiCad: the custom rule wins)
        cr.plan_regions(cr_nets, {n: base_clearance(n) for n, _c, _d in cr_nets},
                        0 if board_auth else floors["min_clearance"])
    default = net_rule("")
    default["width_source"] = "default"
    per_net: dict[str, dict] = {}
    for name, code in sorted(codes.items(), key=lambda kv: kv[1]):
        if int(code) == 0:
            continue
        per_net[str(int(code))] = net_rule(name)

    if cr:
        custom_notes = cr.rule_warnings(skip=_REFUSE) + cr.notes.items
        warnings.extend(cr.rule_warnings(skip=_REFUSE))
        warnings.extend(cr.notes.items)

    predefined = predefined_vias_from_pro(pro)
    if via_setup_missing_from(predefined, classes):
        warnings.append("via sizes not set up: no pre-defined via and every net class via is KiCad's factory "
                        "0.8/0.4 mm (the engine's via fallback applies)")
    return {
        **floors,
        "profile": BOARD_AUTHORITY if board_auth else fab_profile,
        # the fab profile's drills and smaller alternative vias; none without a profile (KiCad's own vias only)
        "drill_list": [] if board_auth else list(profile["drill_list"]),
        "routing_vias": [] if board_auth else list(profile["routing_vias"]),
        "predefined_vias": predefined,
        "default": default,
        "per_net": per_net,
        "floors_applied": floors_applied,
        "warnings": warnings,
        "ignored_checks": ignored_checks_from_pro(pro),
        # How each custom rule is used (report) and the courtyard / rule-area regions the model export turns
        # into ``clearance_regions`` (it has the polygons). Absent without a .kicad_dru rule.
        **({"custom_rules": cr.classify(), "custom_regions": cr.regions, "custom_notes": custom_notes} if cr else {}),
    }
