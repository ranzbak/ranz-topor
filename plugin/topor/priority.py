"""Net priorities (46 §2.1): ``@N`` name marker > ``topor_pN`` netclass > ``[nets] priority`` > none; power nets
(netclass name containing "power", or ``power_pattern`` for nets still in ``Default``) get tier 2 unless one of
those set the priority. Diff-pair halves share the higher priority (lower number). Names are only read.

The engine option ``net_priority`` (net name -> 1..3) is built from the result by run.build_options.

Complexity: ``parse_marker`` O(len(name)); ``net_priorities`` O(N * (len(name) + #priority patterns)) for N nets
(one pass per source, pair detection is a dict lookup per net).
"""

from __future__ import annotations

import fnmatch
import re

from .config import Config
from .findings import Finding

_OVERBAR = re.compile(r"~\{([^}]*)\}")
_AT = re.compile(r"@(\d+)")
_NETCLASS_P = re.compile(r"^topor_p([1-3])$")
_PAIR_SUFFIXES = (("_P", "_N"), ("+", "-"), ("P", "N"))


def _segment(net_name: str) -> str:
    """Last path segment with the overbar markup removed (``~{X}`` -> ``X``). O(len)."""
    return _OVERBAR.sub(r"\1", net_name.rsplit("/", 1)[-1])


def parse_marker(net_name: str) -> int | None:
    """Priority 1..3 from ``@N`` (N one digit, not followed by another digit) in the last path segment, else
    None. O(len(net_name))."""
    for m in _AT.finditer(_segment(net_name)):
        if len(m.group(1)) == 1 and m.group(1) in "123":
            return int(m.group(1))
    return None


def _marker_ignored(net_name: str) -> bool:
    """The name carries an ``@<digit>`` that is not a valid marker (``DQ@13``, ``A@4``). O(len)."""
    return bool(_AT.search(_segment(net_name))) and parse_marker(net_name) is None


def _class_priority(classes: str) -> int | None:
    best = None
    for c in classes.split(","):
        m = _NETCLASS_P.match(c.strip())
        if m:
            best = int(m.group(1)) if best is None else min(best, int(m.group(1)))
    return best


def _valid(v) -> bool:
    return isinstance(v, int) and not isinstance(v, bool) and 1 <= v <= 3


def _toml_priority(name: str, table: dict) -> int | None:
    if name in table:
        return int(table[name]) if _valid(table[name]) else None
    hits = [int(v) for pat, v in table.items()
            if _valid(v) and any(ch in pat for ch in "*?[") and fnmatch.fnmatchcase(name, pat)]
    return min(hits) if hits else None


def _pair_key(name: str):
    """(key, side) when the name ends like a diff-pair half, else None. KiCad matches the last characters."""
    name = _AT.sub("", name, count=1)  # halves that differ only in the marker still pair (that is the warning)
    for pos, neg in _PAIR_SUFFIXES:
        if name.endswith(pos):
            return name[:-len(pos)], 0
        if name.endswith(neg):
            return name[:-len(neg)], 1
    return None


def net_priorities(nets, netclass_of: dict, cfg: Config) -> tuple:
    """(``{net name: 1..3}`` for every net that has one, findings). Findings: MARKER_IGNORED (invalid marker,
    one finding listing all), MARKER_CONFLICT (sources disagree, the stronger source wins), MARKER_PAIR (halves
    with different priorities), POWER_BY_NAME (info: nets treated as power by name guess). O(N * (len + P))."""
    nets = list(nets)
    table = cfg.get("nets", "priority")
    try:
        power_re = re.compile(cfg.get("nets", "power_pattern"))
    except re.error:
        power_re = None
    prio: dict = {}
    ignored, conflicts, power_by_name = [], [], []
    for n in nets:
        cls = netclass_of.get(n, "") or "Default"
        srcs = [v for v in (parse_marker(n), _class_priority(cls), _toml_priority(n, table)) if v is not None]
        if _marker_ignored(n):
            ignored.append(n)
        if srcs:
            prio[n] = srcs[0]
            if len(set(srcs)) > 1:
                conflicts.append(n)
        elif any("power" in c.lower() for c in cls.split(",")):
            prio[n] = 2
        elif power_re is not None and all(c.strip() == "Default" for c in cls.split(",")) and power_re.search(n):
            prio[n] = 2
            power_by_name.append(n)
    findings: list = []
    if ignored:
        findings.append(Finding(
            "MARKER_IGNORED", "warning",
            "These net names contain an @ marker that is not a priority marker (@1, @2 or @3 followed by a "
            "non-digit), so they get no marker priority: " + ", ".join(ignored) + ".",
            "Rename the net in the schematic with a separator after the marker, e.g. DQ@1_0 instead of DQ@10, "
            "or set the priority in topor.toml [nets] priority.", tuple(ignored)))
    if conflicts:
        findings.append(Finding(
            "MARKER_CONFLICT", "warning",
            "The priority of these nets is set in more than one place with different values; the @N marker wins "
            "over a topor_pN netclass, which wins over [nets] priority: " + ", ".join(conflicts) + ".",
            "Make them agree: KiCad → File → Board Setup → Design Rules → Net Classes (topor_p1..p3) or the @N "
            "marker in the net name, or edit topor.toml [nets] priority.", tuple(conflicts)))
    pairs: dict = {}
    for n in nets:
        k = _pair_key(n)
        if k:
            pairs.setdefault(k[0], {})[k[1]] = n
    mismatched = []
    for halves in pairs.values():
        if len(halves) != 2:
            continue
        a, b = halves[0], halves[1]
        pa, pb = prio.get(a), prio.get(b)
        if pa is None and pb is None:
            continue
        if pa is not None and pb is not None and pa != pb:
            mismatched += [a, b]
        prio[a] = prio[b] = min(x for x in (pa, pb) if x is not None)
    if mismatched:
        findings.append(Finding(
            "MARKER_PAIR", "warning",
            "The halves of these differential pairs have different priorities; the pair is routed with the "
            "higher one: " + ", ".join(mismatched) + ".",
            "Use the same @N marker on both halves before the polarity suffix (USB_D@1+ / USB_D@1-), or set both "
            "in topor.toml [nets] priority.", tuple(mismatched)))
    if power_by_name:
        findings.append(Finding(
            "POWER_BY_NAME", "info",
            "These nets are in the Default netclass and were treated as power nets (priority 2) because their "
            "name matches power_pattern: " + ", ".join(power_by_name) + ".",
            "Put them in a power netclass: KiCad → File → Board Setup → Design Rules → Net Classes, or change "
            "topor.toml [nets] power_pattern.", tuple(power_by_name)))
    return prio, findings
