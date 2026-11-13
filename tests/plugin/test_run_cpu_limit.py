"""run.timeout_clock: the CPU-time limit (plugin-side cancel), the unchanged wall limit, router_cpu_s reported.

Real engine for the routed result (like test_run_flow.py), wrapped in a job double that stays "running" for a few
polls; ``time.process_time`` is replaced by a counter (+1 CPU second per call) so the test needs no real load.
"""
import json
import time

import pytest

pytest.importorskip("wx")
pytest.importorskip("topor_engine")

from test_run_flow import (CFG, ProxyEngine, app, do_run, eprom, reports, _report_root)  # noqa: E402,F401
from topor.fake import ScriptedUi  # noqa: E402


def _as(stop_reason, **extra):
    def patch(out_json):
        d = json.loads(out_json)
        d.update(stop_reason=stop_reason, **extra)
        return json.dumps(d)
    return patch


class RecordingEngine(ProxyEngine):
    def __init__(self, **kw):
        super().__init__(**kw)
        self.options = []

    def route_async(self, m, o):
        self.options.append(json.loads(o))
        return super().route_async(m, o)


@pytest.fixture
def cpu_counter(monkeypatch):
    n = {"t": 0.0}

    def fake():
        n["t"] += 1.0
        return n["t"]

    monkeypatch.setattr(time, "process_time", fake)
    return n


def _result_json(out):
    return json.loads((out.report_dir / "result.json").read_text())


def test_cpu_clock_cancels_the_job_and_keeps_the_best(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=12, patch=_as("cancel", timed_out=True))
    ui = ScriptedUi(accept=False)
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, engine=eng)
    # M2.TL soft limit (default): the CPU limit is the engine's soft limit, not a Cancel
    assert eng.jobs[0].reached and not eng.jobs[0].cancelled
    assert eng.options[0]["wall_clock_s"] == 0.0  # the engine's own wall limit is off in cpu mode
    assert out.stop_reason == "timeout_cpu" and out.cancelled is False
    assert out.items, "the best result so far is kept"
    assert out.router_cpu_s >= 3.0
    f = {x.code: x for x in out.findings}
    assert "TIME_BUDGET_HIT" in f and "CPU-time limit" in f["TIME_BUDGET_HIT"].message
    assert any("timeout_cpu" in line for line in out.report.summary_lines())
    res = _result_json(out)
    assert res["router_cpu_s"] == pytest.approx(out.router_cpu_s, abs=0.01) and res["stop_reason"] == "timeout_cpu"
    assert "CPU" in " ".join(out.report.summary_lines())


def test_cpu_clock_below_the_limit_does_not_fire(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 1.0e6})
    eng = RecordingEngine(hold=3)
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert not eng.jobs[0].cancelled
    assert out.stop_reason != "timeout_cpu" and out.router_cpu_s > 0
    assert "TIME_BUDGET_HIT" not in {x.code for x in out.findings}


def test_wall_clock_is_unchanged_and_cpu_time_is_still_reported(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_s"): 60.0})  # timeout_clock stays "wall"
    eng = RecordingEngine(hold=6, patch=_as("timeout", timed_out=True,
                                             warnings=["time budget hit: result may differ between runs"]))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert not eng.jobs[0].cancelled  # the plugin never cancels on the wall clock: the engine's deadline does
    assert eng.options[0]["wall_clock_s"] == 60.0
    assert out.stop_reason == "timeout_wall"
    assert "TIME_BUDGET_HIT" in {x.code for x in out.findings}
    assert out.router_cpu_s >= 5.0 and _result_json(out)["router_cpu_s"] >= 5.0


def test_cpu_clock_soft_limit_reports_where_and_how_far_past(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=12, patch=_as("timeout", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    tl = _result_json(out)["time_limit"]
    assert tl["mode"] == "soft" and tl["reached"] is True and tl["overrun_s"] >= 0.0
    assert "reached during" in (out.report.dir / "report.md").read_text()


def test_cpu_clock_hard_limit_cancels_the_job(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0, ("run", "timeout_mode"): "hard"})
    eng = RecordingEngine(hold=12, patch=_as("cancel", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert eng.jobs[0].cancelled and not eng.jobs[0].reached
    assert eng.options[0]["timeout_mode"] == "hard"
    assert out.stop_reason == "timeout_cpu"


def test_cancel_before_any_plan_is_a_loud_error(app, eprom, tmp_path, reports, cpu_counter):
    # M2.TL fail fast: the engine kept no plan (Cancel during the initial routing): an error, never a quiet cancel
    why = "nothing to draw yet: Cancel / OK came before the initial routing of any run had ended"
    eng = RecordingEngine(hold=2, patch=_as("cancel", items=[], routed=0, warnings=[f"ERROR: {why}", why]))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=CFG, engine=eng, strict=False)
    assert out.status == "error" and out.error is not None and out.error.code == "NOTHING_TO_DRAW", out.status
    assert why in out.error.message


def test_soft_limit_overrun_leaves_out_the_measurements(app, eprom, tmp_path, reports, cpu_counter):
    # M2.TL: the overrun is router time past the limit; the engine's measurements outside the limit (M1 compass cut
    # test, Progress.measure_s) are not part of it, as they are not part of the limit
    from test_run_flow import ProxyJob

    class Measuring(ProxyJob):
        def progress(self):
            d = json.loads(super().progress())
            d["measure_s"] = 4.0
            return json.dumps(d)

    class Eng(RecordingEngine):
        def route_async(self, m, o):
            job = super().route_async(m, o)
            mj = Measuring(job.real, job.hold, job.patch)
            self.jobs[-1] = mj
            return mj

    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = Eng(hold=14, patch=_as("timeout", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    tl = _result_json(out)["time_limit"]
    assert eng.jobs[0].reached and tl["reached"] is True
    assert tl["overrun_s"] == pytest.approx(max(0.0, out.router_cpu_s - 4.0 - 3.0), abs=1e-3), (tl, out.router_cpu_s)


class _CancelOnceLimitPassed(ScriptedUi):
    """Presses Cancel once the CPU soft limit (3 s) is well behind (8 CPU s on the counter)."""

    def progress(self, p):
        super().progress(p)
        return not (p.cpu_s >= 8.0)


def test_cancel_after_the_cpu_soft_limit_still_cancels_the_job(app, eprom, tmp_path, reports, cpu_counter):
    # M2.TL: the soft limit lets the current step finish; Cancel after it must still stop now (found by verify-m2:
    # the soft limit used the Cancel bookkeeping, so the later Cancel never reached the engine)
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=20, patch=_as("timeout", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=_CancelOnceLimitPassed(accept=False), cfg=cfg, engine=eng)
    job = eng.jobs[0]
    assert job.reached, "the soft limit came first"
    assert job.cancelled, "the user's Cancel after the soft limit never reached the engine job"
