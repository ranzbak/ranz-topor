"""kicad-cli DRC runner and compact, classified summary (full JSON stays on disk; no pcbnew).

run_drc is O(kicad-cli); classify/split/summarise are O(V + U) in violations + unconnected items."""
from __future__ import annotations

import re

import json
import time
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from toporlib.kicad_cli import kicad_cli, run_cli

ROUTING_TYPES = {
    "clearance",
    "copper_edge_clearance",
    "hole_clearance",
    "hole_to_hole",
    "track_width",
    "track_angle",
    "track_segment_length",
    "tracks_crossing",
    "shorting_items",
    "items_not_allowed",
    "annular_width",
    "via_diameter",
    "drill_out_of_range",
    "connection_width",
}
INCOMPLETE_TYPES = {"track_dangling", "via_dangling"}
SUMMARY_MAX_LINES = 20
SUMMARY_MAX_TYPES = 12


@dataclass
class Violation:
    type: str
    severity: str
    description: str
    items: list[dict] = field(default_factory=list)


@dataclass
class DrcReport:
    path: Path
    exit_code: int
    seconds: float
    violations: list[Violation]
    unconnected: list[dict]
    by_type: dict[str, int]


def run_drc(pcb: Path, out_json: Path, *, save_board: bool = False, timeout_s: float | None = None,
            on_poll=None) -> DrcReport:
    """Run the fixed kicad-cli DRC command; exit 0 and 5 both count as successful runs. `save_board` adds
    ``--save-board``: the refilled board is written back over `pcb`. `timeout_s` / `on_poll` as in
    :func:`toporlib.kicad_cli.run_cli` (kill + KicadCliTimeout / KicadCliCancelled); both None = blocking wait."""
    pcb = Path(pcb)
    out_json = Path(out_json)
    if not pcb.is_file():
        raise RuntimeError(f"DRC input not found: {pcb}")
    cmd = [
        str(kicad_cli()), "pcb", "drc",
        "--format", "json", "--refill-zones", "--all-track-errors", "--exit-code-violations",
        *(["--save-board"] if save_board else []),
        "-o", str(out_json), str(pcb),
    ]
    t0 = time.monotonic()
    proc = run_cli(cmd, timeout_s=timeout_s, on_poll=on_poll)
    seconds = time.monotonic() - t0
    if proc.returncode not in (0, 5):
        raise RuntimeError(
            f"kicad-cli drc failed (exit {proc.returncode}) on {pcb}: {proc.stderr.strip()}"
        )
    doc = json.loads(out_json.read_text())
    violations = [
        Violation(
            type=v["type"],
            severity=v.get("severity", ""),
            description=v.get("description", ""),
            items=list(v.get("items", [])),
        )
        for v in doc.get("violations", [])
    ]
    unconnected = list(doc.get("unconnected_items", []))
    by_type = dict(Counter(v.type for v in violations))
    return DrcReport(
        path=out_json,
        exit_code=proc.returncode,
        seconds=seconds,
        violations=violations,
        unconnected=unconnected,
        by_type=by_type,
    )


def _key(v: Violation) -> tuple:
    """Robust identity of a violation: type + sorted item uuids (never the description text,
    which carries measured values)."""
    return (v.type, tuple(sorted(str(i.get("uuid", "")) for i in v.items)))


def _uuids(v: Violation) -> set[str]:
    return {str(i.get("uuid", "")) for i in v.items}


def _is_pre(v: Violation, seen: set, router_uuids) -> bool:
    """Pre-existing iff it matches the ripped-board baseline and names no router-written item."""
    if not seen or _key(v) not in seen:
        return False
    return not (router_uuids and _uuids(v) & set(router_uuids))


def _unconnected_key(u: dict) -> tuple:
    """Identity of an unconnected-items entry: the sorted uuids of its item pair."""
    return tuple(sorted(str(i.get("uuid", "")) for i in u.get("items", [])))


def _unconnected_desc(u: dict) -> list[str]:
    return [str(i.get("description", "")) for i in u.get("items", [])]


def split_unconnected(report: DrcReport, baseline: DrcReport | None) -> tuple[list[dict], list[dict]]:
    """(new, pre-existing) unconnected items. Pre-existing iff the same item pair (sorted uuids)
    is unconnected in the baseline, the ORIGINAL (hand) board: the board's own open ratsnest.
    Everything else is `new` — connections the router left unrouted as well as pairs that lost
    their connection (e.g. a pad cut off from its pour) — so a router failure is never hidden as
    pre-existing (M3 gate review B-3: the ripped board's ratsnest is the router's own work list)."""
    seen = {_unconnected_key(u) for u in baseline.unconnected} if baseline is not None else set()
    new, pre = [], []
    for u in report.unconnected:
        (pre if _unconnected_key(u) in seen else new).append(u)
    return new, pre


def _split(report: DrcReport, baseline: DrcReport | None,
           router_uuids=None, unconnected_baseline: DrcReport | None = None
           ) -> tuple[dict, dict[str, int]]:
    seen = {_key(v) for v in baseline.violations} if baseline is not None else set()
    routing_new = 0
    routing_pre = 0
    other = 0
    by_type_new: Counter[str] = Counter()
    by_type_pre: Counter[str] = Counter()
    incomplete: Counter[str] = Counter({"track_dangling": 0, "via_dangling": 0})
    for v in report.violations:
        if v.type in INCOMPLETE_TYPES:
            incomplete[v.type] += 1
        elif v.type in ROUTING_TYPES:
            if _is_pre(v, seen, router_uuids):
                routing_pre += 1
                by_type_pre[v.type] += 1
            else:
                routing_new += 1
                by_type_new[v.type] += 1
        else:
            other += 1
    unc_new, unc_pre = split_unconnected(report, unconnected_baseline)
    classified = {
        "routing_new": routing_new,
        "routing_preexisting": routing_pre,
        "by_type_preexisting": dict(by_type_pre),
        "incomplete": {
            "track_dangling": incomplete["track_dangling"],
            "via_dangling": incomplete["via_dangling"],
            "unconnected_items": len(report.unconnected),
            "unconnected_new": len(unc_new),
            "unconnected_preexisting": len(unc_pre),
        },
        "unconnected_preexisting_items": [_unconnected_desc(u) for u in unc_pre],
        "other": other,
        "by_type_new": dict(by_type_new),
    }
    return classified, dict(by_type_pre)


def new_routing(report: DrcReport, baseline: DrcReport | None,
                router_uuids=None) -> list[Violation]:
    """Routing violations of `report` that are not pre-existing (same rule as `classify`)."""
    seen = {_key(v) for v in baseline.violations} if baseline is not None else set()
    return [v for v in report.violations
            if v.type in ROUTING_TYPES and not _is_pre(v, seen, router_uuids)]


def classify(report: DrcReport, baseline: DrcReport | None, router_uuids=None,
             unconnected_baseline: DrcReport | None = None) -> dict:
    """Split violations into routing (new / pre-existing), incomplete and other buckets.

    `baseline` is the DRC of the ripped-but-unrouted board; a violation matching it by
    (type, item uuids) is pre-existing unless one of its items is in `router_uuids`
    (uuids of router-written items: router copper always counts). Unconnected items are
    pre-existing only against `unconnected_baseline`, the original (hand) board's DRC; without
    it every unconnected item is new.
    """
    return _split(report, baseline, router_uuids, unconnected_baseline)[0]


def is_clean(classified: dict) -> bool:
    return classified["routing_new"] == 0


def summarise(board: str, report: DrcReport, baseline: DrcReport | None = None,
              extra: dict | None = None, router_uuids=None,
              unconnected_baseline: DrcReport | None = None) -> str:
    c, pre_by_type = _split(report, baseline, router_uuids, unconnected_baseline)
    inc = c["incomplete"]
    dangling = inc["track_dangling"] + inc["via_dangling"]
    lines = [
        f"DRC {board}: routing_new={c['routing_new']} preexisting={c['routing_preexisting']} "
        f"other={c['other']} incomplete={dangling}/{inc['unconnected_items']} "
        f"(unconnected new {inc['unconnected_new']}, preexisting {inc['unconnected_preexisting']}) "
        f"exit={report.exit_code} {report.seconds:.1f}s"
    ]
    types = set(c["by_type_new"]) | set(pre_by_type)
    ranked = sorted(types, key=lambda t: (-c["by_type_new"].get(t, 0), t))
    for t in ranked[:SUMMARY_MAX_TYPES]:
        lines.append(f"  {t}: {c['by_type_new'].get(t, 0)} (+{pre_by_type.get(t, 0)})")
    for k, v in (extra or {}).items():
        lines.append(f"  {k}: {v}")
    return "\n".join(lines[:SUMMARY_MAX_LINES])


_BRACKET = re.compile(r"\[([^\]]*)\]")


def _net_of(desc: str) -> str:
    m = _BRACKET.search(desc or "")
    return m.group(1) if m else "?"


def ratsnest_marks(unconnected) -> dict:
    """KiCad DRC ``unconnected_items`` sorted for drawing (K1). A zone item's ``pos`` is the zone outline's anchor
    point, not the place of the break, so a zone end is never drawn there:

    * ``lines``: ``[(x0, y0, x1, y1)]`` mm, two non-zone items that are apart;
    * ``rings``: ``[(x, y, desc)]`` mm, the non-zone end of an item <-> zone pair (a pad, via or track not joined to
      its pour);
    * ``islands``: ``{net: n}``, zone <-> zone pairs: pieces of a pour cut off from the rest (no place to draw);
    * ``unjoined``: ``[desc]`` of the ring items.

    O(U)."""
    out = {"lines": [], "rings": [], "islands": {}, "unjoined": []}
    for u in unconnected or ():
        its = [i for i in u.get("items", []) if "pos" in i]
        zone = [str(i.get("description", "")).startswith("Zone") for i in its]
        if len(its) == 2 and all(zone):
            n = _net_of(its[0].get("description", ""))
            out["islands"][n] = out["islands"].get(n, 0) + 1
        elif any(zone):
            for i, z in zip(its, zone):
                if not z:
                    out["rings"].append((float(i["pos"]["x"]), float(i["pos"]["y"]), str(i.get("description", ""))))
                    out["unjoined"].append(str(i.get("description", "")))
        elif len(its) == 2:
            a, b = its
            out["lines"].append((float(a["pos"]["x"]), float(a["pos"]["y"]), float(b["pos"]["x"]), float(b["pos"]["y"])))
    return out


def ratsnest_caption(marks: dict) -> str:
    """Caption tail such as ' · 3 GND pour pieces cut off, 2 not joined to their pour (rings)', or ''. O(nets)."""
    parts = [f"{n} {net} pour piece{'s' if n != 1 else ''} cut off" for net, n in sorted(marks["islands"].items())]
    if marks["rings"]:
        parts.append(f"{len(marks['rings'])} not joined to their pour (rings)")
    return (" · " + ", ".join(parts)) if parts else ""
