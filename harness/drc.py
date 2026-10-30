"""kicad-cli DRC runner and compact, classified summary (full JSON stays on disk)."""
from __future__ import annotations

import json
import subprocess
import time
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from harness.paths import kicad_cli

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


def run_drc(pcb: Path, out_json: Path) -> DrcReport:
    """Run the fixed kicad-cli DRC command; exit 0 and 5 both count as successful runs."""
    pcb = Path(pcb)
    out_json = Path(out_json)
    if not pcb.is_file():
        raise RuntimeError(f"DRC input not found: {pcb}")
    cmd = [
        str(kicad_cli()), "pcb", "drc",
        "--format", "json", "--refill-zones", "--all-track-errors", "--exit-code-violations",
        "-o", str(out_json), str(pcb),
    ]
    t0 = time.monotonic()
    proc = subprocess.run(cmd, capture_output=True, text=True)
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
    return (v.type, v.description, tuple(sorted(str(i.get("uuid", "")) for i in v.items)))


def _split(report: DrcReport, baseline: DrcReport | None) -> tuple[dict, dict[str, int]]:
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
            if _key(v) in seen:
                routing_pre += 1
                by_type_pre[v.type] += 1
            else:
                routing_new += 1
                by_type_new[v.type] += 1
        else:
            other += 1
    classified = {
        "routing_new": routing_new,
        "routing_preexisting": routing_pre,
        "incomplete": {
            "track_dangling": incomplete["track_dangling"],
            "via_dangling": incomplete["via_dangling"],
            "unconnected_items": len(report.unconnected),
        },
        "other": other,
        "by_type_new": dict(by_type_new),
    }
    return classified, dict(by_type_pre)


def classify(report: DrcReport, baseline: DrcReport | None) -> dict:
    """Split violations into routing (new / pre-existing), incomplete and other buckets."""
    return _split(report, baseline)[0]


def is_clean(classified: dict) -> bool:
    return classified["routing_new"] == 0


def summarise(board: str, report: DrcReport, baseline: DrcReport | None = None,
              extra: dict | None = None) -> str:
    c, pre_by_type = _split(report, baseline)
    inc = c["incomplete"]
    dangling = inc["track_dangling"] + inc["via_dangling"]
    lines = [
        f"DRC {board}: routing_new={c['routing_new']} preexisting={c['routing_preexisting']} "
        f"other={c['other']} incomplete={dangling}/{inc['unconnected_items']} "
        f"exit={report.exit_code} {report.seconds:.1f}s"
    ]
    types = set(c["by_type_new"]) | set(pre_by_type)
    ranked = sorted(types, key=lambda t: (-c["by_type_new"].get(t, 0), t))
    for t in ranked[:SUMMARY_MAX_TYPES]:
        lines.append(f"  {t}: {c['by_type_new'].get(t, 0)} (+{pre_by_type.get(t, 0)})")
    for k, v in (extra or {}).items():
        lines.append(f"  {k}: {v}")
    return "\n".join(lines[:SUMMARY_MAX_LINES])
