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

_UNITS = {"mm": 1_000_000.0, "um": 1_000.0, "mil": 25_400.0, "in": 25_400_000.0, "": 1_000_000.0}


@dataclass
class DruRule:
    name: str
    layer: str | None = None
    condition: str | None = None
    constraints: dict[str, dict[str, int]] = field(default_factory=dict)
    severity: str | None = None


# ------------------------------------------------------------------ s-expression
def _tokenize(text: str) -> list[str]:
    toks, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
        elif c == "#":  # comment to end of line
            while i < n and text[i] != "\n":
                i += 1
        elif c in "()":
            toks.append(c)
            i += 1
        elif c == '"':
            j, buf = i + 1, []
            while j < n and text[j] != '"':
                if text[j] == "\\" and j + 1 < n:
                    j += 1
                buf.append(text[j])
                j += 1
            toks.append("\0" + "".join(buf))  # \0 marks a quoted string
            i = j + 1
        else:
            j = i
            while j < n and not text[j].isspace() and text[j] not in "()":
                j += 1
            toks.append(text[i:j])
            i = j
    return toks


def _parse_sexp(toks: list[str]) -> list:
    stack: list[list] = [[]]
    for t in toks:
        if t == "(":
            stack.append([])
        elif t == ")":
            if len(stack) > 1:
                done = stack.pop()
                stack[-1].append(done)
        else:
            stack[-1].append(t[1:] if t.startswith("\0") else t)
    while len(stack) > 1:  # tolerate unbalanced input
        done = stack.pop()
        stack[-1].append(done)
    return stack[0]


def _value_nm(tok: str) -> int | None:
    m = re.fullmatch(r"(-?\d*\.?\d+(?:[eE][-+]?\d+)?)\s*(mm|um|mil|in|deg)?", tok)
    if not m:
        return None
    v, unit = float(m.group(1)), m.group(2) or ""
    if unit == "deg":
        return int(round(v))
    return int(round(v * _UNITS[unit]))


def parse_dru(text: str) -> list[DruRule]:
    rules: list[DruRule] = []
    for form in _parse_sexp(_tokenize(text)):
        if not (isinstance(form, list) and form and form[0] == "rule" and len(form) > 1):
            continue
        r = DruRule(name=str(form[1]) if not isinstance(form[1], list) else "")
        for sub in form[2:]:
            if not (isinstance(sub, list) and sub):
                continue
            head = sub[0]
            if head == "condition" and len(sub) > 1:
                r.condition = str(sub[1])
            elif head == "layer" and len(sub) > 1:
                r.layer = str(sub[1])
            elif head == "severity" and len(sub) > 1:
                r.severity = str(sub[1])
            elif head == "constraint" and len(sub) > 1:
                vals: dict[str, int] = {}
                for arg in sub[2:]:
                    if isinstance(arg, list) and len(arg) > 1 and arg[0] in ("min", "max", "opt"):
                        v = _value_nm(str(arg[1]))
                        if v is not None:
                            vals[arg[0]] = v
                r.constraints[str(sub[1])] = vals
        rules.append(r)
    return rules


# ------------------------------------------------------------------ conditions
_TERM_RES = [
    ("class", re.compile(r"^[AB]\.NetClass\s*==\s*'([^']*)'$")),
    ("name", re.compile(r"^[AB]\.NetName\s*==\s*'([^']*)'$")),
    ("hasclass", re.compile(r"^[AB]\.hasNetclass\('([^']*)'\)$")),
    ("member", re.compile(r"^[AB]\.memberOfFootprint\('([^']*)'\)$")),
    ("type", re.compile(r"^[AB]\.Type\s*==\s*'([^']*)'$")),
]


def _match_term(term: str, net_name: str, netclass: str, ref: str | None) -> bool | None:
    term = term.strip()
    for kind, rx in _TERM_RES:
        m = rx.match(term)
        if not m:
            continue
        arg = m.group(1)
        if kind in ("class", "hasclass"):
            return arg in netclass.split(",")
        if kind == "name":
            return arg == net_name
        if kind == "type":
            return arg.lower() == "track"
        return None if ref is None else ref == arg  # member
    return None


def match_condition(cond: str | None, *, net_name: str, netclass: str, ref: str | None) -> bool | None:
    """True/False for the supported shapes, None for anything else (caller falls back to strictest)."""
    if cond is None or not cond.strip():
        return True
    if "||" in cond or "!" in cond.replace("!=", ""):
        return None
    results = [_match_term(t, net_name, netclass, ref) for t in cond.split("&&")]
    if any(r is False for r in results):
        return False
    return None if any(r is None for r in results) else True


# ------------------------------------------------------------------ resolution
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
