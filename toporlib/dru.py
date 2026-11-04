"""`.kicad_dru` custom-rule subset parser and condition matcher. Pure; no pcbnew.

parse_dru is O(T) in tokens; match_condition is O(C) in the condition length; dru_keepouts is
O(K * A) for K rules and A rule-area outlines."""
from __future__ import annotations

import re
from dataclasses import dataclass, field

_UNITS = {"mm": 1_000_000.0, "um": 1_000.0, "mil": 25_400.0, "in": 25_400_000.0, "": 1_000_000.0}


@dataclass
class DruRule:
    name: str
    layer: str | None = None
    condition: str | None = None
    constraints: dict[str, dict[str, int]] = field(default_factory=dict)
    severity: str | None = None
    disallow: tuple[str, ...] = ()  # item kinds of a `(constraint disallow track via ...)`


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
                if sub[1] == "disallow":
                    r.disallow = tuple(str(a) for a in sub[2:] if not isinstance(a, list))
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


# ------------------------------------------------------------------ courtyard clearance rules
_COURTYARD_TERM = re.compile(r"^\s*[AB]\.(?:intersectsCourtyard|insideCourtyard)\('([^']*)'\)\s*$")


def dru_courtyard_clearances(rules: list[DruRule]) -> list[tuple[str, str, int]]:
    """`(rule name, footprint reference, clearance nm)` of the rules whose whole condition is
    `A.intersectsCourtyard('REF')` (or `insideCourtyard`) and that set a clearance minimum: a custom
    rule that replaces the net classes' clearance inside that courtyard (KiCad: the matching rule wins,
    also when it is smaller). Other conditions are not handled here. O(rules)."""
    out = []
    for r in rules:
        m = _COURTYARD_TERM.match(r.condition or "")
        cl = r.constraints.get("clearance", {}).get("min")
        if m and cl is not None and cl >= 0 and r.severity not in ("ignore", "exclude"):
            out.append((r.name, m.group(1), int(cl)))
    return out


# ------------------------------------------------------------------ disallow -> keepouts
_AREA_TERM = re.compile(r"^[AB]\.(?:insideArea|intersectsArea)\('([^']*)'\)$")
_TYPE_TERM = re.compile(r"^[AB]\.Type\s*==\s*'[^']*'$")


def _disallow_areas(cond: str | None) -> list[str] | None:
    """Rule-area names named by a `disallow` condition, or None when the condition is not one of the
    supported shapes (`A.insideArea('x')` / `A.intersectsArea('x')`, optionally `&&` an item-type test)."""
    if cond is None or not cond.strip() or "||" in cond or "!" in cond.replace("!=", ""):
        return None
    names: list[str] = []
    for t in cond.split("&&"):
        t = t.strip()
        m = _AREA_TERM.match(t)
        if m:
            names.append(m.group(1))
        elif not _TYPE_TERM.match(t):
            return None
    return names or None


def dru_keepouts(rules: list[DruRule], areas: list[dict], layer_ids: dict[str, int]) -> tuple[list[dict], list[str]]:
    """Keepouts from `(constraint disallow track|via ...)` rules that name a rule area.

    `areas`: the rule-area outlines as model keepout dicts (``name, layers, poly``), `layer_ids`: copper
    layer name -> engine id. Each matching outline yields ``{"name": "dru:<rule>", "layers", "poly",
    "no_tracks", "no_vias"}`` (layers: the rule's ``(layer ...)`` -- a copper name, ``outer`` or ``inner`` --
    else the area's). Rules that disallow neither track nor via, or are ``ignore``d, give nothing. A rule
    this cannot place (no / unsupported condition, unknown area) gives a ``REFUSE: ...`` warning and no
    keepout. Returns (keepouts, warnings)."""
    out: list[dict] = []
    warns: list[str] = []
    ids = sorted(layer_ids.values())
    outer = {i for i in ids if i in (0, 2)}
    for r in rules:
        if not r.disallow or r.severity in ("ignore", "exclude"):
            continue
        no_tracks, no_vias = "track" in r.disallow, "via" in r.disallow
        if not (no_tracks or no_vias):
            continue
        names = _disallow_areas(r.condition)
        if names is None:
            warns.append(f"REFUSE: rule {r.name} disallows {' '.join(r.disallow)} without a supported "
                         "area condition (A.insideArea('name'))")
            continue
        rule_layers: set[int] | None = None
        if r.layer:
            rule_layers = (outer if r.layer == "outer" else set(ids) - outer if r.layer == "inner"
                           else {layer_ids[r.layer]} if r.layer in layer_ids else None)
        for nm in names:
            hit = [a for a in areas if a["name"] == nm]
            if not hit:
                warns.append(f"REFUSE: rule {r.name} disallows {' '.join(r.disallow)} in rule area '{nm}', "
                             "which does not exist")
            for a in hit:
                lay = list(a["layers"]) if rule_layers is None else sorted(set(a["layers"]) & rule_layers)
                if lay:
                    out.append({"name": f"dru:{r.name}", "layers": lay, "poly": a["poly"],
                                "no_tracks": no_tracks, "no_vias": no_vias})
    return out, warns


def merge_dru_keepouts(keepouts: list[dict], rules: dict, dru_text: str | None,
                       layer_ids: dict[str, int]) -> tuple[list[dict], dict]:
    """`keepouts` (rule-area model dicts) plus the keepouts of the .kicad_dru `disallow` rules; `rules` is
    returned with new REFUSE warnings appended (a copy; the input is not modified). O(K * A)."""
    if not dru_text:
        return keepouts, rules
    extra, warns = dru_keepouts(parse_dru(dru_text), keepouts, layer_ids)
    have = set(rules.get("warnings", []))
    new = [w for w in warns if w not in have]
    if new:
        rules = {**rules, "warnings": [*rules.get("warnings", []), *new]}
    return keepouts + extra, rules
