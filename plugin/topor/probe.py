"""Fail-fast probe (robust-cdt, user 2026-11-07): can the engine read this board at all?

Right after the board is read (``run._run``, after the "model:" log line) and before the pre-flight DRC and the
settings dialog, the engine builds the board's projection (``topor_engine.probe_board``: the triangulation of
every pad, via, rule area, copper graphic and the outline that a route builds first) without routing. When that
fails, every connection of a route would fail with the same reason, so the run stops at once with the
ENGINE_BOARD_UNREADABLE error naming the copper involved, instead of after the DRC (about 40 s on a large
board), the dialog and the router.

Also: :func:`engine_failure` turns a route result whose connections all (or mostly) failed with the same
engine-level reason into the ENGINE_FAILED error (never a quiet "rejected" run that routed nothing).

An engine without ``probe_board`` (older wheel, test fakes) skips the probe. Complexity: one projection build
(seconds on the largest benchmark boards) plus O(connections) for :func:`engine_failure`.
"""

from __future__ import annotations

import json
import threading
from collections import Counter

from .findings import Finding
from .messages import MESSAGES

#: Connection reasons that are the engine's, not the board's: every connection fails with the same one.
ENGINE_REASONS = ("projection failed", "the topological core routes boards with", "no routing engine")
#: share of the connections that must fail with one engine-level reason for ENGINE_FAILED
ENGINE_FAILED_SHARE = 0.5
_ITEMS = 12


def probe_board(engine, model: dict, opts: dict, on_poll=None, poll_s: float = 0.25) -> dict | None:
    """``engine.probe_board`` on a worker thread (the engine releases the GIL), calling `on_poll()` every
    `poll_s` so the progress window stays alive. The probe's dict (``ok``, ``error``, ``places``, ``notes``,
    ``path``, ``seconds``), or None when the engine has no probe. An exception of the probe itself is returned
    as a failed probe."""
    fn = getattr(engine, "probe_board", None)
    if not callable(fn):
        return None
    box: dict = {}

    def work():
        try:
            box["r"] = json.loads(fn(json.dumps(model), json.dumps(opts)))
        except Exception as e:  # noqa: BLE001 - reported as the probe's failure
            box["r"] = {"ok": False, "error": f"{type(e).__name__}: {e}", "places": [], "notes": []}

    t = threading.Thread(target=work, name="topor-probe", daemon=True)
    t.start()
    while t.is_alive():
        t.join(poll_s)
        if on_poll is not None and t.is_alive():
            on_poll()
    return box.get("r")


def unreadable_finding(probe: dict) -> Finding:
    """ENGINE_BOARD_UNREADABLE for a failed probe: the engine's error, and the copper involved as items."""
    places = [str(p) for p in probe.get("places") or []]
    where = f" It failed at {places[0]}." if places else ""
    near = f" Copper nearby: {'; '.join(places[1:3])}{'; and more' if len(places) > 3 else ''}." if len(places) > 1 else ""
    msg = ("The routing engine could not read this board, so Topor stopped before the DRC and the settings "
           f"dialog; the board was not changed.{where}{near} Engine message: {probe.get('error') or 'unknown'}")
    return Finding("ENGINE_BOARD_UNREADABLE", MESSAGES["ENGINE_BOARD_UNREADABLE"].severity, msg,
                   MESSAGES["ENGINE_BOARD_UNREADABLE"].fix, tuple(places[:_ITEMS]))


def engine_failure(result: dict | None) -> Finding | None:
    """ENGINE_FAILED when at least ENGINE_FAILED_SHARE of the connections failed with one engine-level reason
    (``ENGINE_REASONS``), else None. O(connections)."""
    conns = (result or {}).get("connections") or []
    if not conns:
        return None
    by = Counter(str(c.get("reason") or "") for c in conns if c.get("status") != "routed"
                 and str(c.get("reason") or "").startswith(ENGINE_REASONS))
    if not by:
        return None
    reason, n = by.most_common(1)[0]
    if n < ENGINE_FAILED_SHARE * len(conns):
        return None
    msg = (f"{n} of {len(conns)} connection(s) failed with the same engine error, so this is not a normal run that "
           f"happened to route nothing: {reason}")
    return Finding("ENGINE_FAILED", MESSAGES["ENGINE_FAILED"].severity, msg, MESSAGES["ENGINE_FAILED"].fix,
                   (reason[:2000],))
