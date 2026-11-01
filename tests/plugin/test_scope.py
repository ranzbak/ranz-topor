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
    def seg(a, b):
        return {"kind": "seg", "a": list(a), "b": list(b)}

    kept = [{"uuid": "t1", "net": 1, "locked": False, "layer": 0, "width": 2, "prim": seg((0, 0), (5, 2))},
            {"uuid": "t2", "net": 1, "locked": True, "layer": 0, "width": 2, "prim": seg((10, 0), (5, 2))},
            {"uuid": "t3", "net": 2, "locked": False, "layer": 2, "width": 2, "prim": seg((0, 5), (5, 7))},
            {"uuid": "t4", "net": 3, "locked": False, "layer": 0, "width": 2, "prim": seg((0, 9), (20, 9))}]
    # v1 / v3 are routing vias (a same-net track ends on them), v2 is flagged free
    vias = [{"uuid": "v1", "net": 1, "locked": False, "free": False, "at": [5, 2], "d": 2, "layers": [0, 2]},
            {"uuid": "v2", "net": 1, "locked": False, "free": True, "at": [30, 30], "d": 2, "layers": [0, 2]},
            {"uuid": "v3", "net": 2, "locked": True, "free": False, "at": [5, 7], "d": 2, "layers": [0, 2]}]
    layers = [{"id": 0, "name": "F.Cu"}, {"id": 2, "name": "B.Cu"}]
    return {"nets": nets, "pads": pads, "kept": kept, "vias": vias, "layers": layers}


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


def test_rip_plan_keep_unconnected_vias():
    """Ruling C24: a via no same-net kept track end touches (within its radius, on a layer of its span) is a
    stitching via and is never ripped by default; flagged free vias are kept; routing vias are ripped;
    keep_unconnected_vias = false rips the untouched unflagged ones."""
    m = tiny()
    m["layers"] = [{"id": 0, "name": "F.Cu"}, {"id": 4, "name": "In1.Cu"}, {"id": 2, "name": "B.Cu"}]
    m["vias"] += [
        {"uuid": "stitch", "net": 3, "locked": False, "free": False, "at": [40, 40], "d": 2, "layers": [0, 2]},
        {"uuid": "near", "net": 3, "locked": False, "free": False, "at": [20, 11.5], "d": 4, "layers": [0, 2]},
        {"uuid": "othernet", "net": 3, "locked": False, "free": False, "at": [5, 2], "d": 2, "layers": [0, 2]},
        {"uuid": "blind", "net": 1, "locked": False, "free": False, "at": [0, 0], "d": 2, "layers": [2, 4]},
        {"uuid": "edge", "net": 3, "locked": False, "free": False, "at": [21, 9], "d": 2, "layers": [0, 2]}]
    # near: t4's end (20, 9) is 2.5 from it, radius 2 -> untouched; othernet: a net-1 end, not net 3;
    # blind: t1 starts at (0, 0) but on F.Cu, outside the In1-B.Cu span; edge: t4's end exactly on the rim
    whole = scope_from_selection(Selection(), m, CFG)
    rip = rip_plan(m, whole, cfg(existing_copper="rip"))
    assert set(rip.remove_uuids) == {"t1", "t3", "t4", "v1", "edge"}
    assert rip.kept_unconnected_vias == 4 and rip.kept_locked == 2
    off = rip_plan(m, whole, cfg(existing_copper="rip", keep_unconnected_vias=False))
    assert set(off.remove_uuids) == {"t1", "t3", "t4", "v1", "edge", "stitch", "near", "othernet", "blind"}
    assert "v2" not in off.remove_uuids and off.kept_unconnected_vias == 0  # KiCad's free flag always wins
    # a via without a position cannot be proven used: kept by default
    m["vias"].append({"uuid": "nopos", "net": 3, "locked": False, "free": False})
    assert "nopos" not in rip_plan(m, whole, cfg(existing_copper="rip")).remove_uuids


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


def test_dangling_drop_removes_only_the_dead_end_stub():
    """A14: search.dangling_ends = "drop" removes the dead-end arm (a stub drawn as two segments goes as a whole,
    up to the joint); fixed / continue remove nothing; do_not_touch and locked (respect_locked) stubs stay."""
    m = tiny()
    for p in m["pads"]:
        p["shapes"] = {"0": {"kind": "circle", "d": 2}}
    # net 3: t4 runs A3 (0, 9) .. C3 (20, 9); a two-segment arm s1 (10, 9) -> (12, 11), s2 (12, 11) -> (14, 11)
    m["kept"] += [
        {"uuid": "s1", "net": 3, "locked": False, "layer": 0, "width": 2,
         "prim": {"kind": "seg", "a": [10, 9], "b": [12, 11]}},
        {"uuid": "s2", "net": 3, "locked": False, "layer": 0, "width": 2,
         "prim": {"kind": "seg", "a": [12, 11], "b": [14, 11]}}]
    m["dangling_ends"] = [{"item": "s2", "net": 3, "layer": 0, "width": 2, "end": [14, 11], "direction": [1.0, 0.0]}]
    whole = scope_from_selection(Selection(), m, CFG)
    drop = CFG.with_values({("search", "dangling_ends"): "drop"})
    r = rip_plan(m, whole, drop)
    assert sorted(r.remove_uuids) == ["s1", "s2"] and r.engine_existing_copper == "fixed"
    for mode in ("continue", "fixed"):
        assert rip_plan(m, whole, CFG.with_values({("search", "dangling_ends"): mode})).remove_uuids == []
    locked = drop.with_values({("run", "respect_locked"): True})
    m2 = json.loads(json.dumps(m))
    m2["kept"][-1]["locked"] = True
    assert rip_plan(m2, whole, locked).remove_uuids == []
    dnt = drop.with_values({("nets", "do_not_touch"): ["GND"]})
    assert rip_plan(m, scope_from_selection(Selection(), m, dnt), dnt).remove_uuids == []
    # an arm that starts on a pad stops there; a lone free-ended track goes alone
    m3 = json.loads(json.dumps(m))
    m3["kept"][-2]["prim"] = {"kind": "seg", "a": [0, 9], "b": [12, 11]}  # s1 starts on pad A3
    assert sorted(rip_plan(m3, whole, drop).remove_uuids) == ["s1", "s2"]
    m3["kept"].pop(-2)
    assert rip_plan(m3, whole, drop).remove_uuids == ["s2"]
