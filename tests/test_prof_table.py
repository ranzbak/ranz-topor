"""TW0: the per-stage table of tools/prof_table.py and tools/prof_compare.py on a synthetic profile (no engine, no board).

The table's rows must partition the run (shares sum to 100 %), the loop split must add up to the loop row, and the cycles and
re-lays per 600 s of loop time must come from the loop window, not from the whole run."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "tools"))
import prof_compare  # noqa: E402
import prof_table as pt  # noqa: E402


def _profile(loop_s=40.0, cycles=8):
    laps = {"topo_setup": 2.0, "st_sweep": 24.0, "st_uncross_reopt": 6.0, "st_viarose": 1.0, "st_float": 1.0, "st_cross": 3.0, "st_pairrelay": 0.5, "st_pairmove": 0.5}
    keys = {k: [cycles, v, 0] for k, v in laps.items()}
    keys["rr_viol_after"] = [320, 18.0, 0]
    keys["reroute"] = [320, 22.0, 0]
    st = lambda stage, wall, k=None: {"stage": stage, "labels": [stage], "wall_s": wall, "mono0": 0.0, "mono1": 0.0, "keys": k or {}}
    return {"version": 1, "engine_s": 0.0, "python": {"before_s": 10.0, "job_s": 100.0, "after_s": 20.0, "total_s": 130.0},
            "stages": [st("stubs", 5.0), st("projection", 1.0), st("p2_p4", 10.0), st("loop", loop_s, keys), st("drawing", 30.0), st("via_reduction", 10.0), st("tail", 3.0)],
            "cycles": [{"n": i, "wall_s": loop_s / cycles, "tried": 40, "kept": 7, "relaid": 2, "pair_moved": 0, "pair_relaid": 0, "archived": True} for i in range(cycles)]}


def test_rows_partition_the_run_and_the_loop_split_adds_up():
    b = pt.build(_profile())
    assert abs(sum(s for _, s in b["rows"]) - b["total_s"]) < 1e-9
    side = pt.sidecar(b)
    assert side["prof_sum_share_pct"] == 100.0
    assert abs(sum(s for _, s in b["loop_split"]) - b["loop_s"]) < 1e-9
    split = dict(b["loop_split"])
    assert split["judge (loop_judge=global only: whole-board layering + measure per re-lay; the local judge is in search)"] == 18.0
    assert split["search (sweep minus judge: price, place, keep)"] == 6.0  # sweep 24 - judge 18
    assert split["layering (uncross, layer re-optimisation, via-rose, floating up)"] == 8.0
    assert split["other (offer, archive, snapshot, gen 0, picking)"] == 2.0  # 40 - 38 in laps


def test_cycles_and_relays_per_600s_of_loop_time():
    b = pt.build(_profile(loop_s=40.0, cycles=8))
    assert b["cycles"] == 8 and b["relays"] == 320 and b["relays_kept"] == 56
    assert abs(b["cycles_per_600s"] - 120.0) < 1e-9  # 8 cycles / 40 s * 600
    assert abs(b["relays_per_600s"] - 4800.0) < 1e-9
    assert abs(b["relay_ms_mean"] - 68.75) < 1e-9  # 22 s / 320


def test_table_text_and_missing_profile(tmp_path):
    t = pt.render(pt.build(_profile()), "demo", [])
    assert "| TopoR loop |" in t and "sum of the rows** | 130.00 | 100.0 %" in t and "120.0 cycles" in t
    p = tmp_path / "x--metrics.json"
    p.write_text(json.dumps({"vias": 3}))
    try:
        pt.load(p)
    except SystemExit as e:
        assert "no `profile`" in str(e)
    else:
        raise AssertionError("a run without profile must stop with a message")


def test_compare_two_runs(tmp_path):
    for tag, loop_s in (("base", 40.0), ("tip", 10.0)):
        d = tmp_path / tag / "demo" / "run"
        d.mkdir(parents=True)
        (d / "demo--metrics.json").write_text(json.dumps({"profile": _profile(loop_s=loop_s)}))
    out = tmp_path / "c.json"
    assert prof_compare.main([str(tmp_path / "base"), str(tmp_path / "tip"), "--json", str(out)]) == 0
    j = json.loads(out.read_text())["demo"]
    assert j["base"]["cycles_per_600s"] == 120.0 and j["tip"]["cycles_per_600s"] == 480.0
