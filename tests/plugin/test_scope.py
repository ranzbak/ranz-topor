"""B7: routing scope from the selection, copper / lock policies (46 row 14, 22, 8b).

Pure tests use a tiny synthetic model; the eprom tests use the harness oracle (pcbnew, tests only) and the real
engine to prove the routed result of a ``between`` selection holds only U1<->U2 connections.
"""
import json

import pytest

from topor.config import Config
from topor.ports import PadRef, Selection
from topor.scope import RipPlan, connections_by_scope, rip_plan, scope_from_selection

CFG = Config.defaults()


def _pad(ref, num, net, x, y):
    return {"uuid": f"{ref}.{num}", "ref": ref, "number": num, "net": net, "center": {"x": x, "y": y}}


def tiny():
    """A(1,2) B(1,2) C(1): net1 = A1 B1 C1, net2 = A2 B2, net3 = A3 C3 (C3 only with A), net4 = lone pad."""
    nets = [{"code": i, "name": n} for i, n in ((1, "N1"), (2, "N2"), (3, "GND"), (4, "LONE"))]
    pads = [_pad("A", "1", 1, 0, 0), _pad("B", "1", 1, 10, 0), _pad("C", "1", 1, 20, 0),
            _pad("A", "2", 2, 0, 5), _pad("B", "2", 2, 10, 5),
            _pad("A", "3", 3, 0, 9), _pad("C", "3", 3, 20, 9), _pad("A", "4", 4, 0, 12)]
    kept = [{"uuid": "t1", "net": 1, "locked": False}, {"uuid": "t2", "net": 1, "locked": True},
            {"uuid": "t3", "net": 2, "locked": False}, {"uuid": "t4", "net": 3, "locked": False}]
    vias = [{"uuid": "v1", "net": 1, "locked": False, "free": False},
            {"uuid": "v2", "net": 1, "locked": False, "free": True},
            {"uuid": "v3", "net": 2, "locked": True, "free": False}]
    return {"nets": nets, "pads": pads, "kept": kept, "vias": vias}


def cfg(**kw):
    return CFG.with_values({("run", k): v for k, v in kw.items()})


def test_empty_selection_is_whole_board():
    plan = scope_from_selection(Selection(), tiny(), CFG)
    assert plan.mode == "whole" and plan.engine_scope["pads"] == [] and plan.engine_scope["nets"] == []
    assert plan.connections == 2 + 1 + 1  # N1: 2, N2: 1, GND: 1
    assert plan.nets == {"N1", "N2", "GND", "LONE"}
    sel = Selection(footprints=["A", "B"])
    plan = scope_from_selection(sel, tiny(), CFG, whole=True)  # dialog: "whole board"
    assert plan.mode == "whole" and plan.engine_scope["pads"] == []


def test_scope_between_vs_touching_tiny():
    sel = Selection(footprints=["A", "B"])
    b = scope_from_selection(sel, tiny(), cfg(scope_mode="between"))
    assert b.mode == "between"
    assert b.engine_scope["pads"] == ["A.1", "A.2", "B.1", "B.2"]  # A3/A4 have no selected partner
    assert b.connections == 2 and b.nets == {"N1", "N2"}
    t = scope_from_selection(sel, tiny(), cfg(scope_mode="touching"))
    assert set(t.engine_scope["pads"]) == {"A.1", "A.2", "B.1", "B.2", "C.1", "A.3", "C.3"}
    assert t.connections == t.counts["touching"]
    assert connections_by_scope(sel, tiny(), CFG) == {"between": 2, "touching": t.counts["touching"], "whole": 4}
    assert t.counts["touching"] >= 3  # N1 B-C edge, N2, GND A-C


def test_scope_pads_two_pins():
    sel = Selection(pads=[PadRef("A", "1"), PadRef("C", "1")])
    p = scope_from_selection(sel, tiny(), cfg(scope_mode="between"))
    assert p.engine_scope["pads"] == ["A.1", "C.1"] and p.connections == 1
    one = scope_from_selection(Selection(pads=[PadRef("A", "2")]), tiny(), cfg(scope_mode="between"))
    assert one.connections == 0 and [f.code for f in one.findings] == ["SCOPE_EMPTY"]
    assert one.findings[0].fix and one.findings[0].severity == "warning"
    touch = scope_from_selection(Selection(pads=[PadRef("A", "2")]), tiny(), cfg(scope_mode="touching"))
    assert touch.engine_scope["pads"] == ["A.2", "B.2"] and touch.connections == 1 and not touch.findings


def test_scope_area_polygon():
    area = [(-1, -1), (11, -1), (11, 6), (-1, 6)]  # A1 A2 B1 B2
    sel = Selection(area=area)
    p = scope_from_selection(sel, tiny(), cfg(scope_mode="between"))
    assert p.engine_scope["pads"] == ["A.1", "A.2", "B.1", "B.2"] and p.engine_scope["area"] is None
    t = scope_from_selection(sel, tiny(), cfg(scope_mode="touching"))
    assert t.engine_scope["area"] == [{"x": x, "y": y} for x, y in area]
    assert "C.1" in t.engine_scope["pads"]


def test_scope_tracks_select_nets():
    sel = Selection(tracks=["t3"], vias=["v1"])
    p = scope_from_selection(sel, tiny(), CFG)
    assert p.engine_scope["nets"] == ["N1", "N2"] and p.nets == {"N1", "N2"}


def test_scope_unrouted_only_keeps_copper():
    c = cfg(unrouted_only=True, existing_copper="rip")
    p = scope_from_selection(Selection(footprints=["A", "B"]), tiny(), c)
    assert p.engine_scope["unrouted_only"] is True
    assert rip_plan(tiny(), p, c).remove_uuids == []
    assert scope_from_selection(Selection(), tiny(), c).engine_scope["unrouted_only"] is True


def test_do_not_touch_and_ignore_pads():
    c = CFG.with_values({("nets", "do_not_touch"): ["GN*"], ("nets", "ignore_pads"): ["B.2"]})
    m = tiny()
    m["nets"][2]["name"] = "GND"
    p = scope_from_selection(Selection(), m, c)
    assert "GND" not in p.nets and p.engine_scope["nets"] == ["LONE", "N1", "N2"]
    p = scope_from_selection(Selection(footprints=["A", "B"]), m, c)
    assert p.engine_scope["pads"] == ["A.1", "B.1"]  # B.2 ignored: N2 has one selected pad left


def test_rip_plan_policies():
    m = tiny()
    whole = scope_from_selection(Selection(), m, CFG)
    fixed = rip_plan(m, whole, cfg(existing_copper="fixed"))
    assert fixed == RipPlan([], "fixed")
    sm = rip_plan(m, whole, cfg(existing_copper="smooth"))
    assert sm.remove_uuids == [] and sm.engine_existing_copper == "smooth"
    rip = rip_plan(m, whole, cfg(existing_copper="rip"))
    assert rip.engine_existing_copper == "fixed"
    assert set(rip.remove_uuids) == {"t1", "t3", "t4", "v1"}  # locked t2 / v3 kept, free via v2 kept
    assert rip.kept_locked == 2
    rl = rip_plan(m, whole, cfg(existing_copper="rip", respect_locked=False))
    assert set(rl.remove_uuids) == {"t1", "t2", "t3", "t4", "v1", "v3"}
    # scope: only the nets of the selection
    sel = scope_from_selection(Selection(footprints=["A", "B"]), m, cfg(scope_mode="between"))
    assert set(rip_plan(m, sel, cfg(existing_copper="rip")).remove_uuids) == {"t1", "t3", "v1"}
    # do_not_touch: never ripped
    dnt = CFG.with_values({("run", "existing_copper"): "rip", ("nets", "do_not_touch"): ["N2"]})
    r = rip_plan(m, scope_from_selection(Selection(), m, dnt), dnt)
    assert set(r.remove_uuids) == {"t1", "t4", "v1"} and r.kept_do_not_touch == 2


# ------------------------------------------------------------------ eprom oracle + real engine
@pytest.fixture(scope="module")
def eprom(tmp_path_factory):
    pytest.importorskip("pcbnew")
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.paths import board_pcb
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("proj")
    cp = copy_project(board_pcb("eprom-to-kernal"), d / "eprom-to-kernal")
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    return export_model(b, rules, name="eprom-to-kernal", dru_text=dru)


def _routed(model, opts_extra):
    eng = pytest.importorskip("topor_engine")
    opts = json.loads(eng.default_options())
    opts.update({"wall_clock_s": 60.0, **opts_extra})
    return json.loads(eng.route(json.dumps(model), json.dumps(opts)))


def test_scope_between_vs_touching(eprom):
    names = {n["code"]: n["name"] for n in eprom["nets"]}
    refs_of = {}
    for p in eprom["pads"]:
        if p["net"]:
            refs_of.setdefault(p["net"], []).append(p["ref"])
    sel = Selection(footprints=["U1", "U2"])
    b = scope_from_selection(sel, eprom, cfg(scope_mode="between"))
    t = scope_from_selection(sel, eprom, cfg(scope_mode="touching"))
    assert 0 < b.connections < t.connections <= b.counts["whole"]
    assert connections_by_scope(sel, eprom, CFG)["between"] == b.connections
    # only nets with >= 2 pads on U1 / U2
    want = {names[c] for c, r in refs_of.items() if sum(x in ("U1", "U2") for x in r) >= 2}
    assert b.nets == want
    assert {p.split(".")[0] for p in b.engine_scope["pads"]} == {"U1", "U2"}
    assert any(p.startswith("J1.") for p in t.engine_scope["pads"])

    # real engine, existing copper removed so every selected connection has to be routed
    bare = {**eprom, "kept": [], "vias": [], "dangling_ends": []}
    res = _routed(bare, {"scope": b.engine_scope, "partial_routes": True})
    routed_nets = {names[it["net"]] for it in res["items"] if it.get("net") in names}
    assert routed_nets and routed_nets <= b.nets
    assert 0 < res["connections_total"] <= b.connections  # estimate: pours / ties may join pads
    for it in res["items"]:
        assert it.get("net") in names
    res_t = _routed(bare, {"scope": t.engine_scope, "partial_routes": True})
    assert res_t["connections_total"] > res["connections_total"]
