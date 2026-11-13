"""B5 run flow: save_as copy -> rules -> model -> route (progress / cancel) -> DRC loop -> SVG preview ->
Accept = one commit / Reject = nothing -> report folder.

Real engine, real kicad-cli, FakeBoard (eprom snapshot copy + harness-built model; pcbnew only here, in the
test fixture) and ScriptedUi. wx needs an App for the preview renderer: `wx.App(False)`, no window is ever
shown (Ruling C18). Every report folder goes under a tmp TOPOR_REPORT_ROOT (Ruling C1).
"""
import json
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
from topor.ports import Report  # noqa: E402
from toporlib import pcbfile  # noqa: E402
from toporlib.drc import classify, run_drc  # noqa: E402

BOARD = "eprom-to-kernal"
MARKER = "11111111-2222-4333-8444-555555555555"  # a valid uuid: kicad-cli re-serialises saved boards
LIVE_EDIT = (f'\t(gr_line\n\t\t(start 190 70)\n\t\t(end 191 70)\n\t\t(stroke\n\t\t\t(width 0.1)\n'
             f'\t\t\t(type default)\n\t\t)\n\t\t(layer "Cmts.User")\n\t\t(uuid "{MARKER}")\n\t)\n')

CFG = Config.defaults().with_values({("run", "existing_copper"): "rip", ("run", "timeout_s"): 60.0})


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


def do_run(eprom, tmp_path, *, ui=None, cfg=CFG, engine=topor_engine, board=None, strict=True):
    board = board or make_board(eprom)
    ui = ui or ScriptedUi()
    out = run_mod.run(board, ui, cfg, engine, tmp_root=tmp_path / "work")
    if strict:
        _ok(out)
    return board, ui, out


# ------------------------------------------------------------------ engine doubles
class ProxyJob:
    """Delegates to a real RouteJob; `done()` stays False for `hold` polls so the progress loop runs."""

    def __init__(self, real, hold=1, patch=None):
        self.real, self.hold, self.polls, self.cancelled, self.patch = real, hold, 0, False, patch
        self.limits = []  # set_time_limit calls (the progress dialog's live limit, wall clock)

    def progress(self):
        self.polls += 1
        d = json.loads(self.real.progress())
        d["iteration"] = max(d.get("iteration", 0), 1)
        return json.dumps(d)

    def done(self):
        return self.polls >= self.hold and self.real.done()

    def cancel(self):
        self.cancelled = True  # recorded only: the real job has to finish for a deterministic result

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
        j = ProxyJob(topor_engine.route_async(m, o), self.hold, self.patch)
        self.jobs.append(j)
        return j


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
    names = ui.names()
    assert names.index("preview") < names.index("done")
    assert [c for c in ui.calls if c[0] == "done"][0][1][0] == "accepted"
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


def test_run_reject_leaves_board_untouched(rejected, eprom):
    board, ui, out, _ = rejected
    assert out.status == "rejected" and out.commit is None
    assert board.commits == [] and board.refills == 0 and board.actions == []
    assert board.text.count(MARKER) == 1 and eprom.text == eprom.pcb.read_text()
    assert not any(c[0] in ("commit", "refill_zones", "run_action") for c in board.calls)
    done = [c for c in ui.calls if c[0] == "done"]
    assert len(done) == 1 and done[0][1][0] == "rejected" and done[0][1][2] == out.report_dir
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


def test_run_cancel_with_nothing_routed_is_cancelled(app, eprom, tmp_path, reports):
    eng = ProxyEngine(hold=2, patch=_nothing_routed)
    ui = ScriptedUi(accept=True, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng)
    assert out.status == "cancelled" and board.commits == []
    assert "preview" not in ui.names()
    assert [c[1][0] for c in ui.calls if c[0] == "done"] == ["cancelled"]


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
    assert "done" not in ui.names() and "preview" not in ui.names()
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

    def cancel(self):
        super().cancel()
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


@pytest.mark.parametrize("step", ["refill", "action", "report", "done"])
def test_run_failure_after_commit_stays_accepted(app, eprom, tmp_path, reports, monkeypatch, step):
    """Ruling C30 (gate review I-1): a failure after the successful commit (refill, KiCad action, report write,
    the done dialog) keeps status "accepted" and adds the POST_COMMIT_STEP_FAILED warning; never INTERNAL /
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

    class Ui(ScriptedUi):
        def done(self, outcome, report, report_dir):
            super().done(outcome, report, report_dir)
            if step == "done":
                raise RuntimeError("wx: dialog could not be created")

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
    if step == "done":
        assert [e.code for e in errs] == ["POST_COMMIT_STEP_FAILED"]
    else:
        assert errs == []
        assert [c[1][0] for c in ui.calls if c[0] == "done"] == ["accepted"]
    if step in ("refill", "action"):
        res = json.loads((out.report_dir / "result.json").read_text())
        assert res["status"] == "accepted" and "POST_COMMIT_STEP_FAILED" in (out.report_dir / "report.md").read_text()
    if step == "refill":
        assert board.refills == 1 and out.report.facts["zones_refilled"] is False and "zone refill" in pc[0].message
    if step == "action":  # refill_after_accept = true: exactly one refill after the commit (ruling C28)
        assert board.refills == 1 and out.report.facts["zones_refilled"] is True
        assert board.actions == [run_mod.ACTION_CLEANUP] and run_mod.ACTION_CLEANUP in pc[0].message
        assert "zones refilled" in (out.report_dir / "report.md").read_text()


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
    rp, rv = pourreach.pour_reach(gnd["net"], gnd["layers"], want, model["pads"], model["vias"])
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
    cfg = CFG.with_values({("optimize", "finish_when_stable"): False})  # else eprom may finish before the OK poll
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=_RealCancelEngine(), cfg=cfg)
    assert out.status == "accepted" and out.routed == 27
    assert out.stop_reason == "stable" and out.cancelled is False
    stable = [c[1][0] for c in ui.calls if c[0] == "progress" and c[1][0].stable]
    assert stable and stable[0].best_total == 27 and stable[0].runs == 5
    log = (out.report_dir / "log.txt").read_text()
    assert "OK at Routes stable: best so far 27/27 routed" in log


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
