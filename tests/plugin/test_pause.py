"""pause-button (user 2026-11-26, docs/dev/design/pause-button.md): the progress dialog's Pause through the plugin's
run flow, with the REAL engine job and the dialog's double (ScriptedUi ``pauses``):
- the run flow pauses / resumes the engine job (RouteJob.pause / resume) and leaves the paused seconds out of the
  router and optimizing time (what run.timeout_s limits), so a pause longer than the limit does not end the run;
- the copper of a run with a pause is the copper of the same run without it;
- Accept while paused draws the best so far, Cancel while paused takes nothing, both within seconds;
- kicad-cli is stopped while paused (SIGSTOP) and its timeout stands still (toporlib.kicad_cli.run_cli).

Real engine, real kicad-cli, FakeBoard (eprom snapshot copy + harness-built model; pcbnew only in the fixture)."""
import json
import sys
import time

import pytest

from toporlib import kicad_cli

wx = pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")

from harness.paths import board_pcb  # noqa: E402
from topor import run as run_mod  # noqa: E402
from topor.config import Config  # noqa: E402
from topor.fake import FakeBoard, ScriptedUi  # noqa: E402

BOARD = "eprom-to-kernal"
CFG = Config.defaults().with_values({("run", "existing_copper"): "rip", ("run", "timeout_s"): 60.0,
                                     ("run", "refill_after_accept"): False})


# ------------------------------------------------------------------ kicad-cli (no board needed)
BUSY = "import time\nt=time.process_time()\nwhile time.process_time()-t<{s}: pass\n"


def test_run_cli_pause_stops_the_process_and_its_timeout_stands_still():
    """A child that needs 0.8 s of CPU, with a 1.5 s timeout, paused for 2 s right after its start: it is stopped
    (no CPU while paused), the paused time is not counted (no timeout) and it ends after Resume."""
    t0 = time.monotonic()

    def poll():
        dt = time.monotonic() - t0
        return kicad_cli.PAUSE if 0.2 <= dt < 2.2 else True

    r = kicad_cli.run_cli([sys.executable, "-c", BUSY.format(s=0.8)], timeout_s=1.5, on_poll=poll, poll_s=0.05,
                          env=None)
    wall = time.monotonic() - t0
    assert r.returncode == 0
    assert wall >= 2.2 + 0.5, f"the child ran while paused: ended after {wall:.2f} s"


def test_run_cli_without_pause_still_times_out():
    with pytest.raises(kicad_cli.KicadCliTimeout):
        kicad_cli.run_cli([sys.executable, "-c", BUSY.format(s=5)], timeout_s=0.5, on_poll=lambda: True, poll_s=0.05)


def test_run_cli_cancel_while_paused_kills_the_stopped_process():
    t0 = time.monotonic()

    def poll():
        return kicad_cli.PAUSE if time.monotonic() - t0 < 0.5 else False

    with pytest.raises(kicad_cli.KicadCliCancelled):
        kicad_cli.run_cli([sys.executable, "-c", BUSY.format(s=30)], timeout_s=None, on_poll=poll, poll_s=0.05)
    assert time.monotonic() - t0 < 5


# ------------------------------------------------------------------ the run flow with the real engine
@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # noqa: BLE001 - no display
        pytest.skip(f"no display for wx: {e}")
    yield a


@pytest.fixture(autouse=True)
def _report_root(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(tmp_path / "reports"))


@pytest.fixture(scope="module")
def eprom(tmp_path_factory):
    pytest.importorskip("pcbnew")
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("proj")
    cp = copy_project(board_pcb(BOARD), d / BOARD)
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    return cp.pcb, export_model(b, rules, name=BOARD, dru_text=dru), cp.pcb.read_text()


class Recorder:
    """The real engine; records the jobs' pause / resume calls."""

    def __init__(self):
        self.calls = []

    def default_options(self):
        return topor_engine.default_options()

    def route_async(self, m, o):
        job = topor_engine.route_async(m, o)
        rec = self.calls

        class Job:
            def __getattr__(self, n):
                return getattr(job, n)

            def pause(self):
                rec.append(("pause", time.monotonic()))
                return job.pause()

            def resume(self):
                rec.append(("resume", time.monotonic()))
                return job.resume()

        return Job()


def _run(eprom, tmp_path, ui, cfg=CFG):
    pcb, model, _ = eprom
    board = FakeBoard(pcb, model, project_dir=pcb.parent)
    eng = Recorder()
    out = run_mod.run(board, ui, cfg, eng, tmp_root=tmp_path / "work")
    return board, out, eng


def _copper(board) -> str:
    """The committed copper, uuids left out (new items get fresh ones)."""
    return json.dumps([sorted(json.dumps({k: v for k, v in it.items() if k != "uuid"}, sort_keys=True) for it in req.items)
                       for req in board.commits])


def _first(p):
    """The first engine poll (the job's phases; the kicad-cli steps before it can pause too, with their own
    SIGSTOP, toporlib.kicad_cli)."""
    return p.phase in ("setup", "route")


class DialogUi(ScriptedUi):
    """The progress dialog's double: the engine never stops at stable by itself (only Accept, Cancel or the limit)."""

    def __init__(self, **kw):
        super().__init__(**kw)
        self.waits_for_ok = True


def _in_loop(p):
    return "TopoR loop" in (p.stage or "")


def test_a_pause_is_not_router_time_and_keeps_the_copper(app, eprom, tmp_path):
    """Headless: a 3 s pause at the first engine poll. The engine job is paused and resumed once, the router time
    leaves the 3 s out, and the committed copper is the copper of the same run without the pause."""
    b0, out0, _ = _run(eprom, tmp_path / "a", ScriptedUi())
    ui = ScriptedUi(pauses=[(_first, 3.0)])
    b1, out1, eng = _run(eprom, tmp_path / "b", ui)
    assert out0.status == out1.status == "accepted", (out0.status, out1.status)
    kinds = [k for k, _ in eng.calls]
    assert kinds == ["pause", "resume"], kinds
    paused = eng.calls[1][1] - eng.calls[0][1]
    assert paused >= 2.9
    assert any(p.paused for p in ui.paused_seen), "the progress said paused while the pause held"
    assert max(p.paused_s for p in ui.paused_seen) >= 2.5
    # router time: never counts the pause (the job ran about as long as without it)
    assert out1.router_s < out0.router_s + paused - 1.5, (out0.router_s, out1.router_s, paused)
    log = (out1.report_dir / "log.txt").read_text()
    assert "Pause pressed at" in log and "Resume pressed: the run goes on" in log
    assert _copper(b0) == _copper(b1), "the copper with a pause differs from the copper without it"


def test_a_pause_longer_than_the_limit_does_not_use_it_up(app, eprom, tmp_path):
    """The dialog's run (ends at the limit), a 6 s limit with no grace and an 8 s pause at the start: the run's clock
    stood still, so the optimizing still gets its 6 s after the pause (router time about 6 s, wall time >= 14 s);
    counted, the limit would have passed during the pause (router time >= 8 s)."""
    cfg = CFG.with_values({("run", "timeout_s"): 6.0, ("run", "limit_grace_s"): 0.0})
    ui = DialogUi(pauses=[(_first, 8.0)])
    t0 = time.monotonic()
    _, out, eng = _run(eprom, tmp_path, ui, cfg)
    wall = time.monotonic() - t0
    assert [k for k, _ in eng.calls] == ["pause", "resume"]
    assert out.time_limit.get("reached"), out.time_limit
    assert 5.5 <= out.router_s < 7.9, f"router time {out.router_s:.1f} s (the 8 s pause counted?)"
    assert wall >= 14.0


def test_accept_while_paused_draws_the_best_so_far(app, eprom, tmp_path):
    class AcceptWhilePaused(DialogUi):
        def progress(self, p):
            r = super().progress(p)
            if self.pause_wanted() and len(self.paused_seen) >= 6:  # 1.5 s into the pause
                self._pause_until = None
                return False  # Accept
            return r

    ui = AcceptWhilePaused(pauses=[(_in_loop, 30.0)])
    board, out, eng = _run(eprom, tmp_path, ui)
    assert out.status == "accepted" and board.commits and board.commits[-1].items, out.status
    kinds = [k for k, _ in eng.calls]
    assert kinds == ["pause"], f"the engine resumes itself at the stop request: {kinds}"
    log = (out.report_dir / "log.txt").read_text()
    assert "Accept / Cancel while paused: the pause ends" in log


def test_cancel_while_paused_takes_nothing_within_seconds(app, eprom, tmp_path):
    t = {}

    def cancel(p):
        if "t" not in t and getattr(p, "paused", False):
            t["t"] = time.monotonic()
        return "t" in t and time.monotonic() - t["t"] > 1.0

    ui = DialogUi(pauses=[(_in_loop, 30.0)], discard_when=cancel)
    board, out, eng = _run(eprom, tmp_path, ui)
    assert out.status == "cancelled" and not board.commits
    assert [k for k, _ in eng.calls] == ["pause"]
    assert time.monotonic() - t["t"] < 15


def test_the_last_round_preview_goes_on_after_resume(app, eprom, tmp_path):
    """Lead check (3): the dialog's last-round image (sketches) keeps coming after Resume, and the elapsed /
    optimizing figures the dialog shows leave the pause out (they stand still while paused)."""

    class Sketching(DialogUi):
        def __init__(self, **kw):
            super().__init__(**kw)
            self.sketches, self.resumed_at, self.figures = [], None, []

        def preview_wanted(self):
            return "last"

        def preview_interval_s(self):
            return 0.5

        def sketch(self, kind, d):
            self.sketches.append((time.monotonic(), kind, d.get("seq")))

        def progress(self, p):
            was = self.pause_wanted()
            r = super().progress(p)
            if was:
                self.figures.append((p.elapsed_s, p.optimize_s))
            if was and not self.pause_wanted():
                self.resumed_at = time.monotonic()
            if self.resumed_at and any(t > self.resumed_at for t, _, _ in self.sketches):
                return False  # Accept once a sketch came after Resume
            if self.resumed_at and time.monotonic() - self.resumed_at > 60:
                return False
            return r

    ui = Sketching(pauses=[(_in_loop, 3.0)])
    _, out, eng = _run(eprom, tmp_path, ui)
    assert [k for k, _ in eng.calls][:2] == ["pause", "resume"]
    assert ui.resumed_at and any(t > ui.resumed_at for t, _, _ in ui.sketches), \
        f"no last-round sketch after Resume: {ui.sketches}"
    # while paused, the figures stand still (the first paused poll may still carry the moment of the press)
    held = ui.figures[1:]
    assert len(held) >= 4 and max(e for e, _ in held) - min(e for e, _ in held) < 0.3, held
    assert max(o for _, o in held) - min(o for _, o in held) < 0.3, held
