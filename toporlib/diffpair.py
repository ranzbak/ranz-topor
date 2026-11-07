"""Differential pairs (D7): KiCad's name rule and the pair dimensions, without pcbnew.

Detection is KiCad 10's ``MatchDpSuffix`` (``pcbnew/board.cpp:2729``, identical in ``drc_engine.cpp:2442``;
docs/dev/kicad-diff-pairs.md section 1): walk the *full* net name (sheet path included) from its end, skip the
characters ``0-9`` and ``_``, and the first other character decides: ``+`` / ``P`` = positive half, ``-`` / ``N`` =
negative half (upper case only), anything else = not a pair. The partner is the same name with that one character
swapped. A name whose partner net does not exist is no pair (silently, as in KiCad). This module is the one
definition: the plugin passes the resulting pairs to the engine in ``model["rules"]["diff_pairs"]`` and the
pre-flight / priority code uses the same function.

Dimensions (doc section 2): pair width = netclass ``diff_pair_width``; gap (edge to edge) = netclass
``diff_pair_gap``, overridden by a custom rule ``diff_pair_gap (opt ..)`` (the last matching rule in the file
wins); both floored by the board minimums (KiCad: ``max(min track width, rule)``, ``max(min clearance, rule)``;
topor's board minimums are the resolved, fab-floored ones). Via gap = ``max(pair gap, hole-to-hole - 2 x annular,
copper-to-hole - annular)`` (``EffectiveDiffPairViaGap``; in netclass mode KiCad takes the pair gap, not the
netclass field ``diff_pair_via_gap``). Max uncoupled length and max skew come only from custom rules
(``diff_pair_uncoupled (max ..)``, ``skew (max ..)``); gap min/max for checking only from ``diff_pair_gap``.

Complexity: detection O(N x L) for N nets of length L; dimensions O(P x R) for P pairs and R custom rules.
"""

from __future__ import annotations

import fnmatch

from .dru import DruRule, match_condition, parse_dru

_SKIP = set("0123456789_")
_POL = {"+": (1, "-"), "-": (-1, "+"), "P": (1, "N"), "N": (-1, "P")}


def match_dp_suffix(name: str):
    """``(polarity, complement name, index of the polarity character)`` for a name that ends like a pair half
    (KiCad ``MatchDpSuffix``), else None. ``CLK_N1`` -> ``(-1, "CLK_P1", 4)``. O(len)."""
    i = len(name) - 1
    while i >= 0 and name[i] in _SKIP:
        i -= 1
    if i < 0 or name[i] not in _POL:
        return None
    pol, other = _POL[name[i]]
    return pol, name[:i] + other + name[i + 1:], i


def base_name(name: str) -> str:
    """The pair's base name (the polarity character removed), as KiCad's ``inDiffPair('...')`` matches it."""
    m = match_dp_suffix(name)
    return name if m is None else name[:m[2]] + name[m[2] + 1:]


def detect_pairs(names) -> list[tuple[str, str]]:
    """Every ``(positive, negative)`` pair of net names present in `names` (each pair once, sorted by the
    positive name). O(N x L)."""
    have = set(names)
    out = set()
    for n in have:
        m = match_dp_suffix(n)
        if m is None or m[1] not in have:
            continue
        out.add((n, m[1]) if m[0] > 0 else (m[1], n))
    return sorted(out)


def pair_nets(names) -> set[str]:
    """Names of all nets that are a half of a pair (both halves present). O(N x L)."""
    return {x for p in detect_pairs(names) for x in p}


def _class_field(classes_pro: dict, ncname: str, field: str):
    """First class of the (composite) name that sets `field` (> 0), else the Default class, else None."""
    parts = [ncname] if ncname in classes_pro else ncname.split(",")
    for p in parts + ["Default"]:
        c = classes_pro.get(p) or {}
        v = c.get(field)
        try:
            if v is not None and float(v) > 0:
                return int(round(float(v) * 1e6))
        except (TypeError, ValueError):
            continue
    return None


def _in_diff_pair_term(cond: str, base: str) -> bool | None:
    """Evaluate ``A.inDiffPair('pat')`` / ``A.isCoupledDiffPair()`` terms of a condition for a pair net; None
    when the condition has other terms (the caller then uses ``match_condition``)."""
    import re  # noqa: PLC0415

    terms = [t.strip() for t in cond.split("&&")]
    res = []
    for t in terms:
        m = re.match(r"^[AB]\.inDiffPair\('([^']*)'\)$", t)
        if m:
            res.append(fnmatch.fnmatchcase(base, m.group(1)))
        elif re.match(r"^[AB]\.isCoupledDiffPair\(\)$", t):
            res.append(True)
        else:
            res.append(None)
    if any(r is False for r in res):
        return False
    return True if all(r is True for r in res) else None


def _matches(r: DruRule, net: str, netclass: str) -> bool:
    if r.severity in ("ignore", "exclude"):
        return False
    cond = r.condition or ""
    if "inDiffPair" in cond or "isCoupledDiffPair" in cond:
        v = _in_diff_pair_term(cond, base_name(net))
        if v is not None:
            return v
        # mixed terms: the diff-pair terms count as true for a pair net, the rest as match_condition says
        rest = " && ".join(t for t in cond.split("&&") if "DiffPair" not in t)
        return bool(match_condition(rest, net_name=net, netclass=netclass, ref=None)) if rest.strip() else True
    return bool(match_condition(cond, net_name=net, netclass=netclass, ref=None))


def resolve_pairs(names_codes: dict, *, pro: dict, dru_text: str | None, netclass_of: dict, rules: dict,
                  settings: dict | None = None) -> tuple[list[dict], list[str]]:
    """The model's ``rules["diff_pairs"]`` entries and report lines.

    `names_codes`: net name -> code. `pro`: the parsed ``.kicad_pro``. `netclass_of`: net name -> effective class.
    `rules`: the resolved rules (``min_track_width``, ``min_clearance``, ``hole_to_hole``, ``per_net`` via sizes).
    `settings`: ``[diffpair]`` values (``detect``, ``width_source``, ``gap_mm``, ``via_gap_mm``,
    ``gap_tolerance_mm``, ``max_uncoupled_mm``, ``skew_max_mm``). Each entry (nm): ``p``, ``n`` (codes), ``p_name``,
    ``n_name``, ``netclass``, ``width``, ``gap``, ``via_gap``, ``gap_min``, ``gap_max`` (0 = none: only a custom rule's min / max raise violations; KiCad's netclass gap
    rule has no max and its min is the board minimum), ``tolerance``,
    ``max_uncoupled`` (0 = none), ``max_skew`` (0 = none), the rule names that set them (``rule_gap``,
    ``rule_uncoupled``, ``rule_skew``) and ``sources`` (where each value came from). O(P x R)."""
    st = {"detect": "kicad", "width_source": "pair", "gap_mm": 0.0, "via_gap_mm": 0.0, "gap_tolerance_mm": 0.01,
          "max_uncoupled_mm": 0.0, "skew_max_mm": 0.0, "pairs": {}}
    st.update({k: v for k, v in (settings or {}).items() if v is not None})
    notes: list[str] = []
    pairs = [] if st["detect"] == "off" else detect_pairs(names_codes)
    for p_name, n_name in sorted((st.get("pairs") or {}).items()):
        if p_name in names_codes and n_name in names_codes and (p_name, n_name) not in pairs:
            pairs.append((p_name, n_name))
        elif p_name not in names_codes or n_name not in names_codes:
            notes.append(f"diff pair [diffpair.pairs] {p_name} / {n_name}: net not on the board, ignored")
    classes_pro = {c.get("name", ""): c for c in ((pro.get("net_settings") or {}).get("classes") or [])}
    dru = parse_dru(dru_text) if dru_text else []
    min_w, min_cl = int(rules.get("min_track_width") or 0), int(rules.get("min_clearance") or 0)
    out = []
    for p_name, n_name in pairs:
        nc = netclass_of.get(p_name, "Default") or "Default"
        src = {}
        nr = (rules.get("per_net") or {}).get(str(names_codes[p_name])) or rules.get("default") or {}
        if st["width_source"] == "track":
            width, src["width"] = int(nr.get("width") or min_w), "track width"
        else:
            w = _class_field(classes_pro, nc, "diff_pair_width")
            width, src["width"] = (w, f"netclass '{nc}' diff_pair_width") if w else (min_w, "board minimum track width")
        if width < min_w:
            src["width"] += f" (raised to the board minimum {min_w / 1e6:g} mm)"
            width = min_w
        g = _class_field(classes_pro, nc, "diff_pair_gap")
        gap, src["gap"] = (g, f"netclass '{nc}' diff_pair_gap") if g else (min_cl, "board minimum clearance")
        rule_gap, gap_min, gap_max = f"netclass '{nc}' (diff pair)", 0, 0
        unc, rule_unc, skew, rule_skew = 0, "", 0, ""
        # custom rules: last in file wins (KiCad evaluates them in reverse order, first match stops)
        for r in reversed(dru):
            if not _matches(r, p_name, nc):
                continue
            c = r.constraints
            if "diff_pair_gap" in c and rule_gap.startswith("netclass"):
                v = c["diff_pair_gap"]
                if "opt" in v:
                    gap, src["gap"] = v["opt"], f"rule '{r.name}' diff_pair_gap opt"
                rule_gap = r.name
                gap_min, gap_max = v.get("min", 0), v.get("max", 0)
            if "diff_pair_uncoupled" in c and not rule_unc and c["diff_pair_uncoupled"].get("max"):
                unc, rule_unc = c["diff_pair_uncoupled"]["max"], r.name
            if "skew" in c and not rule_skew and c["skew"].get("max"):
                skew, rule_skew = c["skew"]["max"], r.name
        if st["gap_mm"]:
            gap, src["gap"] = int(round(float(st["gap_mm"]) * 1e6)), "[diffpair] gap_mm"
        if gap < min_cl:
            src["gap"] += f" (raised to the board minimum clearance {min_cl / 1e6:g} mm)"
            gap = min_cl
        via_d = int(nr.get("via_d") or 0)
        via_drill = int(nr.get("via_drill") or 0)
        ann = max(0, (via_d - via_drill) // 2)
        h2h = int(rules.get("hole_to_hole") or 0)
        c2h = int(rules.get("via_hole_clearance") or rules.get("hole_clearance") or 0)
        via_gap = max(gap, h2h - 2 * ann, c2h - ann)
        src["via_gap"] = "pair gap, floored by hole-to-hole and copper-to-hole (KiCad EffectiveDiffPairViaGap)"
        if st["via_gap_mm"]:
            via_gap, src["via_gap"] = int(round(float(st["via_gap_mm"]) * 1e6)), "[diffpair] via_gap_mm"
        if st["max_uncoupled_mm"]:
            unc, rule_unc = int(round(float(st["max_uncoupled_mm"]) * 1e6)), "[diffpair] max_uncoupled_mm"
        if st["skew_max_mm"]:
            skew, rule_skew = int(round(float(st["skew_max_mm"]) * 1e6)), "[diffpair] skew_max_mm"
        out.append({
            "p": int(names_codes[p_name]), "n": int(names_codes[n_name]), "p_name": p_name, "n_name": n_name,
            "netclass": nc, "width": int(width), "gap": int(gap), "via_gap": int(via_gap),
            "via_d": via_d, "via_drill": via_drill,
            "gap_min": int(gap_min), "gap_max": int(gap_max),
            "tolerance": int(round(float(st["gap_tolerance_mm"]) * 1e6)),
            "max_uncoupled": int(unc), "max_skew": int(skew),
            "rule_gap": rule_gap if (gap_min or gap_max) else "",
            "rule_uncoupled": rule_unc, "rule_skew": rule_skew, "sources": src,
        })
        notes.append(f"diff pair {p_name} / {n_name}: width {width / 1e6:g} mm ({src['width']}), gap "
                     f"{gap / 1e6:g} mm ({src['gap']}), via gap {via_gap / 1e6:g} mm, max uncoupled "
                     + (f"{unc / 1e6:g} mm ({rule_unc})" if unc else "none") + ", max skew "
                     + (f"{skew / 1e6:g} mm ({rule_skew})" if skew else "none"))
    return out, notes
