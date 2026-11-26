"""Headless test doubles for the ports: ``FakeBoard`` (BoardPort over a .kicad_pcb text) and ``ScriptedUi``
(UiPort answering from a script and recording every call). Used by the core's tests and by the preview/DRC
loop tests; no KiCad, no pcbnew (M4-R1).

Complexity: FakeBoard.commit is O(len(board text) + #items) (toporlib.pcbfile); everything else O(1) except
file copies.
"""

from __future__ import annotations

import hashlib
import shutil
import uuid as _uuid
import time
from pathlib import Path

from toporlib import pcbfile

from .config import Config
from .findings import Finding
from .ports import (CommitRequest, CommitResult, Decision, NetClassValues, Progress, RunSummary, Selection)


class FakeBoard:
    """BoardPort over a `.kicad_pcb` file.

    `live_text` is the "live" board state (what KiCad holds in memory) when it differs from the file on
    disk; `save_copy` writes the current in-memory text. `commit` applies the request to that text with
    ``toporlib.pcbfile`` so a second `save_copy` shows the result. Every call is recorded in `calls`;
    commits in `commits`; refills / actions in `refills` / `actions`.
    """

    def __init__(self, pcb: Path, model: dict, *, live_text: str | None = None,
                 selection: Selection | None = None,
                 netclasses: tuple | None = None, project_dir: Path | None = None):
        self.pcb = Path(pcb)
        self.model = model
        self.text = live_text if live_text is not None else self.pcb.read_text()
        self.selection_value = selection if selection is not None else Selection()
        self.netclass_value = netclasses if netclasses is not None else (
            {"Default": NetClassValues("Default")}, {})
        self._project_dir = Path(project_dir) if project_dir else self.pcb.parent
        self.calls: list = []
        self.commits: list = []
        self.refills = 0
        self.actions: list = []
        self.groups: dict = {}  # group name -> uuids created into it

    # ---- BoardPort
    def project_dir(self) -> Path:
        return self._project_dir

    def board_name(self) -> str:
        return self.pcb.name

    def save_copy(self, dst_dir: Path) -> Path:
        self.calls.append(("save_copy", (Path(dst_dir),)))
        dst_dir = Path(dst_dir)
        dst_dir.mkdir(parents=True, exist_ok=True)
        out = dst_dir / self.pcb.name
        out.write_text(self.text)
        for ext in (".kicad_pro", ".kicad_dru"):
            src = self.pcb.with_suffix(ext)
            if src.is_file():
                shutil.copy2(src, dst_dir / src.name)
        return out

    def selection(self) -> Selection:
        self.calls.append(("selection", ()))
        return self.selection_value

    def read_model(self, rules: dict, pcb_copy: Path, *, layer_roles: dict | None = None) -> dict:
        self.calls.append(("read_model", (rules, Path(pcb_copy))))
        return self.model

    def netclasses(self) -> tuple:
        self.calls.append(("netclasses", ()))
        return self.netclass_value

    def net_codes(self) -> dict | None:
        return None  # the fixed model carries its own codes (and rules)

    def fingerprint(self) -> str:
        """Counts + sha256 of the text of every track / via / footprint item (uuid, position, orientation live
        in that text) of the current in-memory board. O(len(text))."""
        self.calls.append(("fingerprint", ()))
        spans = pcbfile.find_items(self.text, ("segment", "arc", "via", "footprint"))
        h = hashlib.sha256()
        counts: dict = {}
        for sp in spans:
            counts[sp.kind] = counts.get(sp.kind, 0) + 1
            h.update(self.text[sp.start:sp.end].encode())
        return f"{sorted(counts.items())}:{h.hexdigest()[:16]}"

    def commit(self, req: CommitRequest) -> CommitResult:
        self.calls.append(("commit", (req,)))
        self.commits.append(req)
        had = {s.uuid for s in pcbfile.find_items(self.text)}
        removed = [u for u in req.remove_uuids if u in had]
        items = [{**it, "uuid": it.get("uuid") or str(_uuid.uuid4())} for it in req.items]
        layer_names = {int(l["id"]): l["name"] for l in self.model.get("layers", [])}
        net_names = {int(n["code"]): n["name"] for n in self.model.get("nets", [])}
        text = pcbfile.remove_items(self.text, removed)
        text = pcbfile.append_items(text, items, layer_names=layer_names, net_names=net_names)
        self.text = pcbfile.replace_zone_fills(text, req.zone_fills)
        ids = [it["uuid"] for it in items]
        grouped = False
        if req.group:
            for it in items:
                if it.get("group"):
                    self.groups.setdefault(it["group"], []).append(it["uuid"])
                    grouped = True
        live = set(pcbfile.parse_zone_fills(self.text))
        missing = sorted(u for u in req.zone_fills if u not in live) if req.zone_fills else []
        return CommitResult(removed=len(removed), created=len(items), created_ids=ids, grouped=grouped,
                            fills_not_applied=missing)

    def refill_zones(self) -> None:
        self.calls.append(("refill_zones", ()))
        self.refills += 1

    def run_action(self, action: str) -> None:
        self.calls.append(("run_action", (action,)))
        self.actions.append(action)


class ScriptedUi:
    """UiPort answering from a script.

    decisions: one Decision (returned every time), a list (consumed in order, the last one repeats) or None
    (proceed with defaults). accept: the `preview` answer (bool, or a list consumed in order, last repeats).
    cancel_at_iteration: `progress` returns False once ``p.iteration >= cancel_at_iteration`` (None = never).
    ok_at_stable (M-B0): act like the progress dialog (``waits_for_ok``: the engine does not stop at "Routes
    stable" by itself) and press OK as soon as a progress says stable.
    discard_at_iteration: Cancel (the progress dialog's stop-and-take-nothing button) once ``p.iteration >= n``:
    ``progress`` returns False and ``stop_discards`` is True from then on. ``discard_when``: the same with a predicate on
    the ``Progress`` (e.g. after the engine is done). Accept = ``cancel_at_iteration`` / ``ok_at_stable``.
    limit_changes: the progress dialog's time-limit control, scripted: ``[(when, seconds), ...]`` in order; `when` is
    a predicate on the ``Progress`` or an int (the n-th ``progress`` call, 1-based); once it holds, the next
    ``time_limit_request`` returns `seconds` (0 = no limit). None (the harness): the limit never changes.
    pauses (pause-button): the progress dialog's Pause, scripted: ``[(when, seconds), ...]`` in order; `when` is a
    predicate on the ``Progress`` (only asked while ``p.pausable``) or an int (the n-th ``progress`` call); once it
    holds, ``pause_wanted`` is True for `seconds` wall seconds (then Resume). `paused_seen`: the progress values seen
    while paused. Accept / Cancel end a pause (as the dialog does).
    Every call is appended to `calls` as ``(method, args)``.
    """

    def __init__(self, decisions=None, accept=True, cancel_at_iteration: int | None = None, ok_at_stable: bool = False,
                 limit_changes=None, discard_at_iteration: int | None = None, discard_when=None, pauses=None):
        self._decisions = decisions
        self._accept = accept
        self.cancel_at_iteration = cancel_at_iteration
        self.ok_at_stable = ok_at_stable
        self.waits_for_ok = ok_at_stable
        self.limit_changes = list(limit_changes or [])
        self.discard_at_iteration, self.discard_when, self.discarded = discard_at_iteration, discard_when, False
        self._progress_n = 0
        self._limit_pending = None
        self.pauses = list(pauses or [])
        self._pause_until: float | None = None
        self.paused_seen: list = []
        self.calls: list = []

    @staticmethod
    def _next(script, default):
        if script is None:
            return default
        if isinstance(script, list):
            if not script:
                return default
            return script.pop(0) if len(script) > 1 else script[0]
        return script

    def names(self) -> list:
        return [c[0] for c in self.calls]

    def first_run_notice(self, toml_path: Path) -> None:
        self.calls.append(("first_run_notice", (toml_path,)))

    def preflight(self, findings: list, summary: RunSummary, cfg: Config) -> Decision:
        self.calls.append(("preflight", (findings, summary, cfg)))
        return self._next(self._decisions, Decision(proceed=True))

    def progress(self, p: Progress) -> bool:
        self.calls.append(("progress", (p,)))
        self._progress_n += 1
        if self.limit_changes:
            when, seconds = self.limit_changes[0]
            if (self._progress_n >= when) if isinstance(when, int) else when(p):
                self.limit_changes.pop(0)
                self._limit_pending = seconds
        if not self.discarded and ((self.discard_at_iteration is not None and p.iteration >= self.discard_at_iteration)
                                   or (self.discard_when is not None and self.discard_when(p))):
            self.discarded = True
        if self.discarded:
            self._pause_until = None
            return False
        if self.ok_at_stable and getattr(p, "stable", False):
            self._pause_until = None
            return False
        stop = self.cancel_at_iteration is not None and p.iteration >= self.cancel_at_iteration
        if stop:
            self._pause_until = None
        elif self._pause_until is not None:
            self.paused_seen.append(p)
            if time.monotonic() >= self._pause_until:
                self._pause_until = None
        elif self.pauses and getattr(p, "pausable", False):
            when, seconds = self.pauses[0]
            if (self._progress_n >= when) if isinstance(when, int) else when(p):
                self.pauses.pop(0)
                self._pause_until = time.monotonic() + float(seconds)
        return not stop

    def pause_wanted(self) -> bool:
        """The scripted Pause (``pauses``) holds now."""
        return self._pause_until is not None

    def stop_discards(self) -> bool:
        """The scripted Cancel (``discard_at_iteration`` / ``discard_when``) was pressed."""
        return self.discarded

    def time_limit_request(self):
        """The scripted time-limit change (``limit_changes``), once; else None."""
        req, self._limit_pending = self._limit_pending, None
        if req is not None:
            self.calls.append(("time_limit_request", (req,)))
        return req

    def preview(self, png: Path, report) -> bool:
        self.calls.append(("preview", (png, report)))
        return bool(self._next(self._accept, True))

    def error(self, finding: Finding) -> None:
        self.calls.append(("error", (finding,)))

    def set_context(self, board=None, report_dir=None, log_path=None, versions=None) -> None:
        self.calls.append(("set_context", (board, report_dir, log_path, versions)))
