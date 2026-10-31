"""Minimal run report (B5; B8 replaces the body, keeps this seam): the summary block of the wireframe (section 5,
at most 20 lines) and the report folder files ``report.md``, ``preview.png`` (written by preview.py),
``result.json``, ``drc.json``, ``log.txt``. Satisfies ``ports.Report`` (``dir``, ``summary_lines()``) and carries
``stop_reason`` for the done notice.

Seam for B8: ``RunReport.facts`` is the one dict of numbers the run fills (see ``run.py``); B8 renders the nine
sections of ``report.md`` from it. ``write()`` is called twice per run (before the preview with outcome
``pending``, after the decision with the final one).

Complexity: O(facts + findings) per call; file writes O(size).
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

SUMMARY_MAX_LINES = 20

#: RunOutcome.status -> wording of the ``Outcome:`` line (wireframe section 5).
OUTCOME_TEXT = {
    "pending": "pending — waiting for Accept or Reject",
    "accepted": "applied as one undo step",
    "rejected": "rejected — board unchanged",
    "cancelled": "cancelled — nothing applied",
    "aborted_preflight": "aborted at pre-flight",
}


def outcome_text(status: str, code: str = "") -> str:
    if status == "error":
        return f"error {code}".strip()
    return OUTCOME_TEXT.get(status, status)


def _na(v, fmt="{}"):
    return "n/a" if v is None else fmt.format(v)


class RunReport:
    """Facts + files of one run. `facts` keys (all optional, missing -> n/a): version, board, timestamp, routed,
    total, nets, layers, partial, failed, vias, vias_before, length_mm, length_before_mm, routing_new,
    preexisting, unconnected, detour_line, stop_reason, iterations, elapsed_s, threads, scope, existing_copper,
    respect_locked, non_default, defaults_used, findings (list of Finding), notes (list of str), drc_rounds."""

    def __init__(self, dir: Path, facts: dict | None = None):
        self.dir = Path(dir)
        self.facts: dict = dict(facts or {})
        self.status = "pending"
        self.error_code = ""

    @property
    def stop_reason(self):
        return self.facts.get("stop_reason")

    def set_outcome(self, status: str, error_code: str = "") -> None:
        self.status, self.error_code = status, error_code

    # ---- summary block (wireframe section 5)
    def summary_lines(self) -> list:
        f = self.facts
        fnd = f.get("findings") or []
        sev = lambda s: sum(1 for x in fnd if x.severity == s)  # noqa: E731
        lines = [
            f"Topor {f.get('version', '?')} — {f.get('board', '?')} — {f.get('timestamp', '?')}",
            f"Routed {_na(f.get('routed'))}/{_na(f.get('total'))} connections · {_na(f.get('nets'))} nets · "
            f"{_na(f.get('layers'))} layers",
            f"Partial {_na(f.get('partial'))} · failed {_na(f.get('failed'))}",
            f"Vias {_na(f.get('vias'))} (current board {_na(f.get('vias_before'))})",
            f"Length outside pads {_na(f.get('length_mm'))} mm"
            + (f" (current {f['length_before_mm']} mm)" if f.get("length_before_mm") is not None else ""),
            f"DRC new {_na(f.get('routing_new'))} · pre-existing {_na(f.get('preexisting'))} · "
            f"unconnected {_na(f.get('unconnected'))}",
        ]
        if f.get("detour_line"):
            lines.append(f"Detours {f['detour_line']}")
        lines += [
            f"Stopped: {_na(f.get('stop_reason'))} after {_na(f.get('iterations'))} iterations, "
            f"{_na(f.get('elapsed_s'))} s, {_na(f.get('threads'))} threads",
            f"Scope {f.get('scope', 'whole board')} · existing copper {_na(f.get('existing_copper'))} · "
            f"locked {_na(f.get('respect_locked'))}",
            f"Settings: {_na(f.get('non_default'), '{}')} non-default",
            f"Findings: {sev('error')} errors · {sev('warning')} warnings · {sev('info')} notes",
            f"Outcome: {outcome_text(self.status, self.error_code)}",
            f"Report: {self.dir}",
        ]
        return lines[:SUMMARY_MAX_LINES]

    # ---- files
    def write(self, *, result: dict | None = None, drc_src: Path | None = None) -> None:
        """(Re)write report.md and, when given, result.json / drc.json. O(size)."""
        self.dir.mkdir(parents=True, exist_ok=True)
        f = self.facts
        md = ["# Topor run report", "", "## Summary", "", *[f"    {ln}" for ln in self.summary_lines()], ""]
        fnd = f.get("findings") or []
        md += ["## Findings", ""]
        md += [f"- **{x.code}** ({x.severity}): {x.message}" + (f" Fix: {x.fix}" if x.fix else "") for x in fnd] or ["none"]
        md += ["", "## Notes", ""] + [f"- {n}" for n in (f.get("notes") or [])] if f.get("notes") else []
        md += ["", "## Files", "", "report.md, preview.png, result.json, drc.json, log.txt", ""]
        (self.dir / "report.md").write_text("\n".join(md))
        if result is not None:
            (self.dir / "result.json").write_text(json.dumps(result, indent=1, default=str))
        if drc_src is not None and Path(drc_src).is_file():
            shutil.copyfile(drc_src, self.dir / "drc.json")
