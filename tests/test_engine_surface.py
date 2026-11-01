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


def test_smooth_surface_round_trip(engine):
    out = json.loads(engine.smooth(json.dumps(_model_with_track()), json.dumps({"existing_copper": "smooth"})))
    for key in ("items", "replaced", "nets", "wires", "runtime_s", "threads_used"):
        assert key in out
    assert out["threads_used"] == 1
    assert out["wires_total"] == 1 and out["wires_taut"] == 1
    assert out["replaced"] == ["k1"] and len(out["items"]) == 1


def test_smooth_rejects_bad_json(engine):
    with pytest.raises(ValueError):
        engine.smooth("{", "{}")


def test_check_items_reports_violation(engine):
    item = {"net": 1, "layer": 0, "width": 250_000, "prim": {"kind": "seg", "a": [0, 1_000_000], "b": [5_000_000, 1_000_000]}}
    rep = json.loads(engine.check_items(json.dumps(_model()), json.dumps([item]), "{}"))
    assert rep["checked"] == 1 and len(rep["violations"]) == 1 and rep["violations"][0]["required"] == 205_000


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


def test_route_surface_round_trip(engine):
    m = _model_with_track()
    m["kept"] = []
    m["layers"][0]["routable"] = True
    out = json.loads(engine.route(json.dumps(m), json.dumps({"wall_clock_s": 10.0})))
    for key in ("items", "connections", "nets", "runtime_s", "iterations", "warnings"):
        assert key in out
    assert out["threads_used"] >= 1
    for key in ("vias", "length_by_layer_nm", "sites"):
        assert key in out
    assert out["vias"] == 0 and out["sites"] >= 0
    assert out["length_by_layer_nm"] == {"F.Cu": out["total_length_nm"]}
    assert out["routed"] == 1 and len(out["items"]) == 1
    stats = json.loads(engine.wire_stats(json.dumps(_model_with_track())))
    assert stats["wires"] == 1 and stats["skipped"] == 0
    with pytest.raises(ValueError):
        engine.route("{", "{}")


def test_route_threads_option(engine):
    m = _model_with_track()
    m["kept"] = []
    m["layers"][0]["routable"] = True
    out = json.loads(engine.route(json.dumps(m), json.dumps({"wall_clock_s": 10.0, "threads": 2})))
    assert out["threads_used"] == 2


def _rust_pub_fields(root, struct):
    import re

    src = (root / "engine/src/model.rs").read_text()
    body = src.split(f"pub struct {struct} {{")[1].split("\n}\n")[0]
    return re.findall(r"^\s+pub (\w+):", body, flags=re.M)


def test_default_options_lists_every_field(engine, root):
    d = json.loads(engine.default_options())
    top = _rust_pub_fields(root, "Options")
    assert top and set(top) == set(d), set(top) ^ set(d)
    expert = _rust_pub_fields(root, "Expert")
    assert expert and set(expert) == set(d["expert"]), set(expert) ^ set(d["expert"])
    assert d["pathfinder_iter_cap"] == 30 and d["patience"] == 5 and d["min_gain_pct"] == 1.0
    assert d["wall_clock_s"] == 60.0 and d["detour_factor"] == 2.0
    assert d["ripup_growth"] == 2.0 and d["ripup_max_per_pair"] == 8
    assert d["expert"] == {"hist_fail": 1.0, "hist_block": 0.5, "hist_overuse": 0.5, "present_overuse": 4.0,
                           "zero_slack_candidates": 4, "relax_iter_cap": 200,
                           "own_net_capacity": True, "wall_aware_capacity": True, "transit_pinch": True,
                           "steiner_min_angle_deg": 60.0, "steiner_len_slack": 1.1, "steiner_ghost_factor": 1.0}
    assert d["steiner"] is True
    assert d["zero_slack_failed"] is True
    # every key parses back as options (round trip)
    m = _model_with_track()
    m["kept"] = []
    out = json.loads(engine.route(json.dumps(m), json.dumps({**d, "wall_clock_s": 10.0})))
    assert out["routed"] == 1 and out["stop_reason"] == "converged"


def _strip(r):
    r = dict(r)
    r.pop("runtime_s")
    return r


def test_route_async_roundtrip_fixture(engine):
    m = _model_with_track()
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


def test_route_async_cancel_returns_anytime_result(engine):
    """Cancel after 2 s of a board that routes for >= 5 s: anytime result, no panic (ruling C10)."""
    import time

    from tests.scaling.synth import synth_model

    m = synth_model(400, 1, layers=2)  # ~12 s uncancelled (2 threads)
    mj = json.dumps(m)
    job = engine.route_async(mj, json.dumps({"wall_clock_s": 0.0, "threads": 2}))
    time.sleep(2.0)
    assert not job.done()
    job.cancel()
    out = json.loads(job.wait())
    assert out["stop_reason"] == "cancel" and out["timed_out"]
    assert out["routed"] >= 1
    chk = json.loads(engine.check_items(mj, json.dumps(out["items"]), json.dumps({"margin_nm": 0})))
    assert chk["violations"] == []
