"""`.kicad_dru` custom-rule subset parser and condition matcher. Pure; no pcbnew.

parse_dru is O(T) in tokens; match_condition is O(C) in the condition length."""
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
