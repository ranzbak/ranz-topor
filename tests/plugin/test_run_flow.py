"""B5 run flow: save_as copy -> rules -> model -> route (progress / cancel) -> DRC loop -> SVG preview ->
Accept = one commit / Reject = nothing -> report folder.

Real engine, real kicad-cli, FakeBoard (eprom snapshot copy + harness-built model; pcbnew only here, in the
test fixture) and ScriptedUi. wx needs an App for the preview renderer: `wx.App(False)`, no window is ever
shown (Ruling C18). Every report folder goes under a tmp TOPOR_REPORT_ROOT (Ruling C1).
"""
import json
from dataclasses import replace
import math
import re
import shutil
import time
from pathlib import Path

import pytest

wx = pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")

from harness.paths import board_pcb  # noqa: E402
from topor import preview, run as run_mod  # noqa: E402
from topor.config import Config  # noqa: E402
from topor.fake import FakeBoard, ScriptedUi  # noqa: E402
from topor.ports import Decision, Report  # noqa: E402
from toporlib import pcbfile  # noqa: E402
from toporlib.drc import classify, run_drc  # noqa: E402

BOARD = "eprom-to-kernal"
MARKER = "11111111-2222-4333-8444-555555555555"  # a valid uuid: kicad-cli re-serialises saved boards
LIVE_EDIT = (f'\t(gr_line\n\t\t(start 190 70)\n\t\t(end 191 70)\n\t\t(stroke\n\t\t\t(width 0.1)\n'
             f'\t\t\t(type default)\n\t\t)\n\t\t(layer "Cmts.User")\n\t\t(uuid "{MARKER}")\n\t)\n')

CFG_DEFAULT_REFILL = Config.defaults().with_values({("run", "existing_copper"): "rip", ("run", "timeout_s"): 60.0})
# refill_after_accept defaults to true since 2026-11-17; most flows below judge Accept = one commit, so they pin false
CFG = CFG_DEFAULT_REFILL.with_values({("run", "refill_after_accept"): False})


@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # noqa: BLE001 - no display
        pytest.skip(f"no display for wx: {e}")
    if not wx.Display.GetCount():
        pytest.skip("no display for wx")
    yield a


@pytest.fixture(autouse=True)
def _report_root(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(tmp_path / "reports"))


@pytest.fixture(scope="module")
def reports(tmp_path_factory):
    mp = pytest.MonkeyPatch()
    root = tmp_path_factory.mktemp("reports")
    mp.setenv("TOPOR_REPORT_ROOT", str(root))
    yield root
    mp.undo()


class Eprom:
    pcb: Path
    model: dict
    text: str


@pytest.fixture(scope="module")
def eprom(tmp_path_factory):
    """Project copy of the snapshot + the harness-built model (pcbnew: test side only)."""
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("proj")
    cp = copy_project(board_pcb(BOARD), d / "eprom-to-kernal")
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    e = Eprom()
    e.model = export_model(b, rules, name=BOARD, dru_text=dru)
    e.pcb = cp.pcb
    e.text = cp.pcb.read_text()
    return e


def make_board(eprom, live=True, **kw):
    text = eprom.text.rstrip()
    live_text = text[:-1].rstrip("\n") + "\n" + LIVE_EDIT + ")\n" if live else None
    return FakeBoard(eprom.pcb, eprom.model, live_text=live_text, project_dir=eprom.pcb.parent, **kw)


def _ok(out):
    if out.status == "error":
        pytest.fail("run failed:\n" + (out.report_dir / "log.txt").read_text()[-2500:])


def _last(ui) -> str:
    """The last UI call that is not ``set_context`` (the run flow sets the context right before every dialog)."""
    return [n for n in ui.names() if n != "set_context"][-1]


def do_run(eprom, tmp_path, *, ui=None, cfg=CFG, engine=topor_engine, board=None, strict=True):
    board = board or make_board(eprom)
    ui = ui or ScriptedUi()
    out = run_mod.run(board, ui, cfg, engine, tmp_root=tmp_path / "work")
    if strict:
        _ok(out)
    return board, ui, out


# ------------------------------------------------------------------ engine doubles
class ProxyJob:
    """Delegates to a real RouteJob; `done()` stays False for `hold` polls so the progress loop runs.

    `end_real` (TW3 / D6, lead 2026-11-19): a dialog run has no cycle cap, so the real job ends only by Cancel / OK or
    its time limit. With it the double presses OK on the real job itself, after `hold` polls, at the first poll whose
    progress shows "Routes stable" with a pick to keep (the user's OK there: the run ends "stable"). The plugin's own
    Cancel / OK stays recorded only, as without it (headless runs, and dialog runs that keep the cap: restart =
    "stall" / "off"), so the real job never ends before it has a plan and the result stays deterministic."""

    def __init__(self, real, hold=1, patch=None, end_real=False):
        self.real, self.hold, self.polls, self.cancelled, self.patch = real, hold, 0, False, patch
        self.end_real, self.real_cancelled, self.last = end_real, False, {}
        self.reached = False
        self.ok = None  # cancel(ok=...): what the button read
        self.hardened = False
        self.limits = []  # set_time_limit calls (the progress dialog's live limit, wall clock)

    def progress(self):
        self.polls += 1
        d = json.loads(self.real.progress())
        self.last = dict(d)
        d["iteration"] = max(d.get("iteration", 0), 1)
        if self.polls < self.hold:
            # time-limit-fix r2: the held polls stand for a long optimizing phase (the real job may be done and
            # drawn already): not drawing, the optimizing time = the job's wall time (no engine figure)
            d["drawing"] = False
            for k in ("optimize_s", "draw_s"):
                d.pop(k, None)
        return json.dumps(d)

    def done(self):
        if self.end_real and self.polls >= self.hold and not self.real_cancelled and self.last.get("stable") \
                and self.last.get("best_total"):
            self.real_cancelled = True
            self.real.cancel(True)  # OK at Routes stable (the progress the plugin polled last)
        return self.polls >= self.hold and self.real.done()

    def cancel(self, ok=None):
        self.cancelled = True  # recorded only: the real job has to finish for a deterministic result
        self.ok = ok  # what the plugin said the button read (user 2026-11-17: the engine latches it)

    def reach_limit(self):
        self.reached = True  # M2.TL soft limit (CPU clock): recorded only, as cancel

    def harden(self):
        self.hardened = True  # F3 overrun cap (CPU clock): recorded only, as cancel

    def set_time_limit(self, seconds):
        self.limits.append(seconds)
        self.real.set_time_limit(seconds)

    def time_limit(self):
        return self.real.time_limit()

    def wait(self):
        out = self.real.wait()
        return self.patch(out) if self.patch else out


class ProxyEngine:
    def __init__(self, hold=1, patch=None):
        self.hold, self.patch, self.jobs = hold, patch, []

    def default_options(self):
        return topor_engine.default_options()

    def route_async(self, m, o):
        j = ProxyJob(topor_engine.route_async(m, o), self.hold, self.patch, end_real=_dialog_without_cap(o))
        self.jobs.append(j)
        return j


def _dialog_without_cap(o):
    """The run's engine options say: progress dialog (no stop at stable) with a restart mode that has no cycle cap in
    the dialog (TW3 / D6: hybrid, periodic)."""
    d = json.loads(o) if isinstance(o, str) else dict(o)
    return d.get("stop_when_stable", True) is False and d.get("restart", "hybrid") not in ("stall", "off")


class ExplodingEngine:
    def default_options(self):
        return topor_engine.default_options()

    def route_async(self, m, o):
        raise AssertionError("the engine must not be called")


class PanicJob:
    """Behaves like the real RouteJob after a panic of its thread (gate review B-1): done() is True, wait()
    raises RuntimeError carrying the panic message (see tests/plugin/test_engine_job.py for the real one)."""
    n = 0

    def progress(self):
        self.n += 1
        return json.dumps({"phase": "route", "iteration": 1, "routed": 0, "total": 27, "vias": 0, "elapsed_s": 0.1})

    def done(self):
        return True  # the thread ended (panicked); wait() raises

    def cancel(self):
        pass

    def wait(self):
        raise RuntimeError("routing thread panicked: fake panic in the test double")


class PanicEngine(ExplodingEngine):
    def route_async(self, m, o):
        return PanicJob()


class RealPanicEngine(ExplodingEngine):
    """The REAL RouteJob whose routing thread panics (engine test hook ``_route_async_panicking``)."""

    def route_async(self, m, o):
        return topor_engine._route_async_panicking("index out of bounds in the test hook")


def _mark_partial(out_json):
    d = json.loads(out_json)
    d["partial"] = 1
    for it in d["items"][:3]:
        it["group"] = "topor-partial"
    return json.dumps(d)


def _nothing_routed(out_json):
    d = json.loads(out_json)
    d.update(items=[], routed=0, stop_reason="cancel", timed_out=True, connections=[], vias=0)
    return json.dumps(d)


# ------------------------------------------------------------------ shared full flows
@pytest.fixture(scope="module")
def accepted(app, eprom, tmp_path_factory, reports):
    """Accept flow with both KiCad follow-up actions switched on."""
    cfg = CFG.with_values({("run", "kicad_cleanup"): True, ("run", "kicad_teardrops"): True})
    tmp = tmp_path_factory.mktemp("acc")
    board = make_board(eprom)
    ui = ScriptedUi(accept=True)
    out = run_mod.run(board, ui, cfg, topor_engine, tmp_root=tmp / "work")
    _ok(out)
    return board, ui, out, tmp


@pytest.fixture(scope="module")
def rejected(app, eprom, tmp_path_factory, reports):
    tmp = tmp_path_factory.mktemp("rej")
    board = make_board(eprom)
    ui = ScriptedUi(accept=False)
    eng = ProxyEngine(hold=2)
    out = run_mod.run(board, ui, CFG, eng, tmp_root=tmp / "work")
    _ok(out)
    return board, ui, out, eng


def _result(board, out, tmp, name):
    """DRC of the committed board (FakeBoard's second save_copy) judged by kicad-cli."""
    copy = board.save_copy(tmp / name)
    rep = run_drc(copy, tmp / f"{name}.json")
    return copy, rep


# ------------------------------------------------------------------ tests
def test_run_uses_save_as_copy_not_disk_file(rejected, eprom):
    board, ui, out, _ = rejected
    assert [c[0] for c in board.calls].count("save_copy") == 1
    rm = next(c for c in board.calls if c[0] == "read_model")
    assert Path(rm[1][1]) == out.pcb_copy and out.pcb_copy.is_relative_to(out.work_dir)
    assert MARKER not in eprom.pcb.read_text()
    temp = [out.pcb_copy, out.ripped_pcb, out.routed_pcb]
    assert len({p for p in temp}) == 3
    for p in temp:
        assert p.is_file() and MARKER in p.read_text(), p  # every temp board derives from the live state


def test_run_accept_single_commit(accepted, tmp_path):
    board, ui, out, tmp = accepted
    assert out.status == "accepted"
    assert len(board.commits) == 1
    req = board.commits[0]
    model_kept = {k["uuid"] for k in out.model["kept"]} | {v["uuid"] for v in out.model["vias"]}
    assert req.remove_uuids and set(req.remove_uuids) == set(out.ripped) <= model_kept
    assert req.items == out.items and len(req.items) > 27
    assert req.message == f"Topor: routed {out.routed}/{out.total}"
    assert req.group is True and {it["group"] for it in req.items} == {"topor"}
    assert board.refills == 0  # ruling C28: run.refill_after_accept defaults to false -> Accept = one undo step
    assert out.report.facts["zones_refilled"] is False
    # run.commit_fills (default): the zones get the fill of the refilled routed copy in the same commit
    from toporlib import pcbfile
    zones = pcbfile.parse_zone_fills(out.routed_pcb.read_text())
    assert req.zone_fills and set(req.zone_fills) <= set(zones) and out.report.facts["zone_fills_committed"] >= 1
    assert pcbfile.parse_zone_fills(board.text) == {**pcbfile.parse_zone_fills(board.text), **req.zone_fills}
    assert "same undo step" in (out.report_dir / "report.md").read_text()
    assert board.actions == ["pcbnew.GlobalEdit.cleanupTracksAndVias", "pcbnew.GlobalEdit.editTeardrops"]
    assert "preview" in ui.names() and _last(ui) == "preview"  # no finished window after Accept (2026-11-19)
    # the second save_copy holds the commit; kicad-cli judges it (M4b gate numbers)
    copy, rep = _result(board, out, tmp, "after")
    spans = pcbfile.find_items(copy.read_text())
    res_ids = {s.uuid for s in spans}
    assert not (set(req.remove_uuids) & res_ids)
    assert len([u for u in res_ids if u in set(board.groups["topor"])]) == len(req.items)
    c = classify(rep, out.base_drc, set(board.groups["topor"]), unconnected_baseline=out.orig_drc)
    print(f"GATE eprom: routed {out.routed}/{out.total} routing_new {c['routing_new']} "
          f"unconnected_new {c['incomplete']['unconnected_new']} (all {c['incomplete']['unconnected_items']})")
    assert (out.routed, out.total) == (27, 27)
    assert c["routing_new"] == 0 and c["incomplete"]["unconnected_new"] == 0
    assert out.drc["routing_new"] == 0 and out.drc["incomplete"]["unconnected_new"] == 0
    # the normal geometry has arcs (so the segments test below means something)
    assert any(it.get("prim", {}).get("kind") == "arc" for it in req.items)


def _with_replaced(out_json):
    d = json.loads(out_json)
    d["remove_uuids"] = ["00000000-0000-4000-8000-00000000beef"]
    return json.dumps(d)


def test_run_commit_removes_the_hand_tracks_the_engine_replaced(app, eprom, tmp_path, reports):
    """exist-term r2: the engine's "remove_uuids" (kept tracks merged into a new collinear segment) are removed
    in the one commit, after the ripped items, and named in the outcome."""
    board, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=True), engine=ProxyEngine(patch=_with_replaced))
    assert out.status == "accepted" and len(board.commits) == 1
    req = board.commits[0]
    assert out.replaced == ["00000000-0000-4000-8000-00000000beef"]
    assert req.remove_uuids == list(out.ripped) + out.replaced


def test_run_reject_leaves_board_untouched(rejected, eprom):
    board, ui, out, _ = rejected
    assert out.status == "rejected" and out.commit is None
    assert board.commits == [] and board.refills == 0 and board.actions == []
    assert board.text.count(MARKER) == 1 and eprom.text == eprom.pcb.read_text()
    assert not any(c[0] in ("commit", "refill_zones", "run_action") for c in board.calls)
    assert _last(ui) == "preview" and "error" not in ui.names()  # no finished window after Reject (user 2026-11-19)
    assert "rejected" in (out.report_dir / "report.md").read_text()


def test_run_cancel_keeps_best_result_and_previews(app, eprom, tmp_path, reports):
    eng = ProxyEngine(hold=2)
    ui = ScriptedUi(accept=False, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng)
    assert eng.jobs[0].cancelled  # progress() returned False -> job.cancel()
    assert out.status == "rejected" and out.routed > 0  # preview was shown after the cancelled route
    assert "preview" in ui.names() and board.commits == []
    assert out.report.stop_reason in ("cancel", "stable")  # the job had finished by itself in this double
    assert out.cancelled is True


class _SketchJob(ProxyJob):
    """ProxyJob that records whether the plugin switched the engine's sketches on (last-preview); asks and fetches are no-ops."""
    enabled = False

    def enable_sketches(self):
        self.enabled = True

    def request_sketch(self):
        pass

    def sketch(self, kind):
        return None


class _SketchEngine(ProxyEngine):
    def route_async(self, m, o):
        j = _SketchJob(topor_engine.route_async(m, o), self.hold, self.patch, end_real=_dialog_without_cap(o))
        self.jobs.append(j)
        return j


class _ImageUi(ScriptedUi):
    """A progress dialog with a board image (or, `kind` None, with "off")."""

    def __init__(self, kind, **kw):
        super().__init__(**kw)
        self.kind = kind

    def preview_wanted(self):
        return self.kind

    def sketch(self, kind, sk):
        pass


@pytest.mark.parametrize("kind", [None, "last"])
def test_run_builds_sketches_only_when_the_dialog_shows_an_image(app, eprom, tmp_path, reports, kind):
    """verify-last-preview F3: progress_preview = off (preview_wanted() None) never switches the engine's sketches on
    (the router then builds none: no cost); an image kind does."""
    eng = _SketchEngine(hold=2)
    ui = _ImageUi(kind, accept=False, cancel_at_iteration=1)
    do_run(eprom, tmp_path, ui=ui, engine=eng)
    assert eng.jobs[0].enabled is (kind is not None)


class _LateJob(ProxyJob):
    """The engine job as seen by a Cancel: `done()` stays False until the plugin cancelled it, then it is done with a
    complete, valid result (a late result). ``waited`` counts the ``wait()`` calls: a cancelled run never reads it."""

    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.waited = 0

    def done(self):
        return self.cancelled and self.real.done()

    def wait(self):
        self.waited += 1
        return super().wait()


class _LateEngine(ProxyEngine):
    def route_async(self, m, o):
        j = _LateJob(topor_engine.route_async(m, o), self.hold, self.patch)
        self.jobs.append(j)
        return j


def _drain(engine):
    """Let a cancelled test job end (the real engine thread keeps computing after a Cancel): no leaked workers."""
    for j in engine.jobs:
        j.real.cancel(False)
        j.real.wait()


def _untouched(board, before_text, ui):
    """Cancel's promise: no tracks, vias or zone changes, no commit, no refill, no result window, no error."""
    assert board.commits == [] and board.refills == 0 and board.actions == []
    assert board.text == before_text
    assert not any(c[0] in ("commit", "refill_zones", "run_action", "preview", "error") for c in board.calls)
    assert not any(n in ui.names() for n in ("preview", "error"))


def test_run_cancel_while_the_engine_runs_takes_nothing_and_does_not_wait(app, eprom, tmp_path, reports):
    """User 2026-11-22: Cancel stops at once and takes no result. The engine job is told to stop (``cancel(ok=False)``)
    and abandoned: ``wait()`` is never called, even when the job finishes with a valid result right after."""
    eng = _LateEngine(hold=1)
    board = make_board(eprom)
    before = board.text
    ui = ScriptedUi(accept=True, discard_at_iteration=1)
    try:
        board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng, board=board)
        job = eng.jobs[0]
        assert job.cancelled and job.ok is False  # the engine is signalled to stop
        assert job.waited == 0  # ... and not waited for
        assert out.status == "cancelled" and out.cancelled is True and out.items == [] and out.commit is None
        _untouched(board, before, ui)
        assert "cancelled" in (out.report_dir / "report.md").read_text()
        assert "cancelled by the user" in (out.report_dir / "log.txt").read_text()
    finally:
        _drain(eng)


def test_run_late_result_after_cancel_is_never_applied(app, eprom, tmp_path, reports):
    """The engine already finished with a full result when Cancel is pressed (here at the DRC of that result): the
    result is discarded, nothing is previewed or written, even though the user's Accept answer would say yes."""
    eng = ProxyEngine(hold=1)
    board = make_board(eprom)
    before = board.text
    ui = ScriptedUi(accept=True, discard_when=lambda p: p.phase == "drc")
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng, board=board)
    assert eng.jobs[0].real.done()  # the result existed
    assert out.status == "cancelled" and out.items == [] and out.commit is None
    _untouched(board, before, ui)


def test_run_accept_still_takes_the_best_so_far(app, eprom, tmp_path, reports):
    """Accept (the stop that keeps the result) is unchanged: the run goes on to the result and the commit."""
    ui = ScriptedUi(accept=True, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=ProxyEngine(hold=2))
    assert ui.stop_discards() is False
    assert out.status == "accepted" and out.routed > 0 and board.commits and "preview" in ui.names()


def test_run_headless_ui_without_stop_discards_never_discards(app, eprom, tmp_path, reports):
    """A UI without ``stop_discards`` (the headless / CLI path) is judged as before: a stop keeps the best."""

    class Plain:  # no stop_discards, no waits_for_ok
        def __init__(self):
            self.inner = ScriptedUi(accept=False, cancel_at_iteration=1)

        def __getattr__(self, name):
            if name == "stop_discards":
                raise AttributeError(name)
            return getattr(self.inner, name)

    ui = Plain()
    board, _, out = do_run(eprom, tmp_path, ui=ui, engine=ProxyEngine(hold=2))
    assert out.status == "rejected" and out.routed > 0


def test_run_cancel_with_nothing_routed_is_cancelled(app, eprom, tmp_path, reports):
    eng = ProxyEngine(hold=2, patch=_nothing_routed)
    ui = ScriptedUi(accept=True, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng)
    assert out.status == "cancelled" and board.commits == []
    assert "preview" not in ui.names()
    assert "error" not in ui.names()  # Cancel before any copper: silent (the user pressed it)


def _no_copper_complete(out_json):
    d = json.loads(out_json)
    d.update(items=[], routed=0, stop_reason="stable", timed_out=False, connections=[], vias=0)
    return json.dumps(d)


def test_run_router_produced_no_copper_is_a_loud_error(app, eprom, tmp_path, reports):
    """N > 0 connections and no copper (not cancelled) = whole-run failure: one NOTHING_ROUTED error dialog, nothing
    committed, outcome error; never a quiet notice (fail-fast rule; lead 2026-11-19)."""
    eng = ProxyEngine(hold=2, patch=_no_copper_complete)
    board, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=True), engine=eng, strict=False)
    assert out.total > 0 and out.routed == 0 and not out.cancelled
    assert out.status == "error" and out.error is not None and out.error.code == "NOTHING_ROUTED"
    assert board.commits == [] and "preview" not in ui.names()
    assert [c[1][0].code for c in ui.calls if c[0] == "error"] == ["NOTHING_ROUTED"] and _last(ui) == "error"
    res = json.loads((out.report_dir / "result.json").read_text())
    assert res["status"] == "error" and "NOTHING_ROUTED" in [f["code"] for f in res["findings"]]


def test_run_nothing_to_route_is_silent(app, eprom, tmp_path, reports):
    """Zero connections: the pre-flight SCOPE_EMPTY warning covered it; no error and no window afterwards."""
    def zero(out_json):
        d = json.loads(_no_copper_complete(out_json))
        d["connections_total"] = 0
        return json.dumps(d)

    board, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=True), engine=ProxyEngine(hold=2, patch=zero), strict=False)
    assert out.total == 0 and out.status == "rejected" and board.commits == []
    assert "error" not in ui.names() and "preview" not in ui.names()


@pytest.mark.parametrize("commit,refill", [(False, False), (False, True), (True, False), (True, True)])
def test_run_preview_summary_carries_the_press_b_hint_only_when_both_are_off(app, eprom, tmp_path, reports, commit, refill):
    """Neither run.commit_fills nor run.refill_after_accept: the preview summary says press B (the hint the removed
    finished window carried); any other combination: no such line."""
    seen = []

    class Ui(ScriptedUi):
        def preview(self, png, report):
            seen.append(list(report.summary_lines()))
            return super().preview(png, report)

    cfg = CFG.with_values({("run", "commit_fills"): commit, ("run", "refill_after_accept"): refill})
    do_run(eprom, tmp_path, ui=Ui(accept=False), cfg=cfg)
    assert len(seen) == 1
    hint = [ln for ln in seen[0] if "press B" in ln]
    assert (len(hint) == 1 and hint[0].startswith("After Accept")) == (not commit and not refill), (seen[0], hint)
    assert len(hint) == int(not commit and not refill)


def test_run_aborts_on_error_finding(app, eprom, tmp_path, reports):
    live = eprom.text.replace('(2 "B.Cu" signal)', '(2 "B.Cu" signal)\n\t\t(4 "In1.Cu" signal)', 1)
    assert live != eprom.text
    board = FakeBoard(eprom.pcb, eprom.model, live_text=live, project_dir=eprom.pcb.parent)
    board, ui, out = do_run(eprom, tmp_path, board=board, engine=ExplodingEngine())
    assert out.status == "aborted_preflight"
    assert any(f.code == "LAYER_ROLES_NEEDED" and f.severity == "error" for f in out.findings)
    pf = [c for c in ui.calls if c[0] == "preflight"]
    assert len(pf) == 1 and any(f.code == "LAYER_ROLES_NEEDED" for f in pf[0][1][0])
    # MINOR 10: the only progress call before the pre-flight is the early "setup" one (no DRC, no routing)
    assert board.commits == [] and [c[1][0].phase for c in ui.calls if c[0] == "progress"] == ["setup"]


def test_run_declined_preflight_aborts(app, eprom, tmp_path, reports):
    from topor.ports import Decision

    ui = ScriptedUi(decisions=Decision(proceed=False))
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=ExplodingEngine())
    assert out.status == "aborted_preflight" and board.commits == []


def test_run_segments_geometry_has_no_arcs(app, eprom, tmp_path, reports):
    cfg = CFG.with_values({("output", "geometry"): "segments"})
    board, ui, out = do_run(eprom, tmp_path, cfg=cfg)
    assert out.status == "accepted" and out.routed == out.total == 27
    assert out.items and all(it.get("prim", {}).get("kind") != "arc" for it in out.items)
    assert board.commits[0].items == out.items
    new = {s.kind for s in pcbfile.find_items(pcbfile.append_items(
        "(kicad_pcb\n)\n", out.items, layer_names={0: "F.Cu", 2: "B.Cu"},
        net_names={n["code"]: n["name"] for n in out.model["nets"]}))}
    assert "arc" not in new
    assert out.drc["routing_new"] == 0


def test_run_partial_routes_group(app, eprom, tmp_path, reports):
    cfg = CFG.with_values({("run", "kicad_cleanup"): True})
    board, ui, out = do_run(eprom, tmp_path, cfg=cfg, engine=ProxyEngine(hold=1, patch=_mark_partial))
    assert out.status == "accepted"
    req = board.commits[0]
    groups = [it["group"] for it in req.items]
    assert groups.count("topor-partial") == 3 and set(groups) == {"topor", "topor-partial"}
    assert board.actions == []  # cleanup skipped when partial routes exist (30 section 4.2)
    assert any(f.code == "CLEANUP_SKIPPED_PARTIAL" for f in out.findings)
    assert set(board.groups) == {"topor", "topor-partial"}


@pytest.mark.parametrize("engine_cls", [PanicEngine, RealPanicEngine])
def test_run_engine_panic_reported_not_crashed(app, eprom, tmp_path, reports, engine_cls):
    """Gate review B-1: a panic of the routing thread ends the run with INTERNAL (never a hang); the REAL
    RouteJob is driven through a panic with the engine's test hook."""
    from topor import messages

    t0 = time.monotonic()
    board, ui, out = do_run(eprom, tmp_path, engine=engine_cls(), strict=False)
    assert time.monotonic() - t0 < 120
    assert out.status == "error" and board.commits == []
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert len(errs) == 1 and errs[0].code == "INTERNAL" and errs[0].severity == "error"
    assert "routing thread panicked" in errs[0].message
    if engine_cls is RealPanicEngine:
        assert "index out of bounds in the test hook" in errs[0].message
    assert errs[0].fix == messages.fix_for("INTERNAL")  # MINOR 6: the catalogue text
    assert "log.txt" in errs[0].fix and "debug.md" in errs[0].fix
    log = (out.report_dir / "log.txt").read_text()
    assert "panicked" in log and "Traceback" in log
    assert "preview" not in ui.names()
    ctx = [c for c in ui.calls if c[0] == "set_context"]
    assert ctx and ctx[-1][1][1] == out.report_dir  # error dialog can open the report folder


class SlowStopClock:
    """Stand-in for ``run.time``: monotonic() runs ahead by what the slow job adds (no real 45 s wait)."""

    def __init__(self):
        self.ahead = 0.0
        self.sleep, self.process_time = time.sleep, time.process_time

    def monotonic(self):
        return time.monotonic() + self.ahead


class SlowStopJob(ProxyJob):
    """A real RouteJob that honours Cancel only `after_s` (plugin clock) after it: an engine finishing a long
    stage on a big board. Every poll after Cancel moves the plugin's clock 2 s ahead."""

    def __init__(self, real, clock, after_s):
        super().__init__(real, hold=2)
        self.clock, self.after_s, self.cancelled_at = clock, after_s, None

    def cancel(self, ok=None):
        super().cancel(ok)
        self.cancelled_at = self.clock.monotonic()

    def done(self):
        if self.cancelled_at is None:
            return False
        self.clock.ahead += 2.0
        return self.clock.monotonic() - self.cancelled_at >= self.after_s and self.real.done()


class SlowStopEngine(ProxyEngine):
    def __init__(self, clock, after_s):
        super().__init__()
        self.clock, self.after_s = clock, after_s

    def route_async(self, m, o):
        j = SlowStopJob(topor_engine.route_async(m, o), self.clock, self.after_s)
        self.jobs.append(j)
        return j


def test_run_waits_for_the_engine_after_cancel_however_long_it_takes(app, eprom, tmp_path, reports, monkeypatch):
    """User report: Cancel waited 30 s and then produced nothing. After Cancel the flow waits until the engine
    has finished its current stage (here 45 s on the plugin's clock) and previews the result it returns."""
    clock = SlowStopClock()
    monkeypatch.setattr(run_mod, "time", clock)
    eng = SlowStopEngine(clock, after_s=45.0)
    ui = ScriptedUi(accept=False, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng)
    job = eng.jobs[0]
    assert job.cancelled and clock.monotonic() - job.cancelled_at >= 45.0
    assert not [c for c in ui.calls if c[0] == "error"]
    assert out.status == "rejected" and out.routed > 0 and out.cancelled is True
    assert "preview" in ui.names() and board.commits == []
    log = (out.report_dir / "log.txt").read_text()
    assert "waiting for the engine to finish its current stage" in log and "engine stopped" in log


class FailingBoard(FakeBoard):
    """FakeBoard whose post-commit steps fail on request ("refill", "action")."""

    def __init__(self, *a, fail=(), **kw):
        super().__init__(*a, **kw)
        self.fail = set(fail)

    def refill_zones(self):
        super().refill_zones()
        if "refill" in self.fail:
            raise RuntimeError("AS_BUSY: zone refill did not finish")

    def run_action(self, action):
        super().run_action(action)
        if "action" in self.fail:
            raise RuntimeError(f"unknown action {action}")


@pytest.mark.parametrize("step", ["refill", "action", "report"])
def test_run_failure_after_commit_stays_accepted(app, eprom, tmp_path, reports, monkeypatch, step):
    """Ruling C30 (gate review I-1): a failure after the successful commit (refill, KiCad action, report write,
    the error dialog) keeps status "accepted" and adds the POST_COMMIT_STEP_FAILED warning; never INTERNAL /
    "board not changed"."""
    from topor import app as app_mod

    cfg = CFG
    if step == "refill":
        cfg = CFG.with_values({("run", "refill_after_accept"): True})
    if step == "action":  # refill on (and succeeding), then the cleanup action fails
        cfg = CFG.with_values({("run", "refill_after_accept"): True, ("run", "kicad_cleanup"): True})
    if step == "report":
        real_write = run_mod.RunReport.write

        def write(self, *a, **kw):
            if (kw.get("result") or {}).get("status") == "accepted":
                raise OSError(28, "No space left on device")
            return real_write(self, *a, **kw)

        monkeypatch.setattr(run_mod.RunReport, "write", write)

    Ui = ScriptedUi

    text = eprom.text.rstrip()
    board = FailingBoard(eprom.pcb, eprom.model, live_text=text[:-1].rstrip("\n") + "\n" + LIVE_EDIT + ")\n",
                         project_dir=eprom.pcb.parent, fail=(step,))
    ui = Ui(accept=True)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, board=board, strict=False)
    assert out.status == "accepted" and app_mod._exit_code(out) == 0
    assert len(board.commits) == 1 and out.commit is not None
    pc = [f for f in out.findings if f.code == "POST_COMMIT_STEP_FAILED"]
    assert len(pc) == 1 and pc[0].severity == "warning", out.findings
    assert "committed" in pc[0].message and "Ctrl+Z" in pc[0].fix and "Edit → Undo" in pc[0].fix
    assert not any(f.code == "INTERNAL" for f in out.findings) and out.error is None
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    # the failed step is loud (one error dialog, the routes are on the board); no finished window
    assert [e.code for e in errs] == ["POST_COMMIT_STEP_FAILED"] and _last(ui) == "error"
    if step in ("refill", "action"):
        res = json.loads((out.report_dir / "result.json").read_text())
        assert res["status"] == "accepted" and "POST_COMMIT_STEP_FAILED" in (out.report_dir / "report.md").read_text()
    if step == "refill":
        assert board.refills == 1 and out.report.facts["zones_refilled"] is False and "zone refill" in pc[0].message
    if step == "action":  # refill_after_accept = true: exactly one refill after the commit (ruling C28)
        assert board.refills == 1 and out.report.facts["zones_refilled"] is True
        assert board.actions == [run_mod.ACTION_CLEANUP] and run_mod.ACTION_CLEANUP in pc[0].message
        assert "zones refilled" in (out.report_dir / "report.md").read_text()


def test_run_default_refills_zones_after_accept(app, eprom, tmp_path, reports):
    """Default run.refill_after_accept = true (user 2026-11-17: pours came back stale): exactly one refill after the
    commit, the report says so; a topor.toml pinning false keeps Accept = one undo step."""
    board, ui, out = do_run(eprom, tmp_path / "on", ui=ScriptedUi(accept=True), cfg=CFG_DEFAULT_REFILL)
    assert out.status == "accepted" and board.refills == 1 and len(board.commits) == 1
    assert out.report.facts["zones_refilled"] is True
    assert "zones refilled by topor" in (out.report_dir / "report.md").read_text()
    assert not [f for f in out.findings if f.code in ("ZONE_FILLS_SKIPPED", "POST_COMMIT_STEP_FAILED")]
    board, ui, out = do_run(eprom, tmp_path / "off", ui=ScriptedUi(accept=True), cfg=CFG)  # pinned false
    assert out.status == "accepted" and board.refills == 0 and out.report.facts["zones_refilled"] is False


class _Log:
    def __init__(self):
        self.lines = []

    def line(self, t):
        self.lines.append(t)


def test_routed_fills_every_skip_path_is_named(accepted, tmp_path):
    """run.commit_fills: _routed_fills returns the fills, or why there are none ("off" = the user's own setting,
    "no_copy", "other_items"); a skip other than "off" is logged with its reason."""
    board, ui, out, tmp = accepted
    state = {"judged_items": list(out.items)}
    fills, skip = run_mod._routed_fills(CFG, out, state, _Log())
    assert fills and skip is None
    log = _Log()
    assert run_mod._routed_fills(CFG.with_values({("run", "commit_fills"): False}), out, state, log) == ({}, "off")
    assert log.lines == []  # the user's own choice is not a failure
    log = _Log()
    out2 = replace(out, routed_pcb=tmp_path / "gone.kicad_pcb")
    assert run_mod._routed_fills(CFG, out2, state, log) == ({}, "no_copy")
    assert "refilled copy of the routed result is missing" in log.lines[0]
    log = _Log()
    other = [{**out.items[0], "x_nm": 1}] + list(out.items[1:])
    assert run_mod._routed_fills(CFG, out, {"judged_items": other}, log) == ({}, "other_items")
    assert "judged for other items" in log.lines[0]


@pytest.mark.parametrize("why", ["no_copy", "other_items", "not_applied"])
def test_run_zone_fills_skip_is_shown_to_the_user(app, eprom, tmp_path, reports, monkeypatch, why):
    """Fail fast: fills that commit_fills could not write are a ZONE_FILLS_SKIPPED warning that is an error dialog,
    in the report's Outcome line and findings, not only the log; moot when the live refill redid the zones."""
    real = run_mod._routed_fills
    if why == "not_applied":  # a fill for a zone that is not on the board: the port reports it
        monkeypatch.setattr(run_mod, "_routed_fills",
                            lambda *a: ({"no-such-zone-uuid": [{"layer": "F.Cu", "pts": [[0, 0], [1, 0], [1, 1]]}]}, None))
    else:
        monkeypatch.setattr(run_mod, "_routed_fills", lambda *a: ({}, why))
    board, ui, out = do_run(eprom, tmp_path / "a", ui=ScriptedUi(accept=True), cfg=CFG)
    f = [x for x in out.findings if x.code == "ZONE_FILLS_SKIPPED"]
    assert len(f) == 1 and f[0].severity == "warning" and "Fill All Zones" in f[0].fix, out.findings
    assert out.status == "accepted" and out.report.facts["post_commit_failed"] == f
    # fail fast (no finished window any more): exactly one error dialog, ZONE_FILLS_SKIPPED, and nothing after it
    assert [c[1][0].code for c in ui.calls if c[0] == "error"] == ["ZONE_FILLS_SKIPPED"]
    assert _last(ui) == "error"
    assert out.report.facts["zone_fills_skipped"] == why and out.report.facts["zone_fills_committed"] == 0
    md = (out.report_dir / "report.md").read_text()
    assert "ZONE_FILLS_SKIPPED" in md and "zone fills NOT written" in md and "press B" in md
    # with the live refill on the pours are redone anyway: nothing to warn about
    board, ui, out = do_run(eprom, tmp_path / "b", ui=ScriptedUi(accept=True), cfg=CFG_DEFAULT_REFILL)
    assert not [x for x in out.findings if x.code == "ZONE_FILLS_SKIPPED"] and board.refills == 1
    assert "error" not in ui.names()
    monkeypatch.setattr(run_mod, "_routed_fills", real)


def test_run_commit_fills_off_is_not_a_warning(app, eprom, tmp_path, reports):
    """The user's own run.commit_fills = false: the preview summary and the report say press B; no ZONE_FILLS_SKIPPED.
    A clean Accept ends at the preview: no error and no finished window (live test; the single-commit test is xfail)."""
    cfg = CFG.with_values({("run", "commit_fills"): False})
    board, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=True), cfg=cfg)
    assert out.status == "accepted" and _last(ui) == "preview" and "error" not in ui.names()
    assert not hasattr(ui, "done")  # the finished window is gone from the UI port
    assert out.report.facts["zone_fills_skipped"] == "off" and out.report.facts["zone_fills_committed"] == 0
    assert not [x for x in out.findings if x.code == "ZONE_FILLS_SKIPPED"]
    assert "zones not refilled: press B" in (out.report_dir / "report.md").read_text()


def test_run_report_dir_unwritable_is_an_error(eprom, tmp_path, monkeypatch):
    """Ruling C32 (gate review I-5): a report folder that cannot be created gives the REPORT_DIR_UNWRITABLE
    error through ui.error (a dialog in KiCad), the board is not read further and nothing is routed."""
    blocker = tmp_path / "not-a-dir"
    blocker.write_text("a file where the report root should be")
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(blocker / "reports"))
    board = make_board(eprom)
    ui = ScriptedUi()
    out = run_mod.run(board, ui, CFG, ExplodingEngine(), tmp_root=tmp_path / "work")
    assert out.status == "error" and out.error is not None and out.error.code == "REPORT_DIR_UNWRITABLE"
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert errs == [out.error] and errs[0].severity == "error"
    assert str(blocker) in errs[0].message and "output.report_dir" in errs[0].fix
    assert board.commits == [] and not any(c[0] == "save_copy" for c in board.calls)


def test_run_preview_png_written_with_caption(rejected):
    board, ui, out, _ = rejected
    png = out.report_dir / "preview.png"
    assert png.is_file() and png.stat().st_size > 0
    pv = [c for c in ui.calls if c[0] == "preview"]
    assert len(pv) == 1 and pv[0][1][0] == png and pv[0][1][1] is out.report
    assert isinstance(out.report, Report)
    lines = out.report.summary_lines()
    assert 0 < len(lines) <= 20
    assert lines[1].startswith("Routed 27/27 connections")
    img = wx.Image(str(png))
    assert img.IsOk() and img.GetWidth() > 400 and img.GetHeight() > 200
    # something is drawn (not a blank sheet) and the new-copper highlight tint is present
    px = img.GetData()
    colours = {bytes(px[i:i + 3]) for i in range(0, len(px), 3 * 7)}
    assert len(colours) > 8
    assert any(bytes(c) in colours for c in (preview.PREVIEW_HIGHLIGHT_RGB, preview.PREVIEW_HIGHLIGHT_FALLBACK_RGB))
    # the five report files, the outcome line rewritten after the decision
    d = out.report_dir
    for f in ("report.md", "preview.png", "result.json", "drc.json", "log.txt"):
        assert (d / f).is_file() and (d / f).stat().st_size > 0, f
    assert "Outcome: rejected" in (d / "report.md").read_text()
    res = json.loads((d / "result.json").read_text())
    assert res["status"] == "rejected" and res["routed"] == 27 and res["total"] == 27
    # context handed to the UI before the preview, and the phases shown
    names = ui.names()
    assert names.index("set_context") < names.index("preview")
    phases = [c[1][0].phase for c in ui.calls if c[0] == "progress"]
    for ph in ("preflight_drc", "drc", "preview"):
        assert ph in phases, phases
    assert phases.index("preflight_drc") < phases.index("drc") < phases.index("preview")
    assert "route" in phases or "done" in phases


def test_report_written_before_preview(app, eprom, tmp_path, reports):
    """Ruling C15/Q2: report.md and preview.png exist when preview() is called."""
    seen = {}

    class Ui(ScriptedUi):
        def preview(self, png, report):
            seen["files"] = sorted(p.name for p in Path(report.dir).iterdir())
            seen["outcome"] = (Path(report.dir) / "report.md").read_text()
            return super().preview(png, report)

    do_run(eprom, tmp_path, ui=Ui(accept=False), engine=ProxyEngine(hold=1, patch=None))
    assert "report.md" in seen["files"] and "preview.png" in seen["files"]


# ------------------------------------------------------------------ preview helpers (no board needed)
def test_arcs_to_segments_error_bound_and_ends():
    r = 3 * 1_000_000
    arc = {"net": 1, "layer": 0, "width": 200_000, "group": "topor", "wire": 4,
           "prim": {"kind": "arc", "start": [r, 0], "mid": [int(r * math.cos(0.7)), int(r * math.sin(0.7))],
                    "end": [int(r * math.cos(1.4)), int(r * math.sin(1.4))]}}
    seg = {"net": 1, "layer": 0, "width": 200_000, "prim": {"kind": "seg", "a": [0, 0], "b": [5, 5]}}
    via = {"kind": "via", "net": 1, "at": [1, 1], "d": 600_000, "drill": 300_000, "layers": [0, 2]}
    out = preview.arcs_to_segments([arc, seg, via], 5000)
    assert out[-2:] == [seg, via]
    segs = out[:-2]
    assert len(segs) > 3 and all(s["prim"]["kind"] == "seg" and s["group"] == "topor" and s["wire"] == 4 for s in segs)
    assert segs[0]["prim"]["a"] == arc["prim"]["start"] and segs[-1]["prim"]["b"] == arc["prim"]["end"]
    for a, b in zip(segs, segs[1:]):
        assert a["prim"]["b"] == b["prim"]["a"]
    for s in segs:  # chord midpoint within max_err of the circle
        a, b = s["prim"]["a"], s["prim"]["b"]
        mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
        assert abs(math.hypot(*mid) - r) <= 5000 + 2


def test_layer_colours_from_export():
    svg = ('<g style="fill:#000000; fill-opacity:1.0000;stroke:#000000; stroke-opacity:1.0000;\n">\n</g>\n'
           '<g style="fill:#840000; fill-opacity:1.0000; stroke:none;">\n<circle />\n</g>\n'
           '<g style="fill:#008400; fill-opacity:1.0000; stroke:none;">\n</g>\n'
           '<g style="fill:#FFFFFF; fill-opacity:1.0000; stroke:none;">\n</g>\n')
    assert preview.layer_colours(svg) == {0: (132, 0, 0), 2: (0, 132, 0)}
    assert preview.contrast((0, 0, 0), (255, 255, 255)) == pytest.approx(21.0)
    assert preview.pick_tint((255, 255, 255)) == preview.PREVIEW_HIGHLIGHT_RGB
    assert preview.pick_tint((53, 53, 53)) == preview.PREVIEW_HIGHLIGHT_FALLBACK_RGB


def test_accepted_findings_update_config(tmp_path):
    from topor.config import load_config
    from topor.findings import Finding
    from topor.ports import Decision

    cfg, _ = load_config(tmp_path)
    toml = tmp_path / "topor.toml"
    fs = [Finding("PAD_OUTSIDE_BOARD", "warning", "x", "y", ("U1.3", "J2.1"))]
    new = run_mod.apply_accepted_findings(Decision(True, accepted_codes=("PAD_OUTSIDE_BOARD",)), fs, cfg, toml)
    assert new.get("nets", "ignore_pads") == ["U1.3", "J2.1"]
    assert "U1.3" in toml.read_text()


# ------------------------------------------------------------------ ruling C20 / C21
def test_parse_zone_fills_and_version(eprom):
    z = pcbfile.parse_zone_fills(eprom.text)
    assert z and all(isinstance(k, str) for k in z)
    polys = [p for v in z.values() for p in v]
    assert polys and all(p["layer"].endswith(".Cu") and len(p["pts"]) >= 3 for p in polys)
    assert pcbfile.file_version(eprom.text) >= pcbfile.KICAD10_VERSION
    assert pcbfile.file_version("(kicad_pcb (version 20211014) (generator pcbnew)\n)") == 20211014
    assert pcbfile.file_version("garbage") is None


def test_run_refuses_pre_kicad10_file(app, eprom, tmp_path, reports):
    old = re.sub(r"\(version \d+\)", "(version 20211014)", eprom.text, count=1)
    board = FakeBoard(eprom.pcb, eprom.model, live_text=old, project_dir=eprom.pcb.parent)
    board, ui, out = do_run(eprom, tmp_path, board=board, engine=ExplodingEngine())
    assert out.status == "aborted_preflight" and board.commits == []
    f = [x for x in out.findings if x.code == "PCB_FORMAT_OLD"]
    assert f and f[0].severity == "error" and "KiCad 10" in f[0].fix


def test_run_wiccon_refill_gives_harness_connection_count(app, tmp_path_factory, reports):
    """Ruling C20: pour reach / dangling ends are recomputed on the refilled ripped copy (harness order
    rip -> refill -> export), so the engine sees the same connections as the harness model (62)."""
    from harness import writer
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths, rip_tracks
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("wic")
    cp = copy_project(board_pcb("wiccon-2023"), d / "w")
    b = load_board(cp)
    writer.save(b, cp.pcb)  # the snapshot is a KiCad 6 file; a live save_as copy is KiCad 10 format
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    model = export_model(b, rules, name="wiccon", dru_text=dru)
    board = FakeBoard(cp.pcb, model, project_dir=cp.pcb.parent)
    out = run_mod.run(board, ScriptedUi(accept=False), CFG, topor_engine, tmp_root=d / "work")
    _ok(out)
    rip_tracks(b, mode="all", scope=None, rf_nets=set())
    ref = json.loads(topor_engine.route(json.dumps(export_model(b, rules, name="wiccon", dru_text=dru)),
                                        json.dumps({"threads": 4, "wall_clock_s": 60.0})))
    print(f"wiccon connections: flow {out.total} harness {ref['connections_total']}")
    assert out.total == ref["connections_total"] == 62


def test_run_report_has_every_section(app, eprom, tmp_path, reports):
    """B8: the real flow writes a report.md with all nine sections and real numbers."""
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False))
    md = (out.report_dir / "report.md").read_text()
    for s in ("Summary", "Connections", "Vias", "Length", "DRC", "Findings", "Settings", "Files", "Versions"):
        assert f"\n## {s}\n" in md, s
    f = out.report.facts
    assert f["routed"] == 27 and f["vias_before"] is not None and f["length_before_mm"] is not None
    assert f["versions"]["engine"] and "non-default" in "\n".join(out.report.summary_lines())
    assert "Total " in md and "routing_new" in md and "Outside pads" in md


# ------------------------------------------------------------------ Q-b / ruling C27: VIA_NOT_SET_UP applies the fallback
class CaptureEngine(ProxyEngine):
    """Records the model / options the run hands to the engine; the route returns nothing (fast, no preview)."""

    def __init__(self):
        super().__init__(hold=1, patch=_nothing_routed)
        self.models, self.options = [], []

    def route_async(self, m, o):
        self.models.append(json.loads(m))
        self.options.append(json.loads(o))
        return super().route_async(m, o)


@pytest.fixture(scope="module")
def factory_eprom(tmp_path_factory):
    """eprom project whose .kicad_pro has only KiCad's factory via size (every net class 0.8 / 0.4, no
    pre-defined via sizes) -> VIA_NOT_SET_UP fires; the model is harness-built from that project (pcbnew,
    test side), so its rules carry what KiCad would resolve for it."""
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("factory")
    cp = copy_project(board_pcb(BOARD), d / BOARD)
    pro_p = cp.pcb.with_suffix(".kicad_pro")
    pro = json.loads(pro_p.read_text())
    for c in pro["net_settings"]["classes"]:
        c.update(via_diameter=0.8, via_drill=0.4)
    pro["board"]["design_settings"]["via_dimensions"] = [{"diameter": 0.0, "drill": 0.0}]  # KiCad's placeholder
    pro_p.write_text(json.dumps(pro))
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    e = Eprom()
    e.model = export_model(b, rules, name=BOARD, dru_text=dru)
    e.pcb = cp.pcb
    e.text = cp.pcb.read_text()
    return e


def _project(src, tmp_path, *, toml=None):
    """Copy of the project of `src` (an Eprom) into tmp_path, optionally with a topor.toml beside it."""
    d = tmp_path / "vproj"
    d.mkdir()
    for ext in (".kicad_pcb", ".kicad_pro", ".kicad_dru"):
        s = src.pcb.with_suffix(ext)
        if s.is_file():
            shutil.copyfile(s, d / s.name)
    if toml is not None:
        (d / "topor.toml").write_text(toml)
    return FakeBoard(d / src.pcb.name, src.model, project_dir=d)


def _via_sizes(rules):
    return ({(rules["default"]["via_d"], rules["default"]["via_drill"])}
            | {(r["via_d"], r["via_drill"]) for r in rules["per_net"].values()})


def test_via_not_set_up_applies_fallback_to_engine_rules(app, eprom, factory_eprom, tmp_path, reports):
    # a class without via fields resolves to KiCad's class default 0.6 / 0.3 (toporlib KICAD_CLASS_ABSENT_NM): the
    # "nothing set up" size every net carries here, which the fallback must replace
    src = Eprom()
    src.pcb, src.text = factory_eprom.pcb, factory_eprom.text
    r = factory_eprom.model["rules"]
    unset = {"via_d": 600_000, "via_drill": 300_000}
    src.model = {**factory_eprom.model, "rules": {**r, "default": {**r["default"], **unset},
                                                  "per_net": {k: {**v, **unset} for k, v in r["per_net"].items()}}}
    board = _project(src, tmp_path)
    eng = CaptureEngine()
    board, ui, out = do_run(eprom, tmp_path, board=board, engine=eng)
    assert any(f.code == "VIA_NOT_SET_UP" for f in out.findings)
    assert eng.models, "the engine was not called"
    rules = eng.models[0]["rules"]
    assert rules["per_net"] and _via_sizes(rules) == {(800_000, 400_000)}
    assert out.via_fallback["diameter_nm"] == 800_000 and out.via_fallback["drill_nm"] == 400_000
    res = json.loads((out.report_dir / "result.json").read_text())
    assert res["via_fallback"]["diameter_nm"] == 800_000 and res["via_fallback"]["drill_nm"] == 400_000
    md = (out.report_dir / "report.md").read_text()
    assert "via fallback 0.8 / 0.4 mm" in md and "VIA_NOT_SET_UP" in md


def test_via_fallback_from_topor_toml(app, factory_eprom, tmp_path, reports):
    toml = 'version = 1\n\n[vias]\nfallback_diameter_mm = 0.7\nfallback_drill_mm = 0.35\n'
    board = _project(factory_eprom, tmp_path, toml=toml)
    eng = CaptureEngine()
    out = run_mod.run(board, ScriptedUi(), None, eng, tmp_root=tmp_path / "work")
    _ok(out)
    assert any(f.code == "VIA_NOT_SET_UP" for f in out.findings)
    assert _via_sizes(factory_eprom.model["rules"]) == {(800_000, 400_000)}  # KiCad's factory size before
    rules = eng.models[0]["rules"]
    assert _via_sizes(rules) == {(700_000, 350_000)}
    assert "via fallback 0.7 / 0.35 mm" in (out.report_dir / "report.md").read_text()


def test_via_sizes_set_up_leaves_rules_unchanged(app, eprom, tmp_path, reports):
    board = _project(eprom, tmp_path)  # eprom's own .kicad_pro: Signal 0.6 / 0.3 -> via sizes are set up
    # the net classes KiCad reports for this project (FakeBoard's default is a Default class without values, which
    # the board-authority resolver rightly reports as rule fallbacks; KiCad always gives the Default values)
    from toporlib.rules import classes_from_pro, netclass_of_from_pro

    pro = json.loads(board.pcb.with_suffix(".kicad_pro").read_text())
    board.netclass_value = (classes_from_pro(pro), netclass_of_from_pro(pro, [n["name"] for n in eprom.model["nets"]]))
    eng = CaptureEngine()
    board, ui, out = do_run(eprom, tmp_path, board=board, engine=eng)
    assert not any(f.code == "VIA_NOT_SET_UP" for f in out.findings)
    assert eng.models[0]["rules"] == eprom.model["rules"]
    assert out.via_fallback is None
    assert json.loads((out.report_dir / "result.json").read_text())["via_fallback"] is None
    md = (out.report_dir / "report.md").read_text()
    assert "via fallback" not in md
    # board authority (user 2026-11-12): eprom's own minimums (track 0.15, hole clearance 0.25 mm) are used as given,
    # nothing is raised and the board sets every field (no rule fallback); the floors line belongs to the opt-in
    # [rules] fab_profile = "jlcpcb_1oz" only (before: "min_track_width 0.15 → 0.2 mm" was listed as raised)
    assert "Design rules: the board's KiCad configuration" in md and "Fab floors raised" not in md, md
    assert "Rule fallbacks (fields the board does not set): none" in md, md
    assert "min_track_width 0.15 → 0.2 mm" not in md


def test_apply_via_fallback_pure():
    cfg = Config.defaults().with_values({("vias", "fallback_diameter_mm"): 0.7, ("vias", "fallback_drill_mm"): 0.35})
    rules = {"min_via_diameter": 450_000, "min_via_drill": 250_000,
             "default": {"via_d": 600_000, "via_drill": 300_000, "width": 1},
             "per_net": {"1": {"via_d": 600_000, "via_drill": 300_000, "width": 2},
                         "2": {"via_d": 800_000, "via_drill": 400_000},   # KiCad factory size
                         "3": {"via_d": 500_000, "via_drill": 250_000}}}  # a size somebody set: kept
    new, info = run_mod.apply_via_fallback(rules, cfg)
    assert new["default"] == {"via_d": 700_000, "via_drill": 350_000, "width": 1}
    assert new["per_net"]["1"] == {"via_d": 700_000, "via_drill": 350_000, "width": 2}
    assert new["per_net"]["2"] == {"via_d": 700_000, "via_drill": 350_000}
    assert new["per_net"]["3"] == rules["per_net"]["3"]
    assert rules["default"]["via_d"] == 600_000  # input not mutated
    assert info["diameter_nm"] == 700_000 and info["drill_nm"] == 350_000 and info["per_net"] == 2
    assert not info["clamped"]
    # the board's minimums still apply
    lo = Config.defaults().with_values({("vias", "fallback_diameter_mm"): 0.3, ("vias", "fallback_drill_mm"): 0.1})
    new, info = run_mod.apply_via_fallback(rules, lo)
    assert (new["default"]["via_d"], new["default"]["via_drill"]) == (450_000, 250_000) and info["clamped"]


# ------------------------------------------------------------------ ruling C29 (gate review I-6)
def test_run_fixed_takes_pour_fills_from_the_refilled_copy(app, eprom, tmp_path, reports):
    """existing_copper = "fixed" (nothing ripped): the engine's pour fills / reach / dangling ends come from the
    kicad-cli REFILLED copy of the untouched board (the original DRC run, ``--refill-zones --save-board``), not
    from the live board's possibly stale fills. The FakeBoard model carries a deliberately wrong fill (one
    rectangle over the whole board) that "reaches" a pad and a via that do not exist."""
    from topor import pourreach

    gnd = next(p for p in eprom.model["pours"] if p.get("fills") and p.get("pads"))
    xs = [pd["center"][0] for pd in eprom.model["pads"]]
    ys = [pd["center"][1] for pd in eprom.model["pads"]]
    big = [[min(xs) - 10**7, min(ys) - 10**7], [max(xs) + 10**7, min(ys) - 10**7],
           [max(xs) + 10**7, max(ys) + 10**7], [min(xs) - 10**7, max(ys) + 10**7]]
    stale_fills = [{"layer": l, "outer": big, "holes": []} for l in gnd["layers"]]
    stale = {**gnd, "fills": stale_fills, "pads": ["FAKE.1"], "vias": ["fake-via"]}
    model = {**eprom.model, "pours": [stale if p is gnd else p for p in eprom.model["pours"]]}
    src = Eprom()
    src.pcb, src.text, src.model = eprom.pcb, eprom.text, model
    board = _project(src, tmp_path)
    eng = CaptureEngine()
    cfg = Config.defaults().with_values({("run", "timeout_s"): 60.0})
    assert cfg.get("run", "existing_copper") == "fixed"
    board, ui, out = do_run(eprom, tmp_path, board=board, cfg=cfg, engine=eng)
    assert out.ripped == [] and eng.models, "the engine was not called"
    assert out.refilled_pcb is not None and out.refilled_pcb.is_file() and out.refilled_pcb != out.pcb_copy
    seen = next(p for p in eng.models[0]["pours"] if p["uuid"] == gnd["uuid"])
    ids = {l["name"]: int(l["id"]) for l in model["layers"]}
    want = [{"layer": ids[q["layer"]], "outer": q["pts"], "holes": []}
            for q in pcbfile.parse_zone_fills(out.refilled_pcb.read_text())[gnd["uuid"]]
            if q["layer"] in ids and ids[q["layer"]] in gnd["layers"]]
    assert want and seen["fills"] == want and seen["fills"] != stale_fills
    assert "FAKE.1" not in seen["pads"] and "fake-via" not in seen["vias"]
    rp, rv = pourreach.pour_reach(gnd["net"], gnd["layers"], want, model["pads"], model["vias"],
                              gnd.get("pad_connection", 1))
    assert (seen["pads"], seen["vias"]) == (rp, rv) and rp
    assert set(rp) <= set(gnd["pads"])  # the refilled reach errs toward "not reached" vs pcbnew (C26)
    # the save_as copy itself stays as KiCad wrote it (the refill went into a separate file)
    assert out.pcb_copy.read_text() == board.text


def test_refill_pours_zone_unfilled_after_refill_reaches_nothing():
    """M4-R8: a zone that is in the refilled copy but has no fill there reaches nothing (its old fill stays as the
    obstacle: a superset of no copper); a zone absent from the copy keeps its model entry."""
    model = {"layers": [{"id": 0, "name": "F.Cu"}, {"id": 2, "name": "B.Cu"}],
             "pads": [{"ref": "R1", "number": "1", "net": 1, "layers": [0], "center": [0, 0], "rotation_deg": 0.0,
                       "shapes": {"0": {"kind": "rect", "w": 1_000_000, "h": 1_000_000}}}],
             "vias": [], "kept": [],
             "pours": [{"uuid": "z-empty", "net": 1, "layers": [0], "pads": ["R1.1"], "vias": [],
                        "fills": [{"layer": 0, "outer": [[-5, -5], [5, -5], [5, 5]], "holes": []}]},
                       {"uuid": "z-absent", "net": 1, "layers": [0], "pads": ["R1.1"], "vias": [],
                        "fills": [{"layer": 0, "outer": [[-5, -5], [5, -5], [5, 5]], "holes": []}]}]}
    text = '(kicad_pcb\n\t(zone\n\t\t(net "N")\n\t\t(layer "F.Cu")\n\t\t(uuid "z-empty")\n\t)\n)\n'
    out = run_mod.refill_pours(model, text)
    empty, absent = out["pours"]
    assert empty["pads"] == [] and empty["vias"] == [] and empty["fills"] == model["pours"][0]["fills"]
    assert "warning" in empty
    assert absent == model["pours"][1]


# ------------------------------------------------------------------ A9: priorities / do-not-touch / ignored pads / pour_served
class OptionsEngine(ProxyEngine):
    """Real engine; records the model / options JSON handed to route_async."""

    def __init__(self):
        super().__init__(hold=1)
        self.models, self.options = [], []

    def route_async(self, m, o):
        self.models.append(json.loads(m))
        self.options.append(json.loads(o))
        return super().route_async(m, o)


def _signal_net(model, skip=()):
    """(name, code) of a net with >= 2 pads and hand copper that is not a ground / power net."""
    pads, kept = {}, {}
    for p in model["pads"]:
        pads[p["net"]] = pads.get(p["net"], 0) + 1
    for k in model["kept"]:
        kept[k["net"]] = kept.get(k["net"], 0) + 1
    for n in model["nets"]:
        if n["code"] and pads.get(n["code"], 0) >= 2 and kept.get(n["code"]) and n["name"] not in skip \
                and not re.search(r"GND|VCC|\+5V|\+3V3|VDD", n["name"]):
            return n["name"], n["code"]
    pytest.skip("no signal net with hand copper")


def test_a9_options_reach_the_engine_and_excluded_net_stays(app, eprom, tmp_path, reports):
    name, code = _signal_net(eprom.model)
    pad = next(p for p in eprom.model["pads"] if p["net"] != code and p["net"])
    ref = f"{pad['ref']}.{pad['number']}"
    other = next(n["name"] for n in eprom.model["nets"] if n["code"] and n["name"] != name)
    toml = ('version = 1\n\n[run]\nexisting_copper = "rip"\n\n[nets]\n'
            f'do_not_touch = ["{name}"]\nignore_pads = ["{ref}"]\n'
            f'pour_served = {{ "{other}" = false, "nonsense" = "yes" }}\n'
            f'priority = {{ "{other}" = 1 }}\n')
    board = _project(eprom, tmp_path, toml=toml)
    eng = OptionsEngine()
    out = run_mod.run(board, ScriptedUi(), None, eng, tmp_root=tmp_path / "work")
    _ok(out)
    o = eng.options[0]
    assert o["exclude_nets"] == [name]
    assert o["ignore_pads"] == [ref]
    assert o["pour_served"] == {other: False}  # only real booleans reach the engine
    assert o["net_priority"][other] == 1 and all(1 <= v <= 3 for v in o["net_priority"].values())
    # the excluded net's hand copper is not ripped and the engine did not route it
    kept_uuids = {k["uuid"] for k in eprom.model["kept"] if k["net"] == code}
    assert kept_uuids and not (kept_uuids & set(out.ripped))
    assert not any(it.get("net") == code for it in out.items)
    md = (out.report_dir / "report.md").read_text()
    assert f"Excluded nets (do not touch): {name}" in md
    assert ref in md  # ignored pad listed in the report


def test_a9_build_options_defaults_are_left_out(eprom):
    cfg = Config.defaults()
    opts = run_mod.build_options(cfg, ProxyEngine(), model=eprom.model, priorities={})
    for k in ("exclude_nets", "ignore_pads", "net_priority", "pour_served"):
        assert k not in opts, k  # empty = the engine's own default
    opts = run_mod.build_options(cfg.with_values({("nets", "pour_served"): {"GND": True, "X": 1}}),
                                 ProxyEngine(), model=eprom.model, priorities={"A": 2})
    assert opts["pour_served"] == {"GND": True} and opts["net_priority"] == {"A": 2}


def test_ripup_notes_lists_totals_and_backstop_pairs():
    """A16: the run report names the rip-up totals and the pairs that hit the max_per_pair backstop."""
    from topor.run import _ripup_notes

    assert _ripup_notes({}) == []
    assert _ripup_notes({"ripup": {"total": 0, "rips_per_connection": [0, 0], "pairs_at_backstop": []}}) == []
    rp = {"total": 6, "max_pair": 3, "rips_per_connection": [3, 3, 0], "pairs_at_backstop": [[1, 0]]}
    (line,) = _ripup_notes({"ripup": rp})
    assert "6 rips" in line and "2 connections" in line and "max 3 per pair" in line and "1->0" in line


def test_via_relax_notes_only_when_the_pass_ran():
    """A13: the run report names the via-relaxation outcome unless it was off or never ran."""
    from topor.run import _via_relax_notes

    assert _via_relax_notes({}) == []
    assert _via_relax_notes({"via_relaxation": "off"}) == []
    assert _via_relax_notes({"via_relaxation": "not run"}) == []
    assert _via_relax_notes({"via_relaxation": "moved 8 of 30 vias, -2004 um"}) == [
        "via relaxation: moved 8 of 30 vias, -2004 um"]


# ------------------------------------------------------------------ M-B0: OK at "Routes stable"
def test_run_ok_at_routes_stable_is_not_a_cancel(app, eprom, tmp_path, reports):
    """With the dialog the engine does not stop at stable by itself (``stop_when_stable`` false); the user's OK
    once the progress says stable ends it: the result is "stable", not cancelled, and is previewed and accepted
    like a finished run."""
    ui = ScriptedUi(accept=True, ok_at_stable=True)
    # default finish_when_stable = False: the run waits for the OK poll; a cap far away (eprom's loop runs ~20 cycles
    # a second), so no run's loop can reach the cap before every run is stable and the OK comes
    cfg = CFG.with_values({("optimize", "rounds"): 1000})
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=_RealCancelEngine(), cfg=cfg)
    assert out.status == "accepted" and out.routed == 27
    assert out.stop_reason == "stable" and out.cancelled is False
    stable = [c[1][0] for c in ui.calls if c[0] == "progress" and c[1][0].stable]
    # every run stable (5 of 5, whatever the engine's thread count: a stable run frees its slot for a run not yet
    # begun) and a pick to keep: "Routes stable" waits for the first pick (user 2026-11-17), the loops' predicted one
    assert stable and stable[0].runs == 5 and stable[0].runs_stable == 5, stable[:1]
    assert stable[0].best_total == 27 and stable[0].best_routed == 27 and stable[0].best_round >= 1, stable[0]
    log = (out.report_dir / "log.txt").read_text()
    assert f"OK at Routes stable: best so far {stable[0].best_routed}/27 routed" in log
    assert "no pick drawn yet" not in log
    # "Report stable live" (user 2026-11-17): the OK reached the running engine, so the drawn run's TopoR loop (the
    # result carries its loop line only) ended by the OK, before the cycle cap, and the engine itself says "stable"
    rr = json.loads((out.report_dir / "result.json").read_text())
    assert rr["stop_reason"] == "stable"
    loops = [w for w in _engine_warnings(out) if w.startswith("TopoR loop (")]
    assert len(loops) == 1 and "ended by Cancel / OK" in loops[0], loops
    # ... the drawn run was stable, and the other 4 runs (every one stable when the OK came, runs_stable above) each
    # had a pick of their own, not drawn because only one pick is drawn after Cancel / OK
    best_of = next(w for w in _engine_warnings(out) if w.startswith("best of 5 seeded runs"))
    assert "stable after cycle" in best_of and "not stable" not in best_of, best_of
    assert best_of.count("its TopoR loop pick not drawn") == 4, best_of


def _engine_warnings(out):
    """The engine's own report lines of the run (result.json "route")."""
    return list(json.loads((out.report_dir / "result.json").read_text())["route"].get("warnings") or [])


def test_ok_after_the_engine_ended_at_the_cap_stays_round_cap(app, eprom, tmp_path, reports):
    """An OK that arrives after the engine had already ended at its cycle cap did not end the run: stop_reason stays
    "round cap" (verify-slfx F1: the plugin no longer relabels it "stable"), and it is not a cancel either. The proxy
    job records the OK and lets the real job run to its cap. TW3 / D6: only restart = "stall" (or "off") keeps the
    dialog's cycle cap; the default hybrid has none."""
    class OkOnceDrawn(ScriptedUi):  # OK at "Routes stable" (a pick to keep: the log names the best so far)
        def progress(self, p):
            super().progress(p)
            return not (p.stable and p.best_total)

    cfg = CFG.with_values({("optimize", "rounds"): 20, ("optimize", "restart"): "stall"})
    ui = OkOnceDrawn(accept=True, ok_at_stable=True)
    eng = ProxyEngine(hold=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng, cfg=cfg)
    assert eng.jobs[0].cancelled, "the OK was sent"
    assert out.stop_reason == "round cap" and out.cancelled is False, out.stop_reason
    assert out.status == "accepted" and out.routed == 27
    log = (out.report_dir / "log.txt").read_text()
    assert re.search(r"OK at Routes stable: best so far \d+/27 routed, [\d.]+ m, \d+ vias \(run \d of 5\)", log), log


class _RealCancelEngine:
    """The real engine; records the options the run passes (M-B0: stop_when_stable off with the dialog)."""

    def __init__(self):
        self.options = []

    def __getattr__(self, name):
        return getattr(topor_engine, name)

    def route_async(self, m, o):
        self.options.append(json.loads(o))
        assert self.options[-1]["stop_when_stable"] is False
        return topor_engine.route_async(m, o)


def test_drawing_style_override_reaches_the_drawing_section_and_toml(tmp_path):
    """The pre-flight trace-shape choice is a [drawing] style override (not a [run] key): the run's config and, with
    Save, topor.toml get `[drawing] style` (user 2026-11-07: "make it kicad accessible")."""
    from topor import run as run_mod
    from topor.config import Config, update_config
    over = {"drawing_style": "smooth", "timeout_s": 30.0}
    where = {k: run_mod._OVERRIDE_KEY.get(k, ("run", k)) for k in over}
    cfg = Config.defaults().with_values({where[k]: v for k, v in over.items()})
    assert cfg.get("drawing", "style") == "smooth" and cfg.get("run", "timeout_s") == 30.0
    assert "drawing_style" in run_mod._ROUTE_OVERRIDES
    toml = tmp_path / "topor.toml"
    toml.write_text("")
    update_config(toml, {f"{where[k][0]}.{where[k][1]}": v for k, v in over.items()})
    text = toml.read_text()
    assert "[drawing]" in text and 'style = "smooth"' in text


# ------------------------------------------------------------------ run lock (one run per project)
class _NestedStartUi(ScriptedUi):
    """On the first progress call a second run starts for the same project (a second click on the button)."""

    def __init__(self, second):
        super().__init__()
        self.second, self.second_out, self.second_ui = second, None, ScriptedUi()

    def progress(self, p):
        if self.second_out is None:
            self.second_out = self.second(self.second_ui)
        return super().progress(p)


def test_second_concurrent_run_aborts_at_preflight_first_finishes(eprom, tmp_path):
    board2 = make_board(eprom)
    ui = _NestedStartUi(lambda ui2: run_mod.run(board2, ui2, CFG, ExplodingEngine(), tmp_root=tmp_path / "work2"))
    board, ui, out = do_run(eprom, tmp_path, ui=ui, cfg=CFG.with_values({("run", "timeout_s"): 10.0}))
    assert out.status in ("accepted", "rejected")
    o2 = ui.second_out
    assert o2.status == "aborted_preflight" and o2.error.code == "RUN_ALREADY_RUNNING"
    assert "already running for this project" in o2.error.message and "process" in o2.error.message
    assert [c[1][0] for c in ui.second_ui.calls if c[0] == "error"] == [o2.error]
    assert "aborted at pre-flight" in (o2.report_dir / "log.txt").read_text()
    assert "aborted at pre-flight" in (o2.report_dir / "report.md").read_text()
    assert board2.commits == [] and not any(c[0] == "save_copy" for c in board2.calls)
    # the lock is free again after the first run: a third run is not blocked
    from topor import runlock
    from topor.config import cache_root
    runlock.acquire(eprom.pcb.parent, "eprom-to-kernal", cache_root()).release()


def test_single_instance_off_takes_no_lock(eprom, tmp_path):
    from topor import runlock
    from topor.config import cache_root
    held = runlock.acquire(eprom.pcb.parent, eprom.pcb.stem, cache_root())
    try:
        board = make_board(eprom)
        out = run_mod.run(board, ScriptedUi(), CFG.with_values({("run", "single_instance"): False}),
                          ExplodingEngine(), tmp_root=tmp_path / "work")
        assert out.error is None or out.error.code != "RUN_ALREADY_RUNNING"
    finally:
        held.release()


def test_run_lock_unwritable_warns_and_goes_on(eprom, tmp_path, monkeypatch):
    from topor import runlock

    def boom(*a, **k):
        raise runlock.RunLockUnavailable(tmp_path / "x.lock", PermissionError("read-only"))
    monkeypatch.setattr(runlock, "acquire", boom)
    out = run_mod.run(make_board(eprom), ScriptedUi(), CFG, ExplodingEngine(), tmp_root=tmp_path / "work")
    assert any(f.code == "RUN_LOCK_UNAVAILABLE" and f.severity == "warning" for f in out.findings)
    assert out.status != "aborted_preflight" or out.error is None


def _edge_zero_project(tmp_path, eprom, toml=None):
    """eprom with copper-to-edge 0 in its .kicad_pro and in the model's rules (as KiCad would resolve it)."""
    src = Eprom()
    src.pcb, src.text = eprom.pcb, eprom.text
    src.model = {**eprom.model, "rules": {**eprom.model["rules"], "edge_clearance": 0}}
    board = _project(src, tmp_path, toml=toml)
    pro_p = board.pcb.with_suffix(".kicad_pro")
    pro = json.loads(pro_p.read_text())
    pro["board"]["design_settings"]["rules"]["min_copper_edge_clearance"] = 0.0
    pro_p.write_text(json.dumps(pro))
    return board


@pytest.mark.parametrize("mode", ["zone_clearance", "board", "pours"])
def test_edge_zero_rule_c_sets_the_engine_edge_clearance(app, eprom, tmp_path, reports, mode):
    """Rule C (edge-clear): copper-to-edge 0 -> the engine gets [rules] edge_clearance_zero's value (default: the
    biggest pour clearance), the EDGE_CLEARANCE_ZERO finding names it and the report's Settings list it."""
    from toporlib import edge_zero as ez

    toml = None if mode == "zone_clearance" else f'version = 1\n\n[rules]\nedge_clearance_zero = "{mode}"\n'
    board = _edge_zero_project(tmp_path, eprom, toml)
    eng = CaptureEngine()
    out = run_mod.run(board, ScriptedUi(), None if toml else CFG, eng, tmp_root=tmp_path / "work")
    _ok(out)
    want = {"zone_clearance": ez.zone_clearance(eprom.model), "board": 0, "pours": ez.pour_setback(eprom.model)}[mode]
    if mode == "zone_clearance":
        assert want == max(p["clearance"] for p in eprom.model["pours"]) > 0
    assert eng.models[0]["rules"]["edge_clearance"] == want
    text = ez.text({"mode": mode, "value_nm": want, "board_nm": 0})
    (f,) = [f for f in out.findings if f.code == "EDGE_CLEARANCE_ZERO"]
    assert text in f.message and "Copper to edge clearance" in f.message, f.message
    md = (out.report_dir / "report.md").read_text()
    assert "Copper to edge clearance 0 on the board: " + text in md, md
    assert "warning: " in (out.report_dir / "log.txt").read_text()


def test_edge_non_zero_board_value_wins_in_the_run(app, eprom, tmp_path, reports):
    """eprom's own 0.25 mm copper-to-edge: rule C is not consulted, the engine gets 0.25, no finding, no line."""
    board = _project(eprom, tmp_path, toml='version = 1\n\n[rules]\nedge_clearance_zero = "pours"\n')
    eng = CaptureEngine()
    out = run_mod.run(board, ScriptedUi(), None, eng, tmp_root=tmp_path / "work")
    _ok(out)
    assert eng.models[0]["rules"]["edge_clearance"] == eprom.model["rules"]["edge_clearance"] == 250_000
    assert not any(f.code == "EDGE_CLEARANCE_ZERO" for f in out.findings)
    assert "Copper to edge clearance 0 on the board" not in (out.report_dir / "report.md").read_text()


class _NeverOkDialog(ScriptedUi):
    """Progress dialog the user never answers: counts the polls before and after "Routes stable" and never ends the run."""

    def __init__(self, **kw):
        super().__init__(**kw)
        self.waits_for_ok = True
        self.polls_after_stable = 0

    def progress(self, p):
        if getattr(p, "stable", False):
            self.polls_after_stable += 1
        return super().progress(p)


def test_dialog_run_ends_at_the_cycle_cap_not_at_stable(app, eprom, tmp_path, reports):
    """Stop-on-stable fix: with the dialog and the defaults the run goes on after "Routes stable" until the cycle
    cap, and says so: stop_reason "round cap", never "stable" (patience gap M5/M6 of verify-stable-toml). TW3 / D6:
    the cap is kept only with restart = "stall" (or "off"); the default hybrid has none."""
    cfg = CFG.with_values({("optimize", "rounds"): 20, ("optimize", "restart"): "stall"})
    ui = _NeverOkDialog(accept=True)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=_RealCancelEngine(), cfg=cfg)
    assert ui.polls_after_stable > 1, "the dialog saw Routes stable and the run went on"
    assert out.stop_reason == "round cap", out.stop_reason
    assert out.status == "accepted" and out.routed == 27 and out.cancelled is False


def _project_with_toml(eprom, tmp_path, toml_text):
    """A project folder of its own (the module's eprom copy stays untouched) whose topor.toml is `toml_text`; the
    run loads it (cfg None). Returns the board port."""
    proj = tmp_path / "proj"
    proj.mkdir()
    (proj / "topor.toml").write_text(toml_text)
    text = eprom.text.rstrip()
    live = text[:-1].rstrip("\n") + "\n" + LIVE_EDIT + ")\n"
    return FakeBoard(eprom.pcb, eprom.model, live_text=live, project_dir=proj)


_PINNED_FINISH = ("version = 1\n[run]\nexisting_copper = \"rip\"\ntimeout_s = 60.0\n[optimize]\nrounds = 1000\n"
                  "finish_when_stable = true\n")


class _OkAfterStable(ScriptedUi):
    """The progress dialog whose user presses OK only `after` polls past the first "Routes stable"."""

    def __init__(self, after, **kw):
        super().__init__(**kw)
        self.waits_for_ok = True
        self.after, self.polls_after_stable = after, 0

    def progress(self, p):
        super().progress(p)
        if getattr(p, "stable", False):
            self.polls_after_stable += 1
        return self.polls_after_stable <= self.after


def test_a_pinned_finish_when_stable_no_longer_ends_a_dialog_run(app, eprom, tmp_path, reports):
    """User 2026-11-17 (a real run in the plugin stopped at "Routes stable" after 119 s of 600 because topor.toml
    pinned finish_when_stable = true from 15 Nov): with the progress dialog nothing ends the run at stable. The key is
    deprecated: the pre-flight lists CFG_DEPRECATED_KEY with its line, the engine gets finish_when_stable false, and
    the run keeps iterating after "Routes stable" until the OK (the TopoR loop ended by the OK, not at stable)."""
    board = _project_with_toml(eprom, tmp_path, _PINNED_FINISH)
    eng = _RealCancelEngine()
    ui = _OkAfterStable(8, accept=True)
    out = run_mod.run(board, ui, None, eng, tmp_root=tmp_path / "work")
    _ok(out)
    pre = next(c[1][0] for c in ui.calls if c[0] == "preflight")
    dep = [f for f in pre if f.code == "CFG_DEPRECATED_KEY"]
    assert len(dep) == 1 and "topor.toml (line 7) sets optimize.finish_when_stable = true" in dep[0].message, pre
    assert eng.options[0]["finish_when_stable"] is False and eng.options[0]["finish_patience"] == 0
    assert ui.polls_after_stable > 8, "the run went on after Routes stable until the OK"
    assert out.stop_reason == "stable" and out.cancelled is False and out.routed == 27
    loops = [w for w in _engine_warnings(out) if w.startswith("TopoR loop (")]
    assert len(loops) == 1 and "ended by Cancel / OK" in loops[0], loops
    assert "finish_when_stable = true" in (board.project_dir() / "topor.toml").read_text()  # untouched without the tick


def test_use_the_current_default_comments_the_pinned_line_out(app, eprom, tmp_path, reports, monkeypatch):
    """User 2026-11-17 ("Show it in the dialog"): ticking Use the current default for a pinned line (a deprecated key,
    a pinned old default) comments the line out in topor.toml when Route is pressed, the run uses the current default
    and the report notes CFG_DEFAULT_RESTORED; an unticked pinned line stays as it is."""
    from topor import schema
    monkeypatch.setattr(schema, "PAST_DEFAULTS", {("run", "timeout_clock"): (("cpu", "2026-01-01"),),
                                                  ("run", "respect_locked"): ((not schema.SETTINGS_BY_KEY[("run", "respect_locked")].default, "2026-01-02"),)})
    old_locked = str(not schema.SETTINGS_BY_KEY[("run", "respect_locked")].default).lower()
    toml = _PINNED_FINISH.replace("timeout_s = 60.0\n", f"timeout_s = 60.0\ntimeout_clock = \"cpu\"\nrespect_locked = {old_locked}\n")
    board = _project_with_toml(eprom, tmp_path, toml)
    chosen = ("optimize.finish_when_stable", "run.timeout_clock")
    ui = ScriptedUi(decisions=Decision(True, use_defaults=chosen), accept=False)
    out = run_mod.run(board, ui, None, topor_engine, tmp_root=tmp_path / "work")
    _ok(out)
    pre = next(c[1][0] for c in ui.calls if c[0] == "preflight")
    assert {f.code for f in pre} >= {"CFG_DEPRECATED_KEY", "CFG_PINNED_OLD_DEFAULT"}
    got = (board.project_dir() / "topor.toml").read_text()
    assert "\n# finish_when_stable = true\n" in got and '\n# timeout_clock = "cpu"\n' in got
    assert f"\nrespect_locked = {old_locked}\n" in got, "an unticked pinned line is kept"
    assert out.time_limit["clock"] == "wall", "the run used the current default of run.timeout_clock"
    notes = [f for f in out.findings if f.code == "CFG_DEFAULT_RESTORED"]
    assert sorted(f.items[0] for f in notes) == sorted(chosen), notes
    assert any("line 9 of topor.toml commented out" in f.message for f in notes), notes
    log = (out.report_dir / "log.txt").read_text()
    assert "pre-flight: run.timeout_clock: line 5 of topor.toml commented out" in log
    # the next load: no warning for the lines commented out, the unticked one still warns
    from topor.config import load_config
    codes = [f.code for f in load_config(board.project_dir())[1]]
    assert codes == ["CFG_PINNED_OLD_DEFAULT"], codes



# ------------------------------------------------------------------ OK vs Cancel at the press (user 2026-11-17)
class _CancelWhen(ScriptedUi):
    """Presses the button once, the first time `when(p)` holds for the progress the dialog shows."""

    def __init__(self, when, **kw):
        super().__init__(**kw)
        self.waits_for_ok = True
        self.when, self.pressed = when, None

    def progress(self, p):
        super().progress(p)
        if self.pressed is None and self.when(p):
            self.pressed = p
        return self.pressed is None


@pytest.mark.parametrize("press", ["cancel_before_stable", "ok_at_stable"])
def test_the_button_read_at_the_press_reaches_the_engine(app, eprom, tmp_path, reports, press):
    """User 2026-11-17: the engine latches what the button read at the first press: ``job.cancel(ok=...)`` gets
    False for a Cancel (the progress not "Routes stable", e.g. at 0 of 5 stable) and True for OK at "Routes stable";
    the run's `cancelled` follows the same value (the proxy records it, the real job runs on)."""
    # a progress of the running engine job (its coordinator set `runs`), before / at "Routes stable"
    when = ((lambda p: getattr(p, "runs", 0) > 0 and not p.stable) if press == "cancel_before_stable"
            else (lambda p: p.stable))
    ui = _CancelWhen(when, accept=True)
    eng = ProxyEngine(hold=1)
    _, _, out = do_run(eprom, tmp_path, ui=ui, engine=eng, cfg=CFG.with_values({("optimize", "rounds"): 20}))
    assert ui.pressed is not None and eng.jobs[0].cancelled
    assert ui.pressed.stable is (press == "ok_at_stable"), ui.pressed
    assert eng.jobs[0].ok is (press == "ok_at_stable")
    if press == "ok_at_stable":
        assert out.cancelled is False
    else:
        assert out.cancelled is True


def test_a_cancel_pressed_just_before_routes_stable_stays_a_cancel(app, eprom, tmp_path, reports):
    """verify-sl3 F2: the dialog handles a click at its next poll, whose progress may be the first "Routes stable"
    (the button turns into OK on it). What the button read at the press (the UI's ``ok_at_press``: Cancel) is what
    reaches the engine, not the stable progress of that poll: ``job.cancel(ok=False)``, the run is a cancel."""
    class CancelQueued(_CancelWhen):
        def ok_at_press(self):
            return None if self.pressed is None else False  # the click came while the button read Cancel

    ui = CancelQueued(lambda p: p.stable, accept=True)
    eng = ProxyEngine(hold=1)
    _, _, out = do_run(eprom, tmp_path, ui=ui, engine=eng, cfg=CFG.with_values({("optimize", "rounds"): 20}))
    assert ui.pressed is not None and ui.pressed.stable, "premise: the poll that handles the press is stable"
    assert eng.jobs[0].cancelled and eng.jobs[0].ok is False
    assert out.cancelled is True
    log = (out.report_dir / "log.txt").read_text()
    assert "OK at Routes stable" not in log


def test_an_engine_without_ok_still_gets_the_cancel(app, eprom, tmp_path, reports):
    """An engine older than the plugin (``RouteJob.cancel()`` without ``ok``): the plugin falls back to the plain
    cancel, which judges by the engine's own progress (fail-safe: never an error)."""
    class OldJob(ProxyJob):
        def cancel(self):
            super().cancel()

    class OldEngine(ProxyEngine):
        def route_async(self, m, o):
            j = OldJob(topor_engine.route_async(m, o), self.hold, self.patch)
            self.jobs.append(j)
            return j

    eng = OldEngine(hold=1)
    ui = _CancelWhen(lambda p: p.stable, accept=True)
    _, _, out = do_run(eprom, tmp_path, ui=ui, engine=eng, cfg=CFG.with_values({("optimize", "rounds"): 20}))
    assert eng.jobs[0].cancelled and eng.jobs[0].ok is None and out.cancelled is False


def test_run_preview_draws_a_yellow_dot_per_engine_violation(app, eprom, tmp_path, reports, monkeypatch):
    """preview-errors (user 2026-11-21): the result's violations reach render_ex, the Result panel carries one dot each
    at its place (outline-corner offset) and the legend says how many."""
    import re
    seen = {}
    real = preview.render_ex

    def spy(*a, **kw):
        seen["errors"] = kw.get("errors")
        seen["png"] = a[2]
        return real(*a, **kw)

    monkeypatch.setattr(preview, "render_ex", spy)

    def with_viols(out_json):
        d = json.loads(out_json)
        d["violations"] = [{"conn": 0, "net": 1, "name": "N", "from": "a", "to": "b", "kind": "clearance", "layer": "F.Cu",
                            "at": [190_000_000, 70_000_000], "depth_nm": 1, "length_nm": 1, "against": "edge", "cause": "",
                            "price_mm": 1.0, "class": "static"},
                           {"conn": 0, "net": 1, "name": "N", "from": "a", "to": "b", "kind": "short", "layer": "B.Cu",
                            "at": [200_000_000, 75_000_000], "depth_nm": 1, "length_nm": 1, "against": "item:1", "cause": "",
                            "price_mm": 1.0, "class": "crossing"}]
        return json.dumps(d)

    board, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), engine=ProxyEngine(patch=with_viols), strict=False)
    assert [e["at"] for e in seen["errors"]] == [[190_000_000, 70_000_000], [200_000_000, 75_000_000]]
    legend = json.loads(preview.legend_path(seen["png"]).read_text())
    assert [e for e in legend if e["label"] == "yellow dot = engine-reported rule violation, 2"][0]["rgb"] == list(preview.ERROR_DOT_RGB)
    svg = (Path(seen["png"]).parent / "preview-work" / "after-errors.svg").read_text()
    grp = svg[svg.index("topor-errors"):]
    assert len(re.findall(r'<circle cx="[\d.]+" cy="[\d.]+" r="[\d.]+"', grp)) == 2
    assert "preview" in ui.names()
