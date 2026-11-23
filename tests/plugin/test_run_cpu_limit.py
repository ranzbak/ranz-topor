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
    assert "TIME_BUDGET_HIT" in f and "CPU-time limit of 3 s" in f["TIME_BUDGET_HIT"].message
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


def test_cpu_clock_hard_limit_takes_the_engines_hard_path(app, eprom, tmp_path, reports, cpu_counter):
    # verify-m2c 1: the CPU clock's hard limit is reached in the engine and acts as hard (reach_limit, then harden):
    # the same path as the wall clock's hard limit (the step that runs is cut, the kept plan drawn in full); never a
    # Cancel, which keeps no plan during the initial routing (NOTHING_TO_DRAW)
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0, ("run", "timeout_mode"): "hard"})
    eng = RecordingEngine(hold=12, patch=_as("timeout", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert eng.jobs[0].reached and eng.jobs[0].hardened and not eng.jobs[0].cancelled
    assert eng.options[0]["timeout_mode"] == "hard"
    assert out.stop_reason == "timeout_cpu" and out.cancelled is False
    assert out.items, "the kept plan is drawn"
    tl = _result_json(out)["time_limit"]
    assert tl["mode"] == "hard" and tl["reached"] is True and tl["cap_hit"] is False, tl


def test_cpu_clock_hard_limit_cancels_an_engine_without_reach(app, eprom, tmp_path, reports, cpu_counter):
    # an engine older than the plugin (no RouteJob.reach_limit / harden): Cancel as the fallback, said loudly
    from test_run_flow import ProxyJob

    class Old(ProxyJob):
        reach_limit = None
        harden = None

    class Eng(RecordingEngine):
        def route_async(self, m, o):
            job = super().route_async(m, o)
            old = Old(job.real, job.hold, job.patch, end_real=job.end_real)
            self.jobs[-1] = old
            return old

    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0, ("run", "timeout_mode"): "hard"})
    eng = Eng(hold=12, patch=_as("cancel", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert eng.jobs[0].cancelled
    assert out.stop_reason == "timeout_cpu"
    assert "cannot reach its time limit from the CPU clock" in (out.report_dir / "log.txt").read_text()


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
            mj = Measuring(job.real, job.hold, job.patch, end_real=job.end_real)
            self.jobs[-1] = mj
            return mj

    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = Eng(hold=14, patch=_as("timeout", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    tl = _result_json(out)["time_limit"]
    assert eng.jobs[0].reached and tl["reached"] is True
    # time-limit-fix r2: on the limit's clock, the optimizing CPU seconds (the final drawing is not counted)
    assert tl["overrun_s"] == pytest.approx(max(0.0, out.optimize_cpu_s - 4.0 - 3.0), abs=1e-3), (tl, out.optimize_cpu_s)
    assert out.optimize_cpu_s <= out.router_cpu_s


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


def test_headless_overrun_cap_acts_as_the_hard_limit_and_warns(app, eprom, tmp_path, reports, cpu_counter):
    # F3 run.timeout_overrun_s = auto (default): headless = timeout_s, so past 2 x the limit the soft limit acts as the
    # hard one (RouteJob.harden on the CPU clock), loudly: log, finding, report, result.json
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=20, patch=_as("timeout", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    job = eng.jobs[0]
    assert job.reached and job.hardened and not job.cancelled
    assert eng.options[0]["timeout_overrun_s"] == 0.0, "the CPU clock caps in the plugin, not on the engine's wall clock"
    tl = _result_json(out)["time_limit"]
    assert tl["cap_s"] == 3.0 and tl["cap_hit"] is True, tl
    f = {x.code: x for x in out.findings}
    assert "TIME_OVERRUN_CAP" in f and "CPU clock" in f["TIME_OVERRUN_CAP"].message, list(f)
    assert "overrun cap" in (out.report.dir / "report.md").read_text()
    assert "WARNING: time limit overrun cap reached" in (out.report_dir / "log.txt").read_text()


def test_headless_wall_clock_overrun_cap_goes_to_the_engine(app, eprom, tmp_path, reports, cpu_counter):
    cfg = CFG.with_values({("run", "timeout_s"): 60.0, ("run", "timeout_overrun_s"): 15.0})
    eng = RecordingEngine(hold=3)
    do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert eng.options[0]["timeout_overrun_s"] == 15.0 and eng.options[0]["wall_clock_s"] == 60.0


class _GuiCancelLate(ScriptedUi):
    """The progress dialog (waits_for_ok) whose user presses Cancel only at 14 CPU s (limit 3 s: past 2 x the limit)."""

    def __init__(self, **kw):
        super().__init__(**kw)
        self.waits_for_ok = True

    def progress(self, p):
        super().progress(p)
        return not (p.cpu_s >= 14.0)


def test_gui_has_no_overrun_cap_and_keeps_going_until_cancel(app, eprom, tmp_path, reports, cpu_counter):
    # F3: in the KiCad dialog auto = no cap: the run goes on past 2 x the limit until the user's Cancel (F1)
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = RecordingEngine(hold=40, patch=_as("cancel", timed_out=True))
    _, _, out = do_run(eprom, tmp_path, ui=_GuiCancelLate(accept=False), cfg=cfg, engine=eng)
    job = eng.jobs[0]
    assert job.reached and job.cancelled and not job.hardened
    assert eng.options[0]["timeout_overrun_s"] == 0.0
    tl = _result_json(out)["time_limit"]
    assert tl["cap_s"] is None and tl["cap_hit"] is False, tl
    assert out.router_cpu_s >= 14.0, "it went on past twice the limit"
    assert "TIME_OVERRUN_CAP" not in {x.code for x in out.findings}


def test_the_engines_cut_steps_reach_result_json(app, eprom, tmp_path, reports, cpu_counter):
    # verify-m2d O4: time_limit.cut / cut_steps of the engine's limit report are carried into result.json
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0, ("run", "timeout_mode"): "hard"})
    lim = {"limit_s": 0.0, "mode": "hard", "reached": True, "step": "drawing the traces", "overrun_s": 0.0,
           "cap_s": None, "cap_hit": False, "cut": "drawing", "cut_steps": ["drawing", "via_reduction"]}
    eng = RecordingEngine(hold=12, patch=_as("timeout_cpu", timed_out=True, limit=lim))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    tl = _result_json(out)["time_limit"]
    assert tl["cut"] == "drawing" and tl["cut_steps"] == ["drawing", "via_reduction"], tl


# ------------------------------------------------------------------ stop_reason x clock x dialog/headless
class _Dialog(ScriptedUi):
    """The progress dialog (``waits_for_ok``) whose user never presses OK / Cancel."""

    def __init__(self, **kw):
        super().__init__(**kw)
        self.waits_for_ok = True


#: time-limit-fix: "loop_time_share" = the TopoR loop's own share of the wall limit ended the loop, the limit was not
#: reached; the plugin keeps it on both clocks (never timeout_wall / timeout_cpu: no limit fired)
ENGINE_LABELS = ["stable", "round cap", "timeout", "cancel", "stop_after plan", "loop_time_share"]


def _expected(label, clock):
    """(stop_reason, TIME_BUDGET_HIT, cancelled) the plugin reports for the engine's `label` when the clock's limit
    fired during the job (wall: the engine's own deadline; cpu: the plugin's reach_limit). verify-slfx F2: only an
    engine "timeout" / "cancel" means the CPU limit cut the run; "stable" / "round cap" = it had ended (or was
    stable) before the limit cut anything, so no timeout_cpu and no "result may differ between runs"."""
    if label == "loop_time_share":
        return label, True, False
    if clock == "cpu" and label in ("timeout", "cancel"):
        return "timeout_cpu", True, False
    if label == "timeout":
        return "timeout_wall", True, False
    return label, False, label == "cancel"


@pytest.mark.parametrize("mode", ["dialog", "headless"])
@pytest.mark.parametrize("clock", ["wall", "cpu"])
@pytest.mark.parametrize("label", ENGINE_LABELS)
def test_stop_reason_for_every_engine_label_clock_and_mode(app, eprom, tmp_path, reports, cpu_counter, label, clock,
                                                           mode):
    """Definition of done 13: every stop_reason the engine can give, under both clocks, with and without the dialog:
    what the plugin reports (stop_reason, result.json, the summary, TIME_BUDGET_HIT, cancelled). The CPU limit fires
    during the job (3 s, +1 CPU s per poll, 12 polls); the wall clock's limit is the engine's, so its label says."""
    cfg = CFG.with_values({("run", "timeout_clock"): clock, ("run", "timeout_s"): 3.0 if clock == "cpu" else 600.0,
                           ("optimize", "rounds"): 20})
    timed_out = label in ("timeout", "cancel", "loop_time_share")
    warn = {"warnings": ["time budget hit: result may differ between runs"]} if label in ("timeout", "loop_time_share") else {}
    eng = RecordingEngine(hold=12, patch=_as(label, timed_out=timed_out, **warn))
    ui = _Dialog(accept=False) if mode == "dialog" else ScriptedUi(accept=False)
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=cfg, engine=eng)
    assert eng.options[0].get("stop_when_stable", True) is (mode == "headless")  # headless: the engine default
    assert eng.jobs[0].reached is (clock == "cpu") and not eng.jobs[0].cancelled
    want, tbh, cancelled = _expected(label, clock)
    codes = {x.code for x in out.findings}
    assert (out.stop_reason, "TIME_BUDGET_HIT" in codes, out.cancelled) == (want, tbh, cancelled), (label, clock, mode)
    assert _result_json(out)["stop_reason"] == want
    # time-limit-fix r2: the stop reason, then its words in brackets, then the cycles
    assert any(line.startswith(f"Stopped: {want} (") and ") after " in line for line in out.report.summary_lines()), \
        out.report.summary_lines()
    if tbh and clock == "cpu" and label != "loop_time_share":
        f = next(x for x in out.findings if x.code == "TIME_BUDGET_HIT")
        assert "CPU-time limit of 3 s" in f.message and "result may differ between runs" in f.message
    if label == "loop_time_share":
        # the share's stop is no CPU limit hit (a real engine gives it only on the wall clock: the CPU clock gives the
        # engine no deadline): the engine's own line stays, the plugin adds no CPU-limit line
        f = next(x for x in out.findings if x.code == "TIME_BUDGET_HIT")
        assert "CPU-time limit" not in f.message, f.message


# ------------------------------------------------------------------ OK at Routes stable, then the limit cut the drawing
OK_CUT_WHY = ("OK came at Routes stable, but the time limit then cut the drawing of the stable plan: the board is "
              "drawn only in part (see the time limit line)")


@pytest.mark.parametrize("clock", ["wall", "cpu"])
def test_ok_at_stable_then_an_engine_timeout_is_a_timeout_that_names_the_ok(app, eprom, tmp_path, reports, cpu_counter,
                                                                         clock):
    """verify-sl4 N1 (rule 13, the Python side): the OK reached the engine (``job.cancel(ok=True)``), then the hard
    limit cut the drawing of the stable plan, so the engine says "timeout" with its TIME_HIT reason "OK came at Routes
    stable, but ...". The plugin reports a timeout on the run's clock (never "stable", never a cancel) and keeps that
    reason: on the wall clock the engine's line itself, on the CPU clock inside the plugin's own CPU line (which
    replaces the engine's clock-less one)."""
    class OkAtStable(ScriptedUi):  # the progress dialog: OK once Routes stable has a pick to keep
        def __init__(self, **kw):
            super().__init__(**kw)
            self.waits_for_ok = True

        def progress(self, p):
            super().progress(p)
            return not (p.stable and p.best_total)

    cfg = CFG.with_values({("run", "timeout_clock"): clock, ("run", "timeout_s"): 3.0 if clock == "cpu" else 600.0,
                           ("run", "timeout_mode"): "hard", ("optimize", "rounds"): 20})
    eng = RecordingEngine(hold=12, patch=_as("timeout", timed_out=True,
                                             warnings=[f"time budget hit: result may differ between runs ({OK_CUT_WHY})"]))
    _, _, out = do_run(eprom, tmp_path, ui=OkAtStable(accept=False), cfg=cfg, engine=eng)
    assert eng.jobs[0].cancelled and eng.jobs[0].ok is True, "premise: the OK reached the engine"
    want = f"timeout_{clock}"
    assert (out.stop_reason, out.cancelled) == (want, False)
    assert _result_json(out)["stop_reason"] == want
    # time-limit-fix r2: the stop reason, then its words in brackets, then the cycles
    assert any(line.startswith(f"Stopped: {want} (") and ") after " in line for line in out.report.summary_lines()), \
        out.report.summary_lines()
    f = next(x for x in out.findings if x.code == "TIME_BUDGET_HIT")
    assert f"({OK_CUT_WHY})." in f.message, f.message  # whole, its own inner parenthesis closed, then the line's
    assert ("CPU-time limit of 3 s" in f.message) is (clock == "cpu"), f.message
    log = (out.report_dir / "log.txt").read_text()
    assert "OK at Routes stable" in log


def test_the_cpu_line_carries_no_other_engine_reason(app, eprom, tmp_path, reports, cpu_counter):
    """verify-sl4 N1: only the OK / stable reason is kept on the CPU clock; the engine's other reasons name its own
    (wall) limit, which the CPU line replaces."""
    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    why = "the caller's own time limit ([run] timeout_s on its clock) stopped the run before its routes were stable"
    eng = RecordingEngine(hold=12, patch=_as("timeout", timed_out=True,
                                             warnings=[f"time budget hit: result may differ between runs ({why})"]))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    f = next(x for x in out.findings if x.code == "TIME_BUDGET_HIT")
    assert out.stop_reason == "timeout_cpu" and "CPU-time limit of 3 s" in f.message and why not in f.message, f.message


# ------------------------------------------------------------------ time-limit-fix: one router clock
def test_router_time_is_the_clock_the_dialog_shows_and_the_summary_prints(app, eprom, tmp_path, reports, cpu_counter):
    """User 2026-11-22: the dialog's Router time, the summary's Stopped line and result.json's router_s are one clock
    (the engine job's wall seconds, what the wall limit counts); the whole run up to the result (pre-flight, KiCad's
    DRC) is shown apart and is longer."""
    eng = RecordingEngine(hold=6, patch=_as("loop_time_share", timed_out=True, threads_used=3, iterations=7,
                                             warnings=["time budget hit: result may differ between runs (share)"]))
    _, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=CFG.with_values({("run", "timeout_s"): 60.0}),
                        engine=eng)
    shown = [c[1][0].elapsed_s for c in ui.calls if c[0] == "progress"]
    res = _result_json(out)
    assert res["router_s"] == pytest.approx(shown[-1], abs=1e-3) and shown[-1] == max(shown) > 0
    assert out.stop_reason == "loop_time_share" and res["stop_reason"] == "loop_time_share"
    line = next(ln for ln in out.report.summary_lines() if ln.startswith("Stopped:"))
    assert line.startswith("Stopped: loop_time_share (the TopoR loop's own time share ended it, the limit was not "
                           f"reached) after 7 cycles, optimizing {round(out.optimize_s, 1)} s, drawing "
                           f"{round(out.draw_s, 1)} s (after the limit, not counted), router time "
                           f"{round(out.router_s, 1)} s, 3 threads")
    # time-limit-fix r2: the optimizing time (the limit's clock) and the drawing apart, in result.json too
    assert res["optimize_s"] == pytest.approx(out.optimize_s, abs=1e-3) and res["draw_s"] == pytest.approx(out.draw_s, abs=1e-3)
    assert 0 < out.optimize_s <= out.router_s and res["time_limit"]["scope"] == "optimizing"
    assert "limit 60 s wall, not reached (" in line
    run_s = out.report.facts["run_s"]
    assert run_s > out.router_s and f"whole run {run_s} s to the result" in line
    log = (out.report_dir / "log.txt").read_text()
    assert "3 threads, 7 cycles (picked run)" in log


def test_the_cpu_clock_counts_optimizing_only_not_the_final_drawing(app, eprom, tmp_path, reports, cpu_counter):
    """time-limit-fix r2 (user 2026-11-22: the limit is optimizing time, the drawing afterwards does not count): with
    run.timeout_clock = cpu the limit counts the CPU seconds up to the final drawing only. A job that draws from its
    first poll (progress ``drawing``) keeps using CPU seconds (router CPU) but none of the limit's: a 3 s CPU limit is
    never reached, the engine is never told to reach it, and the summary and result.json show both apart."""
    from test_run_flow import ProxyJob

    class Drawing(ProxyJob):
        def progress(self):
            d = json.loads(super().progress())
            d.update(drawing=True, optimize_s=0.0, draw_s=1.0)
            return json.dumps(d)

    class Eng(RecordingEngine):
        def route_async(self, m, o):
            job = super().route_async(m, o)
            dj = Drawing(job.real, job.hold, job.patch, end_real=job.end_real)
            self.jobs[-1] = dj
            return dj

    cfg = CFG.with_values({("run", "timeout_clock"): "cpu", ("run", "timeout_s"): 3.0})
    eng = Eng(hold=14, patch=_as("stable"))
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False), cfg=cfg, engine=eng)
    assert not eng.jobs[0].reached and not eng.jobs[0].cancelled
    assert out.stop_reason == "stable" and out.router_cpu_s >= 10.0 and out.optimize_cpu_s == 0.0
    res = _result_json(out)
    assert res["time_limit"]["reached"] is False and res["optimize_cpu_s"] == 0.0 and res["draw_s"] == 1.0
    assert not any(x.code == "TIME_BUDGET_HIT" for x in out.findings)
