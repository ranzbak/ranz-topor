"""The DRC re-route loop lives in toporlib.drcloop; run_bench output is unchanged by the extraction.

Expected values were captured with the PRE-refactor run_bench (ruling C6) on the synthetic fixture
board (rip none, threads 1, wall_clock_s 60). Only harness-level outputs are pinned, never
engine-internal router-report fields.
"""
import hashlib
import json

from harness import bench
from tests.test_smoother import _thin_router

# unconnected new/preexisting split is excluded: KiCad regenerates uuid-less pad uuids per load.
_SUMMARY_KEYS = ("routing_new", "routing_preexisting", "other", "by_type_new", "by_type_preexisting")


def _summary(r: dict) -> dict:
    d = r["routed_drc"]
    inc = d["incomplete"]
    return {**{k: d[k] for k in _SUMMARY_KEYS},
            "track_dangling": inc["track_dangling"], "via_dangling": inc["via_dangling"],
            "unconnected_items": inc["unconnected_items"]}


def _digest(items: list[dict]) -> str:
    return hashlib.sha256(
        json.dumps(sorted(json.dumps(i, sort_keys=True) for i in items)).encode()).hexdigest()


def _run(fixture_board, tmp_path, router, monkeypatch):
    from toporlib import drcloop

    calls = []
    real = drcloop.route_with_drc_loop

    def spy(*a, **k):
        calls.append(1)
        return real(*a, **k)

    monkeypatch.setattr(drcloop, "route_with_drc_loop", spy)
    r = bench.run_bench("fixture", tmp_path / "o", router=router, rip="none", threads=1,
                        options={"wall_clock_s": 60.0}, pcb_override=fixture_board.pcb)
    assert calls == [1], "run_bench must route through toporlib.drcloop.route_with_drc_loop"
    return r


def test_drcloop_matches_run_bench_on_fixture(fixture_board, tmp_path, monkeypatch, engine):
    from harness.router import route_router

    r = _run(fixture_board, tmp_path, route_router, monkeypatch)
    rep = r["router_report"]
    assert (rep["routed"], rep["connections_total"]) == (3, 3)
    assert _summary(r) == {
        "routing_new": 0, "routing_preexisting": 0, "other": 0, "by_type_new": {},
        "by_type_preexisting": {}, "track_dangling": 2, "via_dangling": 1, "unconnected_items": 1}
    assert r["drc_loop"] == {"rounds": 1, "pinned": {}, "spots": [], "budget_exhausted": False}
    assert r["zero_slack"] == {"status": "not run", "wires": []}
    # 8 items, not 7, since realise = "erbs" is the default (TW2): one wire ends in an octilinear tail, one more
    # segment and +0.14 mm; not a defect (lead, verify V1 2026-11-18). The digest below is that run's (tw2r2-b92ac74).
    assert (len(rep["items"]), r["routed_items"], r["unchanged_items"], r["replaced"]) == (8, 8, 0, 0)
    # golden digest carries the engine (IPC) net codes: GND 1, SIG 2, VCC 3 (pcbnew file order was VCC 1, SIG 2, GND 3)
    assert _digest(rep["items"]) == "d979f1f05ce96e854c8cf25b20b0d683747f19df5b2fc7698c3e3e52a88f4672"


def test_drcloop_pin_round_matches_run_bench_on_fixture(fixture_board, tmp_path, monkeypatch):
    """A router whose wire fails DRC: round 2 re-runs with the offending hand wire pinned."""
    r = _run(fixture_board, tmp_path, _thin_router, monkeypatch)
    assert _summary(r) == {
        "routing_new": 0, "routing_preexisting": 0, "other": 0, "by_type_new": {},
        "by_type_preexisting": {}, "track_dangling": 2, "via_dangling": 1, "unconnected_items": 4}
    dl = r["drc_loop"]
    assert (dl["rounds"], dl["spots"], dl["budget_exhausted"]) == (2, [], False)
    assert list(dl["pinned"].values()) == ["drc track_width at (40.000, 40.000) mm"]
    assert r["zero_slack"] == {"status": "not attempted", "wires": []}
    assert (r["routed_items"], r["unchanged_items"], r["replaced"]) == (0, 0, 0)


def test_wall_clock_zero_gives_the_drc_loop_no_budget_cut():
    """wall_clock_s 0 disables the engine deadline; the harness DRC loop must not read it as an
    exhausted budget (DG concern 1: a second DRC round never ran, routing_new stayed 1)."""
    assert bench._drc_budget_s({"wall_clock_s": 0}) == bench.NO_BUDGET_S
    assert bench._drc_budget_s({"wall_clock_s": 600}) == 600.0
    assert bench._drc_budget_s({}) == float(bench.ENGINE_WALL_CLOCK_S)


def test_router_remove_uuids_reach_drc_fn_and_the_loop_result():
    """exist-term r2: hand tracks the engine's tidy merged into a new collinear segment come back as
    the router's "remove_uuids"; the loop hands them to drc_fn (the board judged without them) and
    returns them, so the commit removes them in the same undo step (an empty list: plain drc_fn call)."""
    from pathlib import Path

    from toporlib.drc import DrcReport
    from toporlib.drcloop import DrcJudgement, route_with_drc_loop

    empty = DrcReport(Path("x"), 0, 0.0, [], [], {})
    for rem in (["k1", "k2"], []):
        seen = []

        def drc_fn(items, **kw):
            seen.append(kw)
            return DrcJudgement(empty, {})

        loop = route_with_drc_loop({"nets": []}, {}, route_fn=lambda m, o: {"items": [], "remove_uuids": rem, "report": None},
                                   drc_fn=drc_fn, base_drc=empty, orig_drc=None, budget_s=60.0, loop_cap=0)
        assert loop.remove_uuids == rem
        assert seen == ([{"remove_uuids": rem}] if rem else [{}])
