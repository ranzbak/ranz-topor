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
    assert r["zero_slack"] == {"status": "no candidate", "wires": []}
    assert (len(rep["items"]), r["routed_items"], r["unchanged_items"], r["replaced"]) == (5, 5, 0, 0)
    assert _digest(rep["items"]) == "0924279a204e2888826be81a8512139ab08454a2221112541fb9086e3a3b1c2b"


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
