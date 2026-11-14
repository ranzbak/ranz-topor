"""The time limit changed while the router runs (user 2026-11-07: "make the max run time editable during the run"):
the progress dialog's request (``UiPort.time_limit_request``, scripted by ``ScriptedUi(limit_changes=...)``) is
applied on both clocks, logged, and recorded in result.json and the report (start, every change, final).

Wall clock: the real engine job reads its limit live (``RouteJob.set_time_limit``). CPU clock: the plugin's poll
loop compares against the changed limit (``time.process_time`` replaced by a +1 s counter, as in
test_run_cpu_limit.py).
"""
import json
import re
import time

import pytest

pytest.importorskip("wx")
pytest.importorskip("topor_engine")

from test_run_flow import CFG, app, do_run, eprom, reports, _report_root  # noqa: E402,F401
from test_run_cpu_limit import RecordingEngine, _as  # noqa: E402
from topor.fake import ScriptedUi  # noqa: E402


@pytest.fixture
def cpu_counter(monkeypatch):
    n = {"t": 0.0}

    def fake():
        n["t"] += 1.0
        return n["t"]

    monkeypatch.setattr(time, "process_time", fake)
    return n


def _res(out):
    return json.loads((out.report_dir / "result.json").read_text())


def _log(out):
    return (out.report_dir / "log.txt").read_text()


def _fired_at(out) -> float:
    """CPU seconds at which the plugin's CPU limit fired (log line "CPU-time limit reached: X s of L s")."""
    m = re.search(r"CPU-time limit reached: ([0-9.]+) s of", _log(out))
    assert m, _log(out)
    return float(m.group(1))


def _routing(p):
    return p.limit_s is not None  # the routing's limit exists (after the pre-flight)


# ---------------------------------------------------------------- wall clock (the engine reads the limit live)
def test_wall_limit_raised_reaches_the_engine_and_is_reported(app, eprom, tmp_path, reports):
    eng = RecordingEngine(hold=4)
    ui = ScriptedUi(accept=False, limit_changes=[(_routing, 900.0)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=CFG, engine=eng)
    job = eng.jobs[0]
    assert eng.options[0]["wall_clock_s"] == 60.0  # the job starts with run.timeout_s
    assert len(job.limits) == 1 and job.limits[0] == pytest.approx(900.0, abs=1.0)
    assert job.time_limit() == pytest.approx(900.0, abs=1.0)  # the engine's own limit moved
    tl = _res(out)["time_limit"]
    assert tl["clock"] == "wall" and tl["start_s"] == 60.0 and tl["final_s"] == pytest.approx(900.0, abs=1.0)
    assert len(tl["changes"]) == 1 and tl["changes"][0]["limit_s"] == pytest.approx(900.0, abs=1.0)
    assert "time limit extended by the user" in _log(out) and "60 s -> 900 s" in _log(out)
    lines = "\n".join(out.report.summary_lines())
    assert "limit 900 s wall (raised during the run from 60 s, 1 change)" in lines
    md = (out.report_dir / "report.md").read_text()
    assert "Time limit (wall clock): 60 s at the start; at " in md and "set to 900 s; final 900 s" in md


def test_wall_limit_lowered_below_the_time_spent_stops_with_the_time_limit(app, eprom, tmp_path, reports):
    eng = RecordingEngine(hold=2)
    ui = ScriptedUi(accept=False, limit_changes=[(_routing, 0.001)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=CFG, engine=eng)
    job = eng.jobs[0]
    assert not job.cancelled, "lowering the limit is not a Cancel"
    assert job.limits and job.limits[0] <= 0.5  # at most the time spent: stop at the next check
    assert out.stop_reason == "timeout_wall" and out.cancelled is False
    assert out.items, "the best result so far is kept"
    f = {x.code: x for x in out.findings}
    assert "TIME_BUDGET_HIT" in f and "changed during the run" in f["TIME_BUDGET_HIT"].message
    log = _log(out)
    assert "time limit lowered by the user" in log and "60 s -> 0.001 s" in log, log
    assert _res(out)["time_limit"]["changes"][0]["limit_s"] == 0.001
    lines = "\n".join(out.report.summary_lines())
    assert "lowered during the run from 60 s" in lines and "time budget hit" in lines
    assert "set during the run" in lines  # the warning names the final limit


def test_wall_no_limit_during_the_run(app, eprom, tmp_path, reports):
    eng = RecordingEngine(hold=2)
    ui = ScriptedUi(accept=False, limit_changes=[(_routing, 0.0)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=CFG, engine=eng)
    assert eng.jobs[0].limits == [0.0] and eng.jobs[0].time_limit() is None
    tl = _res(out)["time_limit"]
    assert tl["final_s"] == 0.0 and tl["start_s"] == 60.0
    assert "removed by the user (no limit)" in _log(out)
    assert "no time limit (removed during the run from 60 s, 1 change)" in "\n".join(out.report.summary_lines())


def test_without_a_change_the_limit_is_fixed_and_reported(app, eprom, tmp_path, reports):
    eng = RecordingEngine(hold=2)
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=CFG, engine=eng)
    assert eng.jobs[0].limits == [] and eng.options[0]["wall_clock_s"] == 60.0
    # M2.TL: the limit's mode and whether it was reached (not here: no overrun, no step)
    assert _res(out)["time_limit"] == {"clock": "wall", "start_s": 60.0, "final_s": 60.0, "changes": [],
                                       "mode": "soft", "reached": False, "step": "", "overrun_s": 0.0,
                                       # F3: headless, run.timeout_overrun_s = auto = timeout_s; not fired
                                       "cap_s": 60.0, "cap_hit": False,
                                       # what the limit acting as hard cut (hard and the cap: one path): nothing
                                       "cut": "none", "cut_steps": []}
    assert "limit 60 s wall" in "\n".join(out.report.summary_lines())


# ---------------------------------------------------------------- CPU clock (the plugin's poll loop)
def test_cpu_limit_raised_does_not_fire(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=8)
    ui = ScriptedUi(accept=False, limit_changes=[(lambda p: p.cpu_s >= 1, 1.0e5)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, engine=eng)
    assert not eng.jobs[0].cancelled and eng.jobs[0].limits == []  # the CPU clock never touches the engine's limit
    assert out.stop_reason != "timeout_cpu" and out.router_cpu_s > 3.0
    assert _res(out)["time_limit"]["final_s"] == 1.0e5
    assert "TIME_BUDGET_HIT" not in {x.code for x in out.findings}


def test_cpu_limit_lowered_fires_at_the_new_value(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 1.0e5})
    eng = RecordingEngine(hold=12, patch=_as("cancel", timed_out=True))
    ui = ScriptedUi(accept=False, limit_changes=[(lambda p: p.cpu_s >= 1, 4.0)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, engine=eng)
    # M2.TL soft limit (default): the CPU clock reaches the engine's soft limit, not a Cancel
    assert eng.jobs[0].reached and not eng.jobs[0].cancelled and out.stop_reason == "timeout_cpu"
    assert 4.0 <= _fired_at(out) <= 5.0  # the job double keeps counting after the cancel: check when it fired
    f = {x.code: x for x in out.findings}
    assert "CPU-time limit of 4 s" in f["TIME_BUDGET_HIT"].message
    assert "changed during the run" in f["TIME_BUDGET_HIT"].message
    assert "CPU-time limit reached" in _log(out) and "of 4 s" in _log(out)


def test_cpu_limit_lowered_below_the_time_spent_stops_now(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 1.0e5})
    eng = RecordingEngine(hold=12, patch=_as("cancel", timed_out=True))
    ui = ScriptedUi(accept=False, limit_changes=[(lambda p: p.cpu_s >= 3, 0.5)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, engine=eng)
    assert eng.jobs[0].reached and not eng.jobs[0].cancelled and out.stop_reason == "timeout_cpu"  # M2.TL soft
    tl = _res(out)["time_limit"]
    assert tl["reached"] is True and tl["mode"] == "soft"
    assert tl["changes"][0]["limit_s"] == tl["changes"][0]["at_s"] >= 3.0  # clamped to the time spent
    assert _fired_at(out) <= tl["final_s"] + 1.0  # at the next poll


def test_cpu_no_limit_during_the_run(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=8)
    ui = ScriptedUi(accept=False, limit_changes=[(lambda p: p.cpu_s >= 1, 0.0)])
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, engine=eng)
    assert not eng.jobs[0].cancelled and out.router_cpu_s > 3.0
    assert _res(out)["time_limit"]["final_s"] == 0.0
