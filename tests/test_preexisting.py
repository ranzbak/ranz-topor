"""Pre-existing attribution: baseline = DRC of the ripped board, matched by (type, item uuids)."""
from pathlib import Path

from harness.drc import DrcReport, Violation, classify, new_routing


def _v(typ, desc, *uuids):
    return Violation(type=typ, severity="error", description=desc,
                     items=[{"uuid": u} for u in uuids])


def _rep(*vs):
    return DrcReport(path=Path("x"), exit_code=5, seconds=0.0, violations=list(vs),
                     unconnected=[], by_type={})


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
