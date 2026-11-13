import json
import math
import random

import pytest


def test_version_matches_cargo(engine, root):
    cargo = (root / "engine/Cargo.toml").read_text()
    assert engine.version() == cargo.split('version = "')[1].split('"')[0]


def test_arc_center_known_case(engine):
    assert engine.arc_center_nm(10_000_000, 10_000_000, 12_000_000, 11_000_000, 14_000_000, 10_000_000) == (
        12_000_000,
        8_500_000,
    )


def test_arc_center_collinear_is_none(engine):
    assert engine.arc_center_nm(0, 0, 1_000, 0, 2_000, 0) is None


def _model():
    return {
        "units": "nm",
        "board": "t",
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": True}],
        "nets": [{"code": 2, "name": "N2"}],
        "pads": [
            {
                "uuid": "p", "ref": "U1", "number": "1", "net": 2, "attr": "smd", "center": [0, 0],
                "rotation_deg": 0.0, "layers": [0], "shapes": {"0": {"kind": "circle", "d": 1_600_000}},
                "drill": None, "local_clearance": None, "net_tie_group": None,
            }
        ],
        "rules": {"default": {"clearance": 200_000}},
    }


def _model_with_track():
    m = _model()
    m["pads"].append(
        {
            "uuid": "q", "ref": "U2", "number": "1", "net": 2, "attr": "smd", "center": [10_000_000, 0],
            "rotation_deg": 0.0, "layers": [0], "shapes": {"0": {"kind": "circle", "d": 1_600_000}},
            "drill": None, "local_clearance": None, "net_tie_group": None,
        }
    )
    m["kept"] = [
        {"uuid": "k1", "net": 2, "layer": 0, "width": 250_000, "locked": False,
         "prim": {"kind": "seg", "a": [0, 0], "b": [10_000_000, 0]}}
    ]
    return m


def test_check_items_reports_violation(engine):
    item = {"net": 1, "layer": 0, "width": 250_000, "prim": {"kind": "seg", "a": [0, 1_000_000], "b": [5_000_000, 1_000_000]}}
    rep = json.loads(engine.check_items(json.dumps(_model()), json.dumps([item]), "{}"))
    assert rep["checked"] == 1 and len(rep["violations"]) == 1 and rep["violations"][0]["required"] == 200_000  # margin_nm default 0 (5febf58)


def test_check_items_bad_json_raises(engine):
    with pytest.raises(ValueError):
        engine.check_items("{", "[]", "{}")


def test_fuzz_items_empty_model_truncates(engine):
    out = json.loads(engine.fuzz_items("{}", "{}", 1, 10))
    assert out["items"] == [] and out["truncated"] is True


def test_schema_compat_with_harness_export(engine, fixture_board):
    from harness.model import export_model, load_board

    rules = {
        "min_clearance": 0, "min_track_width": 200_000, "min_via_diameter": 400_000, "min_via_drill": 300_000,
        "min_annular": 50_000, "hole_clearance": 250_000, "hole_to_hole": 250_000, "edge_clearance": 500_000,
        "max_error": 5_000, "profile": "jlcpcb_1oz",
        "drill_list": [200_000, 250_000, 300_000, 400_000, 500_000, 800_000, 1_000_000],
        "routing_vias": [250_000, 300_000, 400_000, 500_000],
        "default": {"clearance": 200_000, "width": 200_000, "via_d": 600_000, "via_drill": 300_000,
                    "netclass": "Default", "width_source": "default"},
        "per_net": {}, "warnings": [],
    }
    model = export_model(load_board(fixture_board), rules, name="fx")
    rep = json.loads(engine.check_items(json.dumps(model), "[]", "{}"))
    assert rep["checked"] == 0 and rep["violations"] == []


def test_arc_center_matches_pcbnew(engine):
    pcbnew = pytest.importorskip("pcbnew")
    rng = random.Random(20261030)
    board = pcbnew.BOARD()
    for _ in range(200):
        r = rng.uniform(0.1, 50.0) * 1e6
        sweep = math.radians(rng.uniform(5, 350))
        a0 = rng.uniform(0, 2 * math.pi)
        cx, cy = rng.randint(-50_000_000, 50_000_000), rng.randint(-50_000_000, 50_000_000)
        pts = [
            (round(cx + r * math.cos(a0 + f * sweep)), round(cy + r * math.sin(a0 + f * sweep)))
            for f in (0, 0.5, 1)
        ]
        arc = pcbnew.PCB_ARC(board)
        s, m, e = (pcbnew.VECTOR2I(x, y) for x, y in pts)
        arc.SetStart(s)
        arc.SetMid(m)
        arc.SetEnd(e)
        c = arc.GetCenter()
        got = engine.arc_center_nm(*(v for p in pts for v in p))
        assert got is not None
        assert abs(got[0] - c.x) <= 100 and abs(got[1] - c.y) <= 100, (pts, got, (c.x, c.y))


def _model_routable():
    """Two connected through-hole pads on a two-layer board: the topological core (2f1430a) routes boards with two
    or more signal layers, so the one-layer `_model_with_track` fixture fails to route by design."""
    m = _model_with_track()
    m["kept"] = []
    m["layers"].append({"id": 1, "name": "B.Cu", "kind": "signal", "routable": True})
    for p in m["pads"]:
        p["layers"] = [0, 1]
        p["shapes"]["1"] = p["shapes"]["0"]
        p["attr"] = "pth"
        p["drill"] = {"w": 800_000, "h": 800_000}
    return m


def test_route_surface_round_trip(engine):
    m = _model_routable()
    m["kept"] = []
    m["layers"][0]["routable"] = True
    out = json.loads(engine.route(json.dumps(m), json.dumps({"wall_clock_s": 10.0})))
    for key in ("items", "connections", "nets", "runtime_s", "iterations", "warnings"):
        assert key in out
    assert out["threads_used"] >= 1
    for key in ("vias", "length_by_layer_nm", "sites"):
        assert key in out
    assert out["vias"] == 0 and out["sites"] >= 0
    assert sum(out["length_by_layer_nm"].values()) == out["total_length_nm"] > 0
    assert out["routed"] == 1 and len(out["items"]) == 1
    stats = json.loads(engine.wire_stats(json.dumps(_model_with_track())))
    assert stats["wires"] == 1 and stats["skipped"] == 0
    with pytest.raises(ValueError):
        engine.route("{", "{}")


def _rust_pub_fields(root, struct):
    import re

    src = (root / "engine/src/model.rs").read_text()
    body = src.split(f"pub struct {struct} {{")[1].split("\n}\n")[0]
    return re.findall(r"^\s+pub (\w+):", body, flags=re.M)


def test_default_options_lists_every_field(engine, root):
    d = json.loads(engine.default_options())
    top = _rust_pub_fields(root, "Options")
    assert top and set(top) == set(d), set(top) ^ set(d)
    assert d["wall_clock_s"] == 60.0 and d["detour_factor"] == 2.0 and d["margin_nm"] == 0
    assert d["steiner"] is False  # 4.2: the Steiner grid of the projection, default off
    # every key parses back as options (round trip)
    m = _model_routable()
    m["kept"] = []
    out = json.loads(engine.route(json.dumps(m), json.dumps({**d, "wall_clock_s": 10.0})))
    assert out["routed"] == 1 and out["stop_reason"] == "stable"  # M-B0: a headless run ends at "Routes stable"


def test_direct_engine_calls_have_the_overrun_cap_by_default(engine):
    """User 2026-11-13: a direct engine call (route / route_async: the harness's route_router, bench.py, the M4 gate,
    classic_ref.py) gets the soft limit's overrun cap by default: timeout_overrun_s = "auto" = wall_clock_s. 0 (what
    the plugin passes in the KiCad dialog) or "none" = no cap."""
    d = json.loads(engine.default_options())
    assert d["timeout_overrun_s"] == "auto"
    m = json.dumps(_model_routable())
    out = json.loads(engine.route(m, json.dumps({"wall_clock_s": 10.0})))
    lim = out["limit"]
    assert lim["cap_s"] == pytest.approx(10.0) and not lim["cap_hit"] and lim["cut"] == "none", lim
    for none in (0.0, "none"):
        lim = json.loads(engine.route(m, json.dumps({"wall_clock_s": 10.0, "timeout_overrun_s": none})))["limit"]
        assert lim["cap_s"] is None, (none, lim)
    lim = json.loads(engine.route(m, json.dumps({"wall_clock_s": 10.0, "timeout_overrun_s": 3.0})))["limit"]
    assert lim["cap_s"] == pytest.approx(3.0), lim
    with pytest.raises(ValueError):
        engine.route(m, json.dumps({"timeout_overrun_s": "soon"}))


def _strip(r):
    r = dict(r)
    r.pop("runtime_s")
    r.pop("warnings")  # the "plan metrics" line carries wall-time shares (layer_share_pct, loop_s) that differ per run
    return r


def test_route_async_roundtrip_fixture(engine):
    m = _model_routable()
    m["kept"] = []
    m["layers"][0]["routable"] = True
    mj, oj = json.dumps(m), json.dumps({"wall_clock_s": 10.0, "threads": 2})
    job = engine.route_async(mj, oj)
    out = json.loads(job.wait())
    assert job.done()
    assert _strip(out) == _strip(json.loads(engine.route(mj, oj)))
    assert job.wait() == job.wait()  # idempotent
    p = json.loads(job.progress())
    assert p["phase"] == "done" and p["routed"] == out["routed"] and p["total"] == out["connections_total"]
    with pytest.raises(ValueError):
        engine.route_async("{", "{}")


def test_route_async_time_limit_is_live(engine):
    """RouteJob.set_time_limit / time_limit: the job's wall-clock limit (seconds since it started) set from
    wall_clock_s at the start and changeable while it runs (the plugin's progress dialog); 0 = no limit."""
    m = _model_routable()
    m["kept"] = []
    m["layers"][0]["routable"] = True
    mj = json.dumps(m)
    job = engine.route_async(mj, json.dumps({"wall_clock_s": 10.0, "threads": 2}))
    assert job.time_limit() == pytest.approx(10.0)
    job.set_time_limit(300.0)
    assert job.time_limit() == pytest.approx(300.0)
    job.set_time_limit(0.0)
    assert job.time_limit() is None
    json.loads(job.wait())
    assert engine.route_async(mj, json.dumps({"wall_clock_s": 0.0, "threads": 2})).time_limit() is None


def _job_at_stage(engine, mj, opts, stage):
    """route_async with `opts`, polled until its progress stage names `stage` (fails if the job ends first)."""
    import time

    job = engine.route_async(mj, json.dumps(opts))
    t0 = time.monotonic()
    while stage not in json.loads(job.progress()).get("stage", ""):
        assert not job.done(), f"the job ended before the stage {stage!r}"
        assert time.monotonic() - t0 < 300, f"never reached the stage {stage!r}"
        time.sleep(0.01)
    return job


def test_route_async_cancel_in_the_initial_routing_is_nothing_to_draw(engine):
    """M2.TL (user 2026-11-13): Cancel during the initial routing (P2-P4) keeps no plan: the loud
    NOTHING_TO_DRAW error, no copper, never a quiet empty or half-drawn result (was PT1: cancel at 2 s)."""
    from tests.scaling.synth import synth_model

    mj = json.dumps(synth_model(400, 1, layers=2))
    job = _job_at_stage(engine, mj, {"wall_clock_s": 0.0, "runs": 1}, "topological search")
    job.cancel()
    out = json.loads(job.wait())
    assert out["stop_reason"] == "cancel" and out["timed_out"]
    assert out["warnings"][0].startswith("ERROR: nothing to draw yet"), out["warnings"][:3]
    assert out["routed"] == 0 and out["items"] == []


def test_route_async_cancel_in_the_topor_loop_returns_anytime_result(engine):
    """Cancel once the TopoR loop runs: the best kept variant is drawn, no panic (ruling C10; M2.TL). Legal copper only
    (allow_violations = false; by default the topo core writes copper it cannot draw legally on purpose, never drop)."""
    from tests.scaling.synth import synth_model

    mj = json.dumps(synth_model(400, 1, layers=2))
    job = _job_at_stage(engine, mj, {"wall_clock_s": 0.0, "allow_violations": False}, "TopoR loop on the topology")
    job.cancel()
    out = json.loads(job.wait())
    # the loop's pick is drawn to its end (not cut short, so timed_out may be false)
    assert out["stop_reason"] == "cancel"
    assert out["routed"] >= 1 and out["items"], out["warnings"][:3]
    assert not any(w.startswith("ERROR:") for w in out["warnings"]), out["warnings"][:3]
    chk = json.loads(engine.check_items(mj, json.dumps(out["items"]), json.dumps({"margin_nm": 0})))
    assert chk["violations"] == []
