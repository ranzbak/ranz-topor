"""Pre-existing attribution: baseline = DRC of the ripped board, matched by (type, item uuids)."""
from pathlib import Path

from harness.drc import DrcReport, Violation, classify, new_routing, split_unconnected


def _v(typ, desc, *uuids):
    return Violation(type=typ, severity="error", description=desc,
                     items=[{"uuid": u} for u in uuids])


def _rep(*vs, unconnected=()):
    return DrcReport(path=Path("x"), exit_code=5, seconds=0.0, violations=list(vs),
                     unconnected=list(unconnected), by_type={})


def _u(a, b, da="pad", db="zone"):
    return {"type": "unconnected_items", "items": [{"uuid": a, "description": da},
                                                   {"uuid": b, "description": db}]}


def test_kept_item_violation_is_preexisting():
    base = _rep(_v("copper_edge_clearance", "Board edge (min 0.5 mm; actual 0.1 mm)", "k1", "edge"))
    routed = _rep(_v("copper_edge_clearance", "Board edge (min 0.5 mm; actual 0.1 mm)", "edge", "k1"))
    c = classify(routed, base, router_uuids={"r1"})
    assert c["routing_new"] == 0 and c["routing_preexisting"] == 1
    assert c["by_type_preexisting"] == {"copper_edge_clearance": 1}
    assert new_routing(routed, base, {"r1"}) == []


def test_router_item_never_preexisting():
    # same type and same pair would be baseline, but a router uuid is involved
    base = _rep(_v("clearance", "d", "k1", "k2"))
    routed = _rep(_v("clearance", "d", "k1", "k2"), _v("clearance", "d", "k1", "r1"))
    c = classify(routed, base, router_uuids={"r1"})
    assert c["routing_new"] == 1 and c["routing_preexisting"] == 1
    assert c["by_type_new"] == {"clearance": 1}
    # even if (impossibly) the baseline held the router uuid, it still counts
    c2 = classify(_rep(_v("clearance", "d", "r1", "k1")), _rep(_v("clearance", "d", "r1", "k1")), {"r1"})
    assert c2["routing_new"] == 1 and c2["routing_preexisting"] == 0


def test_description_change_does_not_flip():
    base = _rep(_v("clearance", "(min 0.2 mm; actual 0.1 mm)", "k1", "k2"))
    routed = _rep(_v("clearance", "(min 0.2 mm; actual 0.0999 mm)", "k2", "k1"))
    assert classify(routed, base, set())["routing_pre" "existing"] == 1
    assert classify(routed, base, set())["routing_new"] == 0


def test_no_baseline_all_new_and_other_types_untouched():
    routed = _rep(_v("clearance", "d", "k1", "k2"), _v("silk_overlap", "d", "a"))
    c = classify(routed, None)
    assert c["routing_new"] == 1 and c["routing_preexisting"] == 0 and c["other"] == 1


def test_unconnected_preexisting_iff_same_pair_in_original_baseline():
    orig = _rep(unconnected=[_u("p1", "p2"), _u("p3", "z")])
    routed = _rep(unconnected=[_u("z", "p3", "zone", "pad"), _u("p5", "z")])
    new, pre = split_unconnected(routed, orig)
    assert [u["items"][0]["uuid"] for u in new] == ["p5"]
    assert [u["items"][0]["uuid"] for u in pre] == ["z"]
    # the ripped baseline (router work list) never makes an unconnected item pre-existing
    assert classify(routed, orig)["incomplete"]["unconnected_preexisting"] == 0
    c = classify(routed, None, unconnected_baseline=orig)
    assert c["incomplete"] == {"track_dangling": 0, "via_dangling": 0, "unconnected_items": 2,
                               "unconnected_new": 1, "unconnected_preexisting": 1}
    assert c["unconnected_preexisting_items"] == [["zone", "pad"]]
    c0 = classify(routed, None)
    assert c0["incomplete"]["unconnected_new"] == 2 and c0["incomplete"]["unconnected_preexisting"] == 0
