"""Custom design rules (.kicad_dru): the condition evaluator, the mapping onto topor's rule model, and KiCad's
own answer (pcbnew's DRC engine through ``GetOwnClearance`` on probe items) for every clearance topor
resolves exactly."""
from __future__ import annotations

import pytest

from toporlib import druexpr as dx
from toporlib.custom_rules import (CustomRules, _match_copper, _walk_rules, apply_to_model, canonical_layers,
                                   courtyard_rings)
from toporlib.rules import NetClassVals, resolve_rules_from_project

L2 = canonical_layers(2)
L4 = canonical_layers(4)


def _t(net="N", nc="Default", layer="F.Cu", kind="track", geo=None):
    return dx.ItemCtx(kind, net=net, netclass=nc, layer=layer, geo=geo or {})


# ------------------------------------------------------------------ expressions
@pytest.mark.parametrize("cond,item,want", [
    ("A.NetClass == 'Power'", _t(nc="Power,Default"), True),
    ("A.NetClass == ('DDR_ADDR')", _t(nc="DDR_ADDR,Default"), True),  # Zynq SoM: parenthesised literal
    ("A.NetClass == 'power'", _t(nc="Power,Default"), True),          # case-insensitive
    ("A.NetClass == 'Power,Default'", _t(nc="Power,Default"), True),  # the full composite name
    ("A.NetClass != 'Power'", _t(nc="Power,Default"), False),
    ("A.NetName == '/USB_*'", _t(net="/USB_DP"), True),               # wildcard
    ("A.NetName == 'X' || A.NetName == 'N'", _t(), True),
    ("!(A.NetName == 'N')", _t(), False),
    ("A.hasNetclass('Pow*')", _t(nc="Power,Default"), True),
    ("A.Type == 'Track'", _t(), True),
    ("A.Type == 'Via'", _t(), False),
    ("A.Pad_Type == 'SMD'", _t(), False),       # undefined property: == is false
    ("A.Pad_Type != 'SMD'", _t(), False),       # ... and so is !=
    ("A.memberOfFootprint('U1')", _t(), False),  # a routed track belongs to no footprint
    ("A.intersectsCourtyard('U6')", _t(geo={("intersectscourtyard", "U6"): True}), True),
    ("A.intersectsCourtyard('U6')", _t(), False),
    ("A.NetName == 'N' && B.NetName == 'M'", _t(), None),  # depends on the other item
    ("A.Layer == 'F.Cu'", _t(), True),
    ("A.Layer == 'B.Cu'", _t(), False),
])
def test_condition_values(cond, item, want):
    assert dx._eval(dx.parse_condition(cond), item, None) is want


def test_commutative_match():
    ast = dx.parse_condition("A.NetName == 'N' && B.NetName == 'M'")
    assert dx.evaluate(ast, _t(net="M"), _t(net="N")) is True
    assert dx.evaluate(ast, _t(net="M"), _t(net="X")) is False


@pytest.mark.parametrize("cond", ["A.getField('x') == 'y'", "A.Position_X > 1mm", "A.fromTo('a','b')", "A.NetName ==",
                                  "L.Foo"])
def test_unsupported_conditions_raise(cond):
    with pytest.raises(dx.Unsupported):
        dx.parse_condition(cond)


# ------------------------------------------------------------------ resolution semantics
def _clr(text, net="N", nc="Default", base=200_000, layers=L2, **kw):
    cr = CustomRules(text, layers)
    return cr.net(net=net, netclass=nc, base_clearance=base, base_width=200_000, base_via=(600_000, 300_000),
                  diff_base=None, **kw), cr


def test_last_matching_rule_wins_also_when_smaller():
    cu, _ = _clr('(rule a (constraint clearance (min 0.3mm))) (rule b (condition "A.NetName == \'N\'") '
                 '(constraint clearance (min 0.1mm)))')
    assert cu.clearance == 100_000
    cu, _ = _clr('(rule b (condition "A.NetName == \'N\'") (constraint clearance (min 0.1mm))) '
                 '(rule a (constraint clearance (min 0.3mm)))')
    assert cu.clearance == 300_000


def test_pair_dependent_rule_is_a_precaution_only_when_stricter():
    tight, cr = _clr('(rule p (condition "A.NetName == \'N\' && B.NetName == \'M\'") (constraint clearance (min 0.4mm)))')
    assert tight.clearance == 400_000 and any("precaution" in n and "'p'" in n for n in cr.notes.items)
    loose, cr = _clr('(rule p (condition "A.NetName == \'N\' && B.NetName == \'M\'") (constraint clearance (min 0.1mm)))')
    assert loose.clearance is None and any("looser" in n and "'p'" in n for n in cr.notes.items)
    other, _ = _clr('(rule p (condition "A.NetName == \'N\' && B.NetName == \'M\'") (constraint clearance (min 0.4mm)))',
                    net="Q")
    assert other.clearance is None


def test_b_only_condition_reads_like_a():
    cu, _ = _clr('(rule p (condition "B.NetName == \'N\'") (constraint clearance (min 0.4mm)))', net="Q")
    assert cu.clearance is None  # not every net gets a precaution for a rule that names the other side only
    cu, _ = _clr('(rule p (condition "B.NetName == \'N\'") (constraint clearance (min 0.4mm)))')
    assert cu.clearance == 400_000


def test_layer_selector_and_per_layer_spread():
    cu, cr = _clr('(rule o (layer outer) (constraint clearance (min 0.3mm)))', layers=L4)
    assert cu.clearance == 300_000  # the router has one clearance per net: the largest
    assert any("differs between layers" in n for n in cr.notes.items)
    cu, _ = _clr('(rule i (layer inner) (constraint clearance (min 0.3mm)))', layers=L2)
    assert cu.clearance is None  # a 2-layer board has no inner layer


def test_ignored_severity_is_skipped():
    cu, _ = _clr('(rule i (constraint clearance (min 0.5mm)) (severity ignore))')
    assert cu.clearance is None


def test_width_opt_min_max_and_per_layer_choice():
    cu, _ = _clr('(rule w (condition "A.NetName == \'N\'") (constraint track_width (min 0.15mm) (opt 0.3mm)))')
    assert (cu.width, cu.min_width) == (300_000, 150_000)
    cu, cr = _clr('(rule w (layer inner) (constraint track_width (opt 0.135mm)))', layers=canonical_layers(8))
    assert cu.width == 135_000 and any("differs per layer" in n for n in cr.notes.items)  # 6 of 8 layers
    cu, _ = _clr('(rule w (layer outer) (constraint track_width (max 0.1mm)))', layers=L4)
    assert cu.width == 100_000  # a max on any layer bounds the one width (DRC-safe)


def test_via_size_rules():
    cu, _ = _clr('(rule v (condition "A.Type == \'Via\'") (constraint via_diameter (min 0.7mm)) (constraint hole_size (opt 0.35mm)))')
    assert (cu.via_d, cu.via_drill) == (700_000, 350_000)


def test_board_edge_clearance_rule_e2e_case():
    """eprom-edge: an unconditional edge_clearance replaces Board Setup's copper-to-edge value."""
    r = resolve_rules_from_project({}, '(version 1)\n(rule "board edge 1.5 mm"\n  (constraint edge_clearance (min 1.5mm)))',
                                   netclass_of={"": "Default", "A": "Default"}, classes={"Default": NetClassVals("Default", 200_000)},
                                   widths_before={})
    assert r["edge_clearance"] == 1_500_000
    assert [row["use"] for row in r["custom_rules"]] == ["board"]


def test_region_plan_and_bornhack_classification():
    dru = open_bornhack_dru()
    pro = {"board": {"design_settings": {"rules": {"min_clearance": 0.09}}}}
    r = resolve_rules_from_project(pro, dru, netclass_of={"": "Default", "A": "Default", "R": "BT-RF,Default"},
                                   classes={"Default": NetClassVals("Default", 127_000), "BT-RF": NetClassVals("BT-RF", 200_000)},
                                   widths_before={}, apply_fab_floors=False, copper_layer_count=4)
    assert r["custom_regions"] == [{"rule": "neck_down", "fn": "intersectscourtyard", "arg": "U6", "clearance": 100_000,
                                    "enclosed": False, "layers": []}]
    uses = {(row["rule"], row["constraint"]): row["use"] for row in r["custom_rules"]}
    assert uses[("neck_down", "clearance")] == "region"
    assert uses[("AE1 antenna internal clearance", "clearance")] == "no routed copper"
    assert uses[("AE3 antenna internal clearance", "clearance")] == "ignored"
    assert uses[("N4 silkscreen ignore", "edge_clearance")] == "ignored"
    assert r["per_net"]["2"]["clearance"] == 200_000  # BT-RF keeps its class value outside U6


def open_bornhack_dru() -> str:
    return """(version 1)
(rule "neck_down"
    (condition "A.intersectsCourtyard('U6')")
    (constraint clearance (min 0.1mm)))
(rule "AE3 antenna internal clearance"
  (condition "A.memberOfFootprint('AE3') && B.memberOfFootprint('AE3')")
  (constraint clearance (min -1mm)) (constraint hole_clearance (min 0mm)) (constraint physical_hole_clearance (min 0mm))
  (severity ignore))
(rule "AE1 antenna internal clearance"
  (condition "A.memberOfFootprint('AE1') && B.memberOfFootprint('AE1')")
  (constraint clearance (min -1mm)) (constraint physical_hole_clearance (min -1mm)) (constraint hole_clearance (min -1mm)))
(rule "N4 silkscreen ignore"
  (condition "A.memberOfFootprint('N4') || B.memberOfFootprint('N4')")
  (constraint silk_clearance (min -1mm)) (constraint edge_clearance (min -1mm)) (severity ignore))
"""


def test_fab_floor_raises_a_custom_width_with_one_note():
    """Zynq SoM ``(layer inner)`` DDR width opt 0.135 mm under the JLCPCB 0.2 mm floor: raised, one note naming the nets."""
    dru = '(rule w (layer inner) (condition "A.NetClass == \'DDR\'") (constraint track_width (opt 0.135mm)))'
    r = resolve_rules_from_project({}, dru, netclass_of={"": "Default", "A": "DDR", "B": "DDR", "C": "Default"},
                                   classes={"Default": NetClassVals("Default", 200_000), "DDR": NetClassVals("DDR", 200_000)},
                                   widths_before={}, copper_layer_count=6)
    assert r["per_net"]["1"]["width"] == r["min_track_width"] == 200_000
    notes = [w for w in r["warnings"] if w.startswith("floor: custom track width")]
    assert notes == ["floor: custom track width 0.135 -> 0.2 mm (fab minimum, [rules] fab_profile) [2 net(s): A, B]"], notes


def test_net_dependent_area_rule_is_no_region():
    r = resolve_rules_from_project({}, '(rule c (condition "A.intersectsCourtyard(\'U1\') && A.NetName == \'A\'") '
                                       '(constraint clearance (min 0.4mm)))',
                                   netclass_of={"": "Default", "A": "Default", "B": "Default"},
                                   classes={"Default": NetClassVals("Default", 200_000)}, widths_before={},
                                   apply_fab_floors=False)
    assert r["custom_regions"] == []
    assert r["per_net"]["1"]["clearance"] == 400_000 and r["per_net"]["2"]["clearance"] == 200_000
    assert any("depends on the net" in w for w in r["warnings"])


def test_pad_override_zynq_smd_rule():
    """Zynq SoM ``A.Pad_Type == 'SMD'`` 0.127 mm: SMD pads get it as a local clearance when stricter."""
    model = {"layers": [{"id": 0, "name": "F.Cu"}, {"id": 2, "name": "B.Cu"}], "nets": [{"code": 1, "name": "A"}],
             "pads": [{"ref": "U1", "net": 1, "attr": "smd", "layers": [0]}, {"ref": "J1", "net": 1, "attr": "pth", "layers": [0, 2]}],
             "keepouts": [],
             "rules": {"default": {"clearance": 90_000, "netclass": "Default"},
                       "per_net": {"1": {"clearance": 90_000, "netclass": "Default"}}, "warnings": []}}
    m = apply_to_model(model, "(rule pad_clearance (condition \"A.Pad_Type == 'SMD'\") (constraint clearance (min 0.127mm)))",
                       courtyards={})
    assert m["pads"][0].get("local_clearance") == 127_000 and m["pads"][1].get("local_clearance") is None
    assert any("raised on 1 pad" in w for w in m["rules"]["warnings"])


def test_apply_to_model_regions_from_courtyards():
    model = {"layers": [{"id": 0, "name": "F.Cu"}, {"id": 2, "name": "B.Cu"}], "nets": [], "pads": [], "keepouts": [],
             "rules": {"default": {"clearance": 200_000}, "per_net": {}, "warnings": [],
                       "custom_regions": [{"rule": "c", "fn": "intersectscourtyard", "arg": "U*", "clearance": 100_000,
                                           "enclosed": False, "layers": []}]}}
    sq = [[0, 0], [10, 0], [10, 10], [0, 10]]
    m = apply_to_model(model, "(rule c (condition \"A.intersectsCourtyard('U*')\") (constraint clearance (min 0.1mm)))",
                       courtyards={"U1": {"B": [sq]}, "J1": {"F": [sq]}})
    assert [(r["ref"], r["clearance"], r["poly"]) for r in m["rules"]["clearance_regions"]] == [("U*", 100_000, sq)]


def test_courtyard_rings_chain_segments():
    k = 1_000_000
    segs = [{"kind": "seg", "a": [0, 0], "b": [k, 0]}, {"kind": "seg", "a": [k, k], "b": [k, 0]},
            {"kind": "seg", "a": [k, k], "b": [0, k]}, {"kind": "seg", "a": [0, k], "b": [0, 0]}]
    assert courtyard_rings(segs) == [[[0, 0], [k, 0], [k, k], [0, k]]]
    assert courtyard_rings(segs[:3]) == []  # open chain


# ------------------------------------------------------------------ KiCad's own answer
ORACLE_DRU = """(version 1)
(rule sig_tight (condition "A.NetName == 'SIG'") (constraint clearance (min 0.31mm)))
(rule power_loose (condition "A.NetClass == 'Power'") (constraint clearance (min 0.12mm)))
(rule outer_gnd (layer outer) (condition "A.NetName == 'GND'") (constraint clearance (min 0.27mm)))
(rule inner_only (layer inner) (constraint clearance (min 0.5mm)))
(rule via_gnd (condition "A.Type == 'Via' && A.NetName == 'GND'") (constraint clearance (min 0.33mm)))
(rule wild (condition "A.NetName == 'V*'") (constraint clearance (min 0.14mm)))
(rule cy (condition "A.intersectsCourtyard('U1')") (constraint clearance (min 0.11mm)))
(rule ign (condition "A.NetName == 'GND' && A.Type == 'Track'") (constraint clearance (min 0.9mm)) (severity ignore))
"""


@pytest.fixture
def oracle_board(tmp_path):
    pcbnew = pytest.importorskip("pcbnew")
    from harness.copy import copy_project
    from harness.fixtures import make_fixture_board, write_project_file
    from harness.model import load_board

    pcb = make_fixture_board(tmp_path / "f.kicad_pcb")
    write_project_file(pcb, rules={}, classes=[
        {"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.6, "via_drill": 0.3,
         "priority": 2147483647},
        {"name": "Power", "clearance": 0.2, "track_width": 0.4, "via_diameter": 0.8, "via_drill": 0.4, "priority": 0}],
        patterns=[{"netclass": "Power", "pattern": "*VCC*"}])
    b = pcbnew.LoadBoard(str(pcb))
    fp = b.FindFootprintByReference("U1")
    sh = pcbnew.PCB_SHAPE(fp, pcbnew.SHAPE_T_RECT)
    sh.SetLayer(pcbnew.F_CrtYd)
    sh.SetStart(pcbnew.VECTOR2I(int(5e6), int(5e6)))
    sh.SetEnd(pcbnew.VECTOR2I(int(9e6), int(9e6)))
    sh.SetWidth(50_000)
    fp.Add(sh)
    b.Save(str(pcb))
    pcb.with_suffix(".kicad_dru").write_text(ORACLE_DRU)
    board = load_board(copy_project(pcb, tmp_path / "w"))
    for f in board.GetFootprints():
        f.BuildCourtyardCaches()
    return pcbnew, board


def test_clearances_match_kicad(oracle_board):
    """For each net (VCC: class Power, SIG, GND, no net), copper layer, track / via, inside the U1 courtyard and
    far from it: KiCad's ``GetOwnClearance`` (its DRC engine with the .kicad_dru) equals topor's rule walk,
    except where topor deliberately skips a ``(severity ignore)`` rule."""
    pcbnew, b = oracle_board
    cr = CustomRules(ORACLE_DRU, canonical_layers(2))
    base = {"VCC": 200_000, "SIG": 200_000, "GND": 200_000, "": 200_000}
    ncls = {"VCC": "Power,Default", "SIG": "Default", "GND": "Default", "": "Default"}
    inside, far = (7_000_000, 7_000_000), (40_000_000, 30_000_000)
    rows = []
    for net in ("VCC", "SIG", "GND"):
        for lname, lid in (("F.Cu", pcbnew.F_Cu), ("B.Cu", pcbnew.B_Cu)):
            for kind in ("track", "via"):
                for where, (x, y) in (("inside", inside), ("far", far)):
                    if kind == "track":
                        it = pcbnew.PCB_TRACK(b)
                        it.SetStart(pcbnew.VECTOR2I(x, y))
                        it.SetEnd(pcbnew.VECTOR2I(x + 100_000, y))
                        it.SetWidth(200_000)
                        it.SetLayer(lid)
                    else:
                        it = pcbnew.PCB_VIA(b)
                        it.SetPosition(pcbnew.VECTOR2I(x, y))
                        it.SetWidth(600_000)
                        it.SetDrill(300_000)
                    it.SetNet(b.FindNet(net))
                    b.Add(it)
                    kicad = int(it.GetOwnClearance(lid))
                    b.Remove(it)
                    geo = {("intersectscourtyard", "U1"): where == "inside"}
                    cr.rules[-2].region.add(("intersectscourtyard", "U1"))
                    ctx = dx.ItemCtx(kind, net=net, netclass=ncls[net], layer=lname, geo=geo)
                    ours = _walk_rules(cr.rules, "clearance", "min", base[net], [(ctx, lname, True)], _match_copper,
                                       None, net)
                    rows.append((net, lname, kind, where, kicad, ours))
    # the ignored GND track rule wins in KiCad's resolution (it only silences the report); topor skips it
    diffs = [r for r in rows if r[4] != r[5]]
    assert all(r[0] == "GND" and r[2] == "track" and r[4] == 900_000 for r in diffs), rows
    assert len(diffs) == 4, rows
    got = {(r[0], r[2], r[3]): r[5] for r in rows if r[1] == "F.Cu"}
    assert got[("SIG", "track", "far")] == 310_000 and got[("VCC", "track", "far")] == 140_000
    assert got[("GND", "via", "far")] == 330_000 and got[("SIG", "track", "inside")] == 110_000
