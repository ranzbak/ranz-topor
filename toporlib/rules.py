"""Rule resolution without pcbnew (M4-R1): Board Setup constraints, net classes, pre-defined vias, custom-rule
(.kicad_dru) subset, widths. ``resolve_rules_from_project`` is the pure core; ``harness.rules.resolve_rules`` (SWIG)
and the plugin (IPC) both feed it.

Authority (user decision 2026-11-12): the board's KiCad configuration, resolved as KiCad's DRC resolves it. A value
the project file carries is used as is, a 0 included (KiCad: 0 = no minimum). Only a field the board does not provide
at all (a key missing from the ``.kicad_pro``, a net class value no class of the net sets, a 0 track width / via size
of the Default class) takes the value of :data:`RULE_FALLBACK` (JLCPCB 1 oz derived, never below the value KiCad
itself uses for the missing key, so KiCad's DRC accepts the copper); every fallback used is one entry of the result's
``fallbacks`` and one ``rule fallback:`` warning naming the KiCad setting to fill in. The optional floor mode
``[rules] fab_profile = "jlcpcb_1oz"`` (off by default) raises board values below the JLCPCB 1 oz minimums.

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

#: JLCPCB standard rigid, 1 oz copper (docs/kicad/30-kicad-constraints.md, retrieved 2026-10-30), nm. Used for the
#: :data:`RULE_FALLBACK` values and by the opt-in floor mode ``[rules] fab_profile = "jlcpcb_1oz"``; never sent to
#: the engine (the engine holds no fab profile).
JLCPCB_1OZ: dict = {
    "width": 200_000,
    "clearance": 150_000,
    "via_d": 600_000,
    "via_drill": 300_000,
    "via_d_min_auto": 450_000,
    "via_drill_min_auto": 250_000,
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

#: ``[rules] fab_profile`` values: ``"board"`` (default: Board Setup, the net classes, the pre-defined vias and the
#: .kicad_dru are the only authority, as KiCad's own DRC judges them) or the name of a fab profile whose minimums are
#: FLOORS over them (opt-in, for a Board Setup known to be looser than the fab).
FAB_PROFILES: dict = {"jlcpcb_1oz": JLCPCB_1OZ}
BOARD_AUTHORITY = "board"
DEFAULT_FAB_PROFILE = BOARD_AUTHORITY

#: The value KiCad itself uses for a ``board.design_settings.rules`` key the .kicad_pro does not carry (nm): the
#: BOARD_DESIGN_SETTINGS constructor values (pcbnew/board_design_settings.cpp:151-161 and :215, the DEFAULT_* of
#: board_design_settings.h:79-89), because BOARD_DESIGN_SETTINGS sets ``m_resetParamsIfMissing = false``
#: (board_design_settings.cpp:48: "leave alone parameters that aren't found in the project JSON"; KiCad
#: 86a7a3f4). The annular ring is (0.5 - 0.3) / 2 = 0.1 mm there (the JSON PARAM default 0.5 mm never applies).
KICAD_ABSENT_NM = {
    "min_clearance": 0,
    "min_track_width": 200_000,
    "min_via_diameter": 500_000,
    "min_through_hole_diameter": 300_000,
    "min_via_annular_width": 100_000,
    "min_hole_clearance": 250_000,
    "min_hole_to_hole": 250_000,
    "min_copper_edge_clearance": 500_000,
    "min_connection": 0,
    "max_error": 5_000,  # ARC_HIGH_DEF
}
#: KiCad's values of a net class field nobody set (common/netclass.cpp:40-45 DEFAULT_CLEARANCE / TRACK_WIDTH /
#: VIA_DIAMETER / VIA_DRILL), nm.
KICAD_CLASS_ABSENT_NM = {"clearance": 200_000, "track_width": 200_000, "via_diameter": 600_000, "via_drill": 300_000}
_CLASS_FIELDS = tuple(KICAD_CLASS_ABSENT_NM)

_BS = "KiCad → File → Board Setup → Design Rules → Constraints → "
_NC = "KiCad → File → Board Setup → Design Rules → Net Classes → Default → "
#: The JLCPCB 1 oz value of each field (None: the fab gives no figure) and the KiCad setting that provides it.
_FALLBACK_SRC = {
    "min_clearance": (JLCPCB_1OZ["clearance"], "minimum clearance", _BS + "Minimum clearance"),
    "min_track_width": (JLCPCB_1OZ["width"], "minimum track width", _BS + "Minimum track width"),
    "min_via_diameter": (JLCPCB_1OZ["via_d_min_auto"], "minimum via diameter", _BS + "Minimum via diameter"),
    "min_through_hole_diameter": (JLCPCB_1OZ["via_drill_min_auto"], "minimum through hole",
                                  _BS + "Minimum through hole"),
    "min_via_annular_width": (JLCPCB_1OZ["annular_2l"], "minimum annular width", _BS + "Minimum annular width"),
    "min_hole_clearance": (JLCPCB_1OZ["pth_hole_to_track"], "hole clearance", _BS + "Hole clearance"),
    "min_hole_to_hole": (JLCPCB_1OZ["hole_to_hole_via"], "hole to hole clearance", _BS + "Hole to hole clearance"),
    "min_copper_edge_clearance": (JLCPCB_1OZ["edge"], "copper to edge clearance", _BS + "Copper to edge clearance"),
    "min_connection": (None, "minimum connection width", _BS + "Minimum connection width"),
    "max_error": (None, "maximum allowed deviation (arc approximation)", _BS + "Maximum allowed deviation"),
    "clearance": (JLCPCB_1OZ["clearance"], "net class Default clearance", _NC + "Clearance"),
    "track_width": (JLCPCB_1OZ["width"], "net class Default track width", _NC + "Track Width"),
    "via_diameter": (JLCPCB_1OZ["via_d"], "net class Default via size", _NC + "Via Size"),
    "via_drill": (JLCPCB_1OZ["via_drill"], "net class Default via hole", _NC + "Via Hole"),
}


def _fallback_row(field: str) -> dict:
    jlc, label, setting = _FALLBACK_SRC[field]
    kicad = KICAD_ABSENT_NM.get(field, KICAD_CLASS_ABSENT_NM.get(field, 0))
    return {"field": field, "label": label, "value": kicad if jlc is None else max(int(jlc), kicad),
            "jlcpcb": jlc, "kicad": kicad, "setting": setting}


#: Fallback per field the board does not provide (nm): the JLCPCB 1 oz value, raised to KiCad's own value for the
#: missing key where that is stricter (KiCad's DRC judges the result with its own value; "KiCad tools are the
#: spec"). Fields without a JLCPCB figure (minimum connection width, arc deviation) take KiCad's value.
RULE_FALLBACK: dict = {f: _fallback_row(f) for f in _FALLBACK_SRC}
#: The via drill's clearance when the hole clearance is missing: JLCPCB's via hole to track 0.2 mm, raised to
#: KiCad's 0.25 mm (component holes use RULE_FALLBACK["min_hole_clearance"], JLCPCB's PTH 0.35 mm).
VIA_HOLE_FALLBACK = max(JLCPCB_1OZ["via_hole_to_track"], KICAD_ABSENT_NM["min_hole_clearance"])


def fallback_text(row: dict) -> str:
    """The report line of one used fallback (`row`: a :data:`RULE_FALLBACK` entry). O(1)."""
    src = (f"JLCPCB 1 oz {row['jlcpcb'] / 1e6:g} mm, KiCad's own {row['kicad'] / 1e6:g} mm, the stricter"
           if row.get("jlcpcb") is not None else "KiCad's own value; JLCPCB gives none")
    return (f"rule fallback: {row['label']} is not set by the board -> {row['value'] / 1e6:g} mm used ({src}); "
            f"set it in {row['setting']}")


_POWER_RE = re.compile(r"^(\+|-)?\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)")
_REFUSE = ("track_angle", "track_segment_length")


@dataclass(frozen=True)
class NetClassVals:
    """Net-class values in nm (``None`` = not set by the class). Duck-compatible with
    ``topor.ports.NetClassValues``: any object with these attributes works."""
    name: str
    clearance: int | None = None
    track_width: int | None = None
    via_diameter: int | None = None
    via_drill: int | None = None


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


def board_rule_values(pro: dict) -> tuple[dict[str, int], list[str]]:
    """``board.design_settings.rules`` as nm and the keys the project file does not carry (in
    :data:`KICAD_ABSENT_NM` order). A missing (or non-numeric) key takes its :data:`RULE_FALLBACK` value; a value
    the file carries is used as is, 0 and negative values included (KiCad: 0 = no minimum; -0.01 mm is KiCad's
    legacy copper-to-edge flag, i.e. no minimum here). ``max_error`` is read from ``rules`` and, for older files,
    from ``design_settings``. O(1)."""
    ds = (pro.get("board") or {}).get("design_settings") or {}
    rules = ds.get("rules") or {}
    out: dict[str, int] = {}
    missing: list[str] = []
    for k in KICAD_ABSENT_NM:
        v = rules.get(k, ds.get(k) if k == "max_error" else None)
        try:
            out[k] = _nm(v)
        except (TypeError, ValueError):
            out[k] = RULE_FALLBACK[k]["value"]
            missing.append(k)
    return out, missing


def rule_values_from_pro(pro: dict) -> dict[str, int]:
    """The values of :func:`board_rule_values` (fallbacks filled in). O(1)."""
    return board_rule_values(pro)[0]


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
    copper_layer_count: int = 2,
    net_codes: dict[str, int] | None = None,
    layer_names: dict[int, str] | None = None,
    fab_profile: str = DEFAULT_FAB_PROFILE,
) -> dict:
    """Resolve the design rules the router works to from the board's KiCad configuration.

    Custom rules (`dru_text`) are applied as :mod:`toporlib.custom_rules` describes (per-net values, board-wide
    edge / hole clearance, a region plan in ``custom_regions``, a report in ``custom_rules``); `layer_names`:
    copper layer id -> the board's (user) layer name, so a rule's ``(layer "<user name>")`` matches.

    `pro`: parsed .kicad_pro (Board Setup constraints, pre-defined vias). `netclass_of`: net name -> effective class
    name (``""`` optional -> Default; a composite ``"Power,Default"`` is resolved from `classes` unless it is itself
    a key of `classes`). `classes`: class name -> values (None = unset: the next class of the composite, then the
    Default class, as KiCad composes them). `widths_before`: net name -> object with ``median_nm``. `net_codes`:
    net name -> code for the ``per_net`` keys (default: :func:`net_codes_by_name` of `netclass_of`).

    `fab_profile` (``[rules] fab_profile``): ``"board"`` (default) = the board's values exactly as KiCad's DRC
    judges them: a minimum of 0 stays 0 (no minimum), a matching custom rule's clearance / width / via (per net or
    in a courtyard / rule-area region) is used as given, also below the Board Setup minimum (KiCad drc_engine.cpp
    EvalRules: a matching custom rule returns before the implicit board-minimum rule). A key of
    :data:`FAB_PROFILES` (opt-in floor mode) raises every design value below the profile minimum to it, reported
    in ``floors_applied`` (``[{"name", "design", "floor"}]``, nm) and in ``warnings`` (``floor: <name> <design> ->
    <floor> mm``).

    A field the board does not provide at all takes its :data:`RULE_FALLBACK` value, listed in ``fallbacks``
    (rows of :data:`RULE_FALLBACK`) and in ``warnings`` (:func:`fallback_text`). The result carries ``"profile"``
    (``"board"`` or the floor profile's name; report only, the engine never reads it). The via alternatives are
    the board's pre-defined vias (``predefined_vias``); there is no fab-profile via list.
    """
    board_auth = fab_profile == BOARD_AUTHORITY
    if not board_auth and fab_profile not in FAB_PROFILES:
        raise ValueError(f"unknown [rules] fab_profile {fab_profile!r} (known: "
                         f"{', '.join([BOARD_AUTHORITY, *FAB_PROFILES])})")
    profile = FAB_PROFILES.get(fab_profile)
    settings_table = settings_table or {}
    dv, missing = board_rule_values(pro)
    fallbacks: dict[str, dict] = {k: RULE_FALLBACK[k] for k in missing}
    codes = net_codes if net_codes is not None else net_codes_by_name(netclass_of)
    cr = CustomRules(parse_dru(dru_text) if dru_text else [], canonical_layers(copper_layer_count),
                     {v: canonical_name(k) for k, v in (layer_names or {}).items()})
    cr_nets = [(n, netclass_of.get(n, "Default"), _diff_base(n)) for n in [""] + sorted(codes)] if cr else []
    hole_missing = "min_hole_clearance" in missing
    if cr:
        # Board-wide custom values replace the Board Setup ones (KiCad: the matching custom rule wins over the
        # implicit board rule); a custom value is a board value, so it also replaces a fallback.
        for key, con in (("min_copper_edge_clearance", "edge_clearance"), ("min_hole_clearance", "hole_clearance")):
            got = cr.board(con, dv[key], cr_nets)
            if got != dv[key]:  # a custom rule gives the value (else the Board Setup / fallback value stays)
                dv[key] = got
                fallbacks.pop(key, None)
                hole_missing = hole_missing and key != "min_hole_clearance"
    warnings: list[str] = []
    floors_applied: list[dict] = []
    annular_floor = (profile["annular_2l"] if copper_layer_count <= 2 else profile["annular_4l"]) if profile else 0

    def floored(name: str, design, floor: int) -> int:
        design = int(design)
        if board_auth:  # Board Setup is the authority: its value as is (0 = no minimum, as in KiCad)
            return max(design, 0)
        if design < floor:
            floors_applied.append({"name": name, "design": design, "floor": floor})
            warnings.append(f"floor: {name} {design / 1e6:g} -> {floor / 1e6:g} mm")
            return floor
        return design

    P = profile or {}
    # The drill of a via the router places keeps KiCad's hole clearance (the floor mode: floored by the fab's VIA
    # hole to track, JLCPCB 0.2 mm, not its PTH value 0.35 mm for component holes); a missing hole clearance:
    # VIA_HOLE_FALLBACK for vias, RULE_FALLBACK (PTH) for component holes.
    via_hole_design = VIA_HOLE_FALLBACK if hole_missing else dv["min_hole_clearance"]
    floors = {
        "min_clearance": floored("min_clearance", dv["min_clearance"], P.get("clearance", 0)),
        "min_track_width": floored("min_track_width", dv["min_track_width"], P.get("width", 0)),
        "min_via_diameter": floored("min_via_diameter", dv["min_via_diameter"], P.get("via_d_min_auto", 0)),
        "min_via_drill": floored("min_via_drill", dv["min_through_hole_diameter"], P.get("via_drill_min_auto", 0)),
        "min_annular": floored("min_annular", dv["min_via_annular_width"], annular_floor),
        "hole_clearance": floored("hole_clearance", dv["min_hole_clearance"], P.get("pth_hole_to_track", 0)),
        "via_hole_clearance": floored("via_hole_clearance", via_hole_design,
                                      P.get("via_hole_to_track", P.get("pth_hole_to_track", 0))),
        "hole_to_hole": floored("hole_to_hole", dv["min_hole_to_hole"], P.get("hole_to_hole_via", 0)),
        "edge_clearance": floored("edge_clearance", dv["min_copper_edge_clearance"], P.get("edge", 0)),
        "max_error": dv["max_error"] if dv["max_error"] > 0 else RULE_FALLBACK["max_error"]["value"],
        # Narrowest copper connection KiCad accepts (Board Setup > Constraints > Minimum connection width): a
        # necked pad exit may not go below it. 0 = no limit (KiCad's own value).
        "min_connection": max(int(dv["min_connection"]), 0),
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

    def eff(ncname: str, field: str) -> int:
        """First class of the (composite) name that sets `field`, else Default (KiCad's composition), else the
        fallback (reported). A track width / via size of 0 is no usable size: the next class decides (a 0
        clearance is a real value)."""
        parts = [ncname] if ncname in classes else ncname.split(",")
        for p in parts + ["Default"]:
            c = classes.get(p)
            v = getattr(c, field, None) if c is not None else None
            if v is not None and (int(v) > 0 or field == "clearance"):
                return int(v)
        fallbacks.setdefault(field, RULE_FALLBACK[field])
        return int(RULE_FALLBACK[field]["value"])

    def base_clearance(net_name: str) -> int:
        return max(eff(netclass_of.get(net_name, "Default"), "clearance"), floors["min_clearance"])

    def net_rule(net_name: str) -> dict:
        ncname = netclass_of.get(net_name, "Default")
        clearance = base_clearance(net_name)
        class_w = eff(ncname, "track_width")
        via_d = max(eff(ncname, "via_diameter"), floors["min_via_diameter"])
        via_drill = max(eff(ncname, "via_drill"), floors["min_via_drill"])
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
        elif non_default:
            width, src = class_w, "netclass"
        elif net_name in widths_before:
            width, src = widths_before[net_name].median_nm, "existing_copper"
        elif net_name in settings_table:
            width, src = int(settings_table[net_name]), "settings_table"
        else:
            width, src = class_w, "default"
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
        # the region floor: the fab minimum in the floor mode, none with the board as the authority (KiCad: the
        # custom rule wins)
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

    fb_rows = [fallbacks[k] for k in _FALLBACK_SRC if k in fallbacks]
    warnings.extend(fallback_text(r) for r in fb_rows)
    predefined = predefined_vias_from_pro(pro)
    if via_setup_missing_from(predefined, classes):
        warnings.append("via sizes not set up: no pre-defined via and every net class via is KiCad's factory "
                        "0.8/0.4 mm (the engine's via fallback applies)")
    return {
        **floors,
        "profile": BOARD_AUTHORITY if board_auth else fab_profile,
        "predefined_vias": predefined,
        "default": default,
        "per_net": per_net,
        "floors_applied": floors_applied,
        "fallbacks": [dict(r) for r in fb_rows],
        "warnings": warnings,
        "ignored_checks": ignored_checks_from_pro(pro),
        # How each custom rule is used (report) and the courtyard / rule-area regions the model export turns
        # into ``clearance_regions`` (it has the polygons). Absent without a .kicad_dru rule.
        **({"custom_rules": cr.classify(), "custom_regions": cr.regions, "custom_notes": custom_notes} if cr else {}),
    }
