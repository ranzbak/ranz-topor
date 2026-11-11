"""robust-cdt (user 2026-11-07): a board the engine cannot read stops right after it is read, before the
pre-flight DRC and the settings dialog (ENGINE_BOARD_UNREADABLE, ``topor.probe``); an engine error that fails
(nearly) every connection is the ENGINE_FAILED error, never a quiet "rejected" run that routed nothing.

Real engine for the probe on eprom; test doubles for the failing probe and the failing route (the real failure
is engine-tested in engine/tests/topo_cdt_noding.rs)."""
import json
import time

import pytest

pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")

from test_run_flow import (CFG, ExplodingEngine, ProxyEngine, app, do_run, eprom, reports,  # noqa: E402,F401
                           _report_root)
from topor import messages, probe as probe_mod  # noqa: E402
from topor import run as run_mod  # noqa: E402

PLACES = ["a side of copper graphic of footprint AE1 on In2.Cu [no net] (uuid 8defc2ce) from (-13.0343, "
          "-39.5648) mm to (-13.0204, -39.5457) mm",
          "a side of copper graphic of footprint AE1 on B.Cu [no net] (uuid 9988979c) from (-13.0450, -39.5857) mm "
          "to (-13.0342, -39.5646) mm"]
ERROR = ("projection failed: triangulation failed on degenerate crossing constraints: assertion failed: "
         "!previous_region.is_empty() || from == final_vertex")


class UnreadableEngine(ExplodingEngine):
    """The probe fails as on bornhack2026 before the noded path; routing must never start."""

    def __init__(self):
        self.probes = 0

    def probe_board(self, m, o):
        self.probes += 1
        json.loads(m), json.loads(o)
        return json.dumps({"ok": False, "error": ERROR, "places": PLACES, "notes": [], "path": "",
                           "vertices": 0, "obstacles": 0, "seconds": 0.01})


def test_unreadable_board_stops_before_the_drc_and_the_dialog(app, eprom, tmp_path, reports, monkeypatch):
    def no_drc(*a, **k):
        raise AssertionError("the pre-flight DRC must not run for a board the engine cannot read")

    monkeypatch.setattr(run_mod, "run_drc", no_drc)
    eng = UnreadableEngine()
    t0 = time.monotonic()
    board, ui, out = do_run(eprom, tmp_path, engine=eng, strict=False)
    assert time.monotonic() - t0 < 30
    assert eng.probes == 1
    assert out.status == "error" and board.commits == []
    assert "preflight" not in ui.names() and "done" not in ui.names() and "preview" not in ui.names()
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert len(errs) == 1
    e = errs[0]
    assert e.code == "ENGINE_BOARD_UNREADABLE" and e.severity == "error"
    assert "footprint AE1 on In2.Cu" in e.message and "(-13.0343, -39.5648) mm" in e.message
    assert "B.Cu" in e.message and ERROR in e.message
    assert list(e.items) == PLACES
    assert e.fix == messages.fix_for("ENGINE_BOARD_UNREADABLE") and "KiCad →" in e.fix
    log = (out.report_dir / "log.txt").read_text()
    assert "engine probe: FAILED" in log and "aborted: the engine could not read this board" in log
    res = json.loads((out.report_dir / "result.json").read_text())
    assert res["status"] == "error"
    assert any(f["code"] == "ENGINE_BOARD_UNREADABLE" for f in res["findings"])
    assert res["route"] == {} or res["route"] is None or "items" not in res["route"]
    # only the early "setup" progress (no DRC, no routing)
    assert {c[1][0].phase for c in ui.calls if c[0] == "progress"} <= {"setup"}


def test_probe_on_a_readable_board(eprom):
    opts = run_mod.build_options(CFG, topor_engine, model=eprom.model)
    polls = []
    pr = probe_mod.probe_board(topor_engine, eprom.model, opts, on_poll=lambda: polls.append(1), poll_s=0.01)
    assert pr["ok"] and pr["error"] is None and pr["places"] == []
    assert pr["path"] == "direct" and pr["vertices"] > 0
    assert pr["seconds"] < 10


def test_probe_skipped_for_an_engine_without_it(eprom):
    assert probe_mod.probe_board(ExplodingEngine(), eprom.model, {}) is None


def test_probe_exception_is_a_failed_probe():
    class Broken:
        def probe_board(self, m, o):
            raise ValueError("model json: missing field `pads`")

    pr = probe_mod.probe_board(Broken(), {}, {})
    assert pr["ok"] is False and "missing field" in pr["error"]
    f = probe_mod.unreadable_finding(pr)
    assert f.code == "ENGINE_BOARD_UNREADABLE" and "missing field" in f.message and f.items == ()


def _conn(i, status="failed", reason=ERROR):
    return {"id": i, "status": status, "reason": reason, "name": f"N{i}"}


def test_engine_failure_needs_most_connections_on_one_engine_reason():
    assert probe_mod.engine_failure(None) is None
    assert probe_mod.engine_failure({"connections": []}) is None
    f = probe_mod.engine_failure({"connections": [_conn(i) for i in range(352)]})
    assert f is not None and f.code == "ENGINE_FAILED" and f.severity == "error"
    assert "352 of 352" in f.message and ERROR in f.message
    # most: 6 of 10 on it
    mixed = [_conn(i) for i in range(6)] + [_conn(i, "routed", "") for i in range(6, 10)]
    assert probe_mod.engine_failure({"connections": mixed}) is not None
    # a few: a board problem, not an engine one
    few = [_conn(i) for i in range(3)] + [_conn(i, "routed", "") for i in range(3, 10)]
    assert probe_mod.engine_failure({"connections": few}) is None
    # ordinary failures (no room for a via, ...) never count
    board = [_conn(i, reason="via 1: no site") for i in range(10)]
    assert probe_mod.engine_failure({"connections": board}) is None


def _projection_failed(out_json):
    d = json.loads(out_json)
    conns = d.get("connections") or []
    for c in conns:
        c.update(status="failed", reason=ERROR + " at a side of copper graphic of footprint AE1")
    d.update(items=[], routed=0, connections=conns, vias=0, warnings=[ERROR] + list(d.get("warnings") or []))
    return json.dumps(d)


def test_engine_failed_run_is_an_error_not_a_quiet_reject(app, eprom, tmp_path, reports):
    board, ui, out = do_run(eprom, tmp_path, engine=ProxyEngine(patch=_projection_failed), strict=False)
    assert out.status == "error" and board.commits == []
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert len(errs) == 1 and errs[0].code == "ENGINE_FAILED" and errs[0].severity == "error"
    assert "projection failed" in errs[0].message and "not a normal run" in errs[0].message
    assert "done" not in ui.names() and "preview" not in ui.names()
    res = json.loads((out.report_dir / "result.json").read_text())
    assert res["status"] == "error" and any(f["code"] == "ENGINE_FAILED" for f in res["findings"])
    assert "Outcome: error" in (out.report_dir / "report.md").read_text()


def test_rules_read_wrong_is_a_loud_error_after_the_drc(app, eprom, tmp_path, reports):
    """Self-check (user 2026-11-11, the eprom case: the engine got 0.2 mm where KiCad requires 0.25 mm): the engine
    is given a smaller clearance than KiCad's rules, KiCad's DRC of the result finds the clearance violations, and the
    run stops with RULES_MISMATCH (nets and both values) before the preview; nothing is committed."""
    import copy

    from test_run_flow import make_board

    model = copy.deepcopy(eprom.model)
    rules = model["rules"]
    for r in [rules["default"], *rules["per_net"].values()]:
        r["clearance"] = 100_000  # KiCad's rules for eprom: 0.2 mm and more
    rules["min_clearance"] = 100_000
    board = make_board(eprom)
    board.model = model
    board, ui, out = do_run(eprom, tmp_path, board=board, strict=False)
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert out.status == "error" and board.commits == [], (out.status, errs)
    assert len(errs) == 1 and errs[0].code == "RULES_MISMATCH" and errs[0].severity == "error"
    assert "topor read the design rules wrong — please report this" in errs[0].message
    assert "topor routed with 0.1000 mm" in errs[0].message
    assert "preview" not in ui.names() and "done" not in ui.names()
    res = json.loads((out.report_dir / "result.json").read_text())
    codes = {f["code"] for f in res["findings"]}
    assert {"RULES_MISMATCH", "DRC_NEW_VIOLATIONS"} <= codes
