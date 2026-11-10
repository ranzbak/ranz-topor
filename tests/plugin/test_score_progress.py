"""Score display (user 2026-11-11: "It would help if I can see the current score, and the best score in the UI"):
the engine's per-round score and round lines reach the UI's progress and log.txt (one line per round, once).

The run flow test uses the real engine for the routed result, wrapped in a job double whose progress carries a
scripted score (like test_run_cpu_limit.py); the label formatting is tested in test_ui_wx.py.
"""
import json

import pytest

from topor import run as run_mod
from topor.ports import Progress

L1 = ("run 1, round 1: errors 3, open 0, vias 166, length 7.384 m, shrinkages 1; rule violations 3, routed 426 of "
      "426: new best (the first result)")
L2 = ("run 2, round 1: errors 5, open 0, vias 168, length 7.412 m, shrinkages 1; rule violations 5, routed 426 of "
      "426: worse than the best (run 1, round 1): errors +2")
L3 = "run 1, round 2: not judged (cut short by the time limit or Cancel / OK)"


def test_round_log_lines_writes_each_round_once_and_says_a_gap():
    p = Progress(rounds_ended=2, round_lines=[L1, L2])
    assert run_mod.round_log_lines(p, 0) == [f"round ended: {L1}", f"round ended: {L2}"]
    assert run_mod.round_log_lines(p, 1) == [f"round ended: {L2}"]
    assert run_mod.round_log_lines(p, 2) == []
    # the engine keeps the last lines only: rounds 1-3 fell out of its list before the poll
    q = Progress(rounds_ended=5, round_lines=[L2, L3])
    assert run_mod.round_log_lines(q, 1) == ["round log: 2 round lines missed", f"round ended: {L2}",
                                             f"round ended: {L3}"]
    # an old engine without the fields
    assert run_mod.round_log_lines(Progress.from_json({"phase": "route"}), 0) == []


def test_progress_from_json_reads_the_score():
    d = {"phase": "route", "best_run": 1, "best_round": 1, "best_errors": 3, "best_open": 0, "best_vias": 166,
         "best_length_mm": 7384.0, "best_shrinkages": 1, "last_run": 2, "last_round": 1, "last_errors": 5,
         "last_open": 0, "last_violations": 5, "last_vias": 168, "last_length_mm": 7412.0, "last_shrinkages": 1,
         "last_routed": 426, "last_vs_best": "worse", "last_decider": "errors", "last_delta": 2.0,
         "rounds_ended": 2, "round_lines": [L1, L2]}
    p = Progress.from_json(d)
    assert (p.best_run, p.best_round, p.best_errors, p.best_open, p.best_vias) == (1, 1, 3, 0, 166)
    assert p.best_shrinkages == 1
    assert (p.last_run, p.last_round, p.last_errors, p.last_open, p.last_violations) == (2, 1, 5, 0, 5)
    assert p.last_vias == 168
    assert (p.last_length_mm, p.last_shrinkages, p.last_routed) == (7412.0, 1, 426)
    assert (p.last_vs_best, p.last_decider, p.last_delta) == ("worse", "errors", 2.0)
    assert (p.rounds_ended, p.round_lines) == (2, [L1, L2])
    assert Progress.from_json({"phase": "x"}).round_lines == []


pytest.importorskip("wx")
pytest.importorskip("topor_engine")

from test_run_flow import CFG, ProxyEngine, ProxyJob, app, do_run, eprom, reports, _report_root  # noqa: E402,F401
from topor.fake import ScriptedUi  # noqa: E402

# per poll: (rounds_ended, round_lines, score fields)
SCRIPT = [
    (1, [L1], dict(best_run=1, best_round=1, best_errors=3, best_vias=166, best_length_mm=7384.0, last_run=1,
                   last_round=1, last_errors=3, last_vias=166, last_length_mm=7384.0, last_vs_best="new best")),
    (3, [L1, L2, L3], dict(best_run=1, best_round=1, best_errors=3, best_vias=166, best_length_mm=7384.0,
                           last_run=2, last_round=1, last_errors=5, last_vias=168, last_length_mm=7412.0,
                           last_vs_best="worse", last_decider="errors", last_delta=2.0)),
]


class ScoreJob(ProxyJob):
    def progress(self):
        d = json.loads(super().progress())
        n, lines, score = SCRIPT[min(self.polls, len(SCRIPT)) - 1]
        d.update(score, rounds_ended=n, round_lines=lines)
        return json.dumps(d)


class ScoreEngine(ProxyEngine):
    def route_async(self, m, o):
        j = super().route_async(m, o)
        if len(self.jobs) == 1:  # the first routing job only
            j.__class__ = ScoreJob
        return j


def test_run_flow_logs_every_round_once_and_shows_the_score(app, eprom, tmp_path, reports):
    eng = ScoreEngine(hold=4)
    ui = ScriptedUi(accept=False)
    _, _, out = do_run(eprom, tmp_path, ui=ui, cfg=CFG, engine=eng)
    log = (out.report_dir / "log.txt").read_text()
    for line in (L1, L2, L3):
        assert log.count(f"round ended: {line}") == 1, log[-3000:]
    assert log.index(L1) < log.index(L2) < log.index(L3)
    shown = [c[1][0] for c in ui.calls if c[0] == "progress" and c[1][0].last_round]
    assert shown, "the UI got the score"
    last = shown[-1]
    assert (last.last_run, last.last_round, last.last_errors) == (2, 1, 5)
    assert (last.last_vs_best, last.last_decider) == ("worse", "errors")
    assert (last.best_run, last.best_round, last.best_errors, last.best_vias) == (1, 1, 3, 166)
