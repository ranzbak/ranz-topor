import json
from pathlib import Path

import pytest

from harness.copy import assert_unchanged, copy_project
from harness.fixtures import make_fixture_board, write_project_file
from harness.paths import BENCH_BOARDS
from harness.model import export_model, load_board
from harness.rip import NetWidths, record_widths, rip_tracks
from harness.rules import JLCPCB_1OZ, match_condition, parse_dru, resolve_rules


CLASSES = [
    {"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.6, "via_drill": 0.3,
     "priority": 2147483647},
    {"name": "Power", "clearance": 0.2, "track_width": 0.4, "via_diameter": 0.8, "via_drill": 0.4, "priority": 0},
]


def _board(tmp_path, rules=None, classes=CLASSES, patterns=({"netclass": "Power", "pattern": "*VCC*"},)):
    pcb = make_fixture_board(tmp_path / "f.kicad_pcb")
    write_project_file(pcb, rules=rules or {}, classes=list(classes), patterns=list(patterns))
    return load_board(copy_project(pcb, tmp_path / "w"))


def _code(b, name):
    return str(int(b.GetNetsByName()[name].GetNetCode()))


def test_profile_values():
    assert JLCPCB_1OZ["width"] == 200_000 and JLCPCB_1OZ["clearance"] == 150_000 and JLCPCB_1OZ["via_d"] == 600_000 and JLCPCB_1OZ["via_drill"] == 300_000
    # 2026-11-12: no fab-profile via / drill list any more (the engine holds no profile; vias = the board's own)
    assert "drill_list" not in JLCPCB_1OZ and "routing_vias" not in JLCPCB_1OZ
    assert JLCPCB_1OZ["edge"] == 200_000 and JLCPCB_1OZ["pth_hole_to_track"] == 350_000 and JLCPCB_1OZ["npth_to_track"] == 250_000
    for k in ("via_d_min_auto", "via_drill_min_auto", "via_hole_to_track", "hole_to_hole_via", "hole_to_hole_pad",
              "vcut", "annular_2l", "annular_4l"):
        assert JLCPCB_1OZ[k] > 0


def test_parse_dru_bornhack_style():
    rs = parse_dru('(version 1)\n(rule "neck_down" (condition "A.intersectsCourtyard(\'U6\')") (constraint clearance (min 0.1mm)))\n(rule "ant" (condition "A.memberOfFootprint(\'AE3\') && B.memberOfFootprint(\'AE3\')") (constraint clearance (min -1mm)) (severity ignore))')
    assert rs[0].constraints == {"clearance": {"min": 100_000}} and rs[1].severity == "ignore"
    assert match_condition(rs[0].condition, net_name="X", netclass="Default", ref=None) is None
    assert match_condition("A.NetClass == 'Power'", net_name="VCC", netclass="Power,Default", ref=None) is True


def test_parse_dru_units_and_conditions():
    rs = parse_dru('(rule a (constraint clearance (min 10mil)) (constraint track_width (opt 100um) (max 0.0039in)))')
    assert rs[0].constraints == {"clearance": {"min": 254_000}, "track_width": {"opt": 100_000, "max": 99_060}}
    f = lambda c, **k: match_condition(c, **{"net_name": "SIG", "netclass": "Default", "ref": None, **k})
    assert f(None) is True
    assert f("A.NetName == 'SIG'") is True and f("B.NetName == 'X'") is False
    assert f("A.hasNetclass('Default') && A.Type == 'Track'") is True
    assert f("A.NetName == 'X' && A.intersectsCourtyard('U1')") is False  # one false term decides
    assert f("A.memberOfFootprint('R1')", ref="R1") is True and f("A.memberOfFootprint('R1')") is None
    assert f("A.NetName == 'SIG' || A.NetName == 'X'") is None


def test_rules_fixture_with_project(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={})
    vcc, sig = _code(b, "VCC"), _code(b, "SIG")
    # the board's own values as is (2026-11-12): hole clearance 0.25 mm, no longer raised to the 0.35 PTH floor
    assert r["edge_clearance"] == 500_000 and r["min_track_width"] == 200_000 and r["hole_clearance"] == 250_000
    assert r["per_net"][vcc]["width"] == 400_000 and r["per_net"][vcc]["width_source"] == "netclass"
    assert r["per_net"][sig]["width"] == 250_000 and r["per_net"][sig]["width_source"] == "default"
    assert r["default"]["clearance"] == 200_000 and r["profile"] == "board"
    assert r["fallbacks"] == [] and r["floors_applied"] == []  # the fixture's .kicad_pro sets every constraint
    assert "routing_vias" not in r and "drill_list" not in r


def test_width_from_existing_copper_beats_default(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={"SIG": NetWidths(400_000, 400_000, 400_000, 3)})
    sig = _code(b, "SIG")
    assert r["per_net"][sig]["width"] == 400_000 and r["per_net"][sig]["width_source"] == "existing_copper"


def test_settings_table_between_copper_and_default(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={}, settings_table={"SIG": 300_000})
    assert r["per_net"][_code(b, "SIG")]["width_source"] == "settings_table"
    assert r["per_net"][_code(b, "SIG")]["width"] == 300_000


def test_custom_rule_width_beats_everything(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(
        b,
        dru_text='(rule w (condition "A.NetName == \'SIG\'") (constraint track_width (opt 0.3mm)))',
        widths_before={"SIG": NetWidths(400_000, 400_000, 400_000, 3)},
    )
    sig = _code(b, "SIG")
    assert r["per_net"][sig]["width"] == 300_000 and r["per_net"][sig]["width_source"] == "custom_rule"
    assert r["per_net"][_code(b, "GND")]["width_source"] == "default"


def test_custom_rule_track_width_min_is_the_net_min_width(tmp_path):
    """A6/NX: a custom `track_width (min …)` is the floor a necked exit stub may not go below
    (engine NetRule.min_width); nets without such a rule carry 0."""
    b = _board(tmp_path)
    r = resolve_rules(
        b,
        dru_text='(rule w (condition "A.NetName == \'SIG\'") (constraint track_width (min 0.25mm) (opt 0.3mm)))',
        widths_before={},
    )
    sig = _code(b, "SIG")
    assert r["per_net"][sig]["width"] == 300_000 and r["per_net"][sig]["min_width"] == 250_000
    assert r["per_net"][_code(b, "GND")]["min_width"] == 0
    assert r["default"]["min_width"] == 0


def test_unknown_condition_is_strictest_with_warning(tmp_path):
    """A condition topor cannot read: its clearance applies where stricter (every net), with a warning that
    names the rule."""
    b = _board(tmp_path)
    r = resolve_rules(
        b,
        dru_text='(rule c (condition "A.getField(\'x\') == \'y\'") (constraint clearance (min 0.35mm)))',
        widths_before={},
    )
    assert r["default"]["clearance"] == 350_000
    assert all(v["clearance"] == 350_000 for v in r["per_net"].values())
    assert any("custom rule 'c'" in w and "not understood" in w for w in r["warnings"])


def test_courtyard_rule_is_a_region_not_a_net_value(tmp_path):
    """A courtyard-only clearance rule leaves the per-net clearances alone and becomes a region plan."""
    b = _board(tmp_path)
    r = resolve_rules(
        b,
        dru_text='(rule c (condition "A.intersectsCourtyard(\'U1\')") (constraint clearance (min 0.35mm)))',
        widths_before={},
    )
    assert r["default"]["clearance"] == 200_000
    assert r["custom_regions"] == [{"rule": "c", "fn": "intersectscourtyard", "arg": "U1", "clearance": 350_000,
                                    "enclosed": False, "layers": []}]


def test_ignored_rule_has_no_effect(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(
        b,
        dru_text='(rule c (condition "A.intersectsCourtyard(\'U1\')") (constraint clearance (min 0.35mm)) (severity ignore))',
        widths_before={},
    )
    assert r["default"]["clearance"] == 200_000 and not any("unparsed" in w for w in r["warnings"])


def test_track_angle_rule_is_refused(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text='(rule ang (constraint track_angle (min 45deg)))', widths_before={})
    assert any(w.startswith("REFUSE") and "ang" in w for w in r["warnings"])
    r = resolve_rules(b, dru_text='(rule seg (constraint track_segment_length (min 1mm)))', widths_before={})
    assert any(w.startswith("REFUSE") for w in r["warnings"])


def test_power_net_on_default_width_warns(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={})
    assert "POWER?: GND uses Default width" in r["warnings"]
    assert not any("VCC" in w or "SIG" in w for w in r["warnings"])


def test_pad_local_clearance_override_wins(tmp_path, engine):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={})
    codes = {n["name"]: n["code"] for n in export_model(b, r, name="x")["nets"]}
    # pad 3 (SIG, roundrect 1x2 mm at 30,20): trace on VCC running 0.25 mm off its right edge
    item = {"net": codes["VCC"], "layer": 0, "width": 250_000,
            "prim": {"kind": "seg", "a": [30_875_000, 19_500_000], "b": [30_875_000, 20_500_000]}}

    def violations(model):
        return json.loads(engine.check_items(json.dumps(model), json.dumps([item]), "{}"))["violations"]

    base = export_model(b, r, name="x")
    before = len(violations(base))
    pad3 = next(p for fp in b.GetFootprints() for p in fp.Pads() if p.GetNumber() == "3")
    pad3.SetLocalClearance(300_000)
    m = export_model(b, r, name="x")
    assert next(p for p in m["pads"] if p["number"] == "3")["local_clearance"] == 300_000
    assert len(violations(m)) == before + 1  # netclass 0.2 + 5 um margin passes; local 0.3 mm does not


def test_zero_minimums_are_board_values(tmp_path):
    """A 0 the board sets is KiCad's "no minimum" and is used as is (2026-11-12; before: filled from the profile)."""
    b = _board(tmp_path, rules={"min_clearance": 0, "min_via_diameter": 0})
    r = resolve_rules(b, dru_text=None, widths_before={})
    assert r["min_clearance"] == 0 and r["min_via_diameter"] == 0 and r["fallbacks"] == []
    # the opt-in floor mode raises them (and lists them)
    f = resolve_rules(b, dru_text=None, widths_before={}, fab_profile="jlcpcb_1oz")
    assert f["min_clearance"] == 150_000 and f["min_via_diameter"] == 450_000
    assert {"min_clearance", "min_via_diameter"} <= {x["name"] for x in f["floors_applied"]}


def test_missing_keys_take_the_reported_fallback(tmp_path):
    """Only a key the .kicad_pro does not carry takes RULE_FALLBACK (JLCPCB 1 oz, raised to KiCad's own value
    for the missing key), one ``fallbacks`` row and one ``rule fallback:`` warning per field naming the KiCad
    setting; the SWIG front end sees the same absence as the plugin."""
    import json as _json

    from toporlib.rules import RULE_FALLBACK, VIA_HOLE_FALLBACK, fallback_text

    b = _board(tmp_path)
    pro_p = tmp_path / "w" / "f.kicad_pro"
    doc = _json.loads(pro_p.read_text())
    for k in ("min_clearance", "min_hole_clearance", "min_copper_edge_clearance", "min_connection"):
        doc["board"]["design_settings"]["rules"].pop(k)
    pro_p.write_text(_json.dumps(doc))
    r = resolve_rules(b, dru_text=None, widths_before={})
    assert [x["field"] for x in r["fallbacks"]] == ["min_clearance", "min_hole_clearance",
                                                    "min_copper_edge_clearance", "min_connection"]
    assert r["min_clearance"] == RULE_FALLBACK["min_clearance"]["value"] == 150_000  # JLCPCB (KiCad: 0)
    assert r["hole_clearance"] == 350_000 and r["via_hole_clearance"] == VIA_HOLE_FALLBACK == 250_000
    assert r["edge_clearance"] == 500_000  # KiCad's own 0.5 mm, stricter than JLCPCB's 0.2 mm
    assert r["min_connection"] == 0
    for row in r["fallbacks"]:
        assert fallback_text(row) in r["warnings"] and "KiCad → File → Board Setup" in fallback_text(row)
    # the keys it carries are used as is (min_track_width 0.2, hole to hole 0.25)
    assert r["min_track_width"] == 200_000 and r["hole_to_hole"] == 250_000


def test_output_parses_with_engine_rules(tmp_path, engine):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={})
    rep = json.loads(engine.check_items(json.dumps(export_model(b, r, name="x")), "[]", "{}"))
    assert rep["checked"] == 0
    json.dumps(r)


def test_fab_floors_raise_and_report(tmp_path):
    """Opt-in floor mode ([rules] fab_profile = "jlcpcb_1oz"; the default before 2026-11-12): fab minimums are
    floors, raised and reported; values above are used as-is."""
    b = _board(tmp_path / "a", rules={"min_copper_edge_clearance": 0.025, "min_clearance": 0.1,
                                      "min_track_width": 0.1, "min_hole_clearance": 0.2,
                                      "min_hole_to_hole": 0.1})
    r = resolve_rules(b, dru_text=None, widths_before={}, fab_profile="jlcpcb_1oz")
    assert r["edge_clearance"] == JLCPCB_1OZ["edge"] == 200_000
    assert r["min_clearance"] == 150_000 and r["min_track_width"] == 200_000
    assert r["hole_clearance"] == 350_000 and r["hole_to_hole"] == 250_000
    got = {f["name"]: (f["design"], f["floor"]) for f in r["floors_applied"]}
    assert got["edge_clearance"] == (25_000, 200_000) and got["min_clearance"] == (100_000, 150_000)
    assert "floor: edge_clearance 0.025 -> 0.2 mm" in r["warnings"]
    b2 = _board(tmp_path / "b", rules={"min_copper_edge_clearance": 0.5})  # above the floor: as-is
    r2 = resolve_rules(b2, dru_text=None, widths_before={}, fab_profile="jlcpcb_1oz")
    assert r2["edge_clearance"] == 500_000
    assert "edge_clearance" not in {f["name"] for f in r2["floors_applied"]}


def test_board_values_below_the_fab_minimums_are_used_by_default(tmp_path):
    b = _board(tmp_path, rules={"min_copper_edge_clearance": 0.025, "min_clearance": 0.1})
    r = resolve_rules(b, dru_text=None, widths_before={})
    assert r["edge_clearance"] == 25_000 and r["min_clearance"] == 100_000
    assert r["floors_applied"] == [] and not any(w.startswith("floor:") for w in r["warnings"])


def _pads_by_net(b):
    out: dict[str, int] = {}
    for fp in b.GetFootprints():
        for pad in fp.Pads():
            if pad.GetNetname():
                out[str(pad.GetNetname())] = out.get(str(pad.GetNetname()), 0) + 1
    return out


@pytest.mark.gate
def test_rules_eprom_kicad10_project(tmp_path):
    """Project values come through the .kicad_pro; netclass patterns resolve to the effective class.

    Edit-proof: only the netclass setup / board-setup values (stable project settings) and
    invariants derived from the copy itself are asserted, never counts of the hand routing.
    """
    src = BENCH_BOARDS["eprom-to-kernal"]
    pc = copy_project(src, tmp_path / "w")
    assert pc.pro is not None
    b = load_board(pc)
    widths = record_widths(b)
    r = resolve_rules(b, dru_text=None, widths_before=widths)
    # the board's own Board Setup (default since 2026-11-12): nothing raised, nothing missing
    assert r["edge_clearance"] == 250_000 and r["min_clearance"] == 150_000
    assert r["min_track_width"] == 150_000 and r["hole_clearance"] == 250_000
    assert r["floors_applied"] == [] and r["fallbacks"] == []
    fl = resolve_rules(b, dru_text=None, widths_before=widths, fab_profile="jlcpcb_1oz")  # opt-in floors
    assert {(f["name"], f["design"], f["floor"]) for f in fl["floors_applied"]} == {
        ("min_track_width", 150_000, 200_000), ("hole_clearance", 250_000, 350_000)}
    assert "floor: min_track_width 0.15 -> 0.2 mm" in fl["warnings"] and fl["edge_clearance"] == 250_000
    assert r["default"]["width"] == 250_000 and r["default"]["clearance"] == 200_000
    codes = {str(n.GetNetname()): str(int(c)) for c, n in b.GetNetsByNetcode().items()}
    # pattern / regex assignments (A3, D7, OE -> Signal; +5V, GND -> power), via the effective class
    expect = {"Signal": (250_000, 250_000, 600_000, 300_000), "power": (250_000, 400_000, 800_000, 400_000)}
    for net, cls in (("A3", "Signal"), ("D7", "Signal"), ("OE", "Signal"), ("+5V", "power"), ("GND", "power")):
        pn = r["per_net"][codes[net]]
        assert pn["netclass"].split(",")[0] == cls, net
        assert (pn["clearance"], pn["width"], pn["via_d"], pn["via_drill"]) == expect[cls], net
        assert pn["width_source"] == "netclass", net
    # every net resolves to a known class, and the class width is never below the profile floor
    assert {pn["netclass"].split(",")[0] for pn in r["per_net"].values()} <= {"Signal", "power", "Default"}
    assert all(pn["width"] >= JLCPCB_1OZ["width"] for pn in r["per_net"].values())
    # all multi-pad nets are connected on the hand board, so ripping everything leaves them to route
    multi = {n for n, c in _pads_by_net(b).items() if c >= 2}
    assert set(rip_tracks(b, mode="all").nets_to_route) == multi
    assert_unchanged(src.parent, pc.src_hashes)


@pytest.mark.gate
def test_eprom_rip_none_keeps_free_vias_and_exports_no_pour(tmp_path):
    """rip=none: free vias survive and are exported (fixed obstacles); pour and teardrop zones are
    not obstacles (only rule areas are exported as keepouts). Counts come from the copy itself."""
    from harness.measure import copper_facts

    src = BENCH_BOARDS["eprom-to-kernal"]
    pc = copy_project(src, tmp_path / "w")
    b = load_board(pc)
    facts = copper_facts(b)
    zones = list(b.Zones())
    teardrops = sum(1 for z in zones if z.IsTeardropArea())
    assert any(not z.IsTeardropArea() and not z.GetIsRuleArea() for z in zones)  # the GND pour
    rep = rip_tracks(b, mode="none")
    assert rep.kept_free_vias == facts["free_vias"] and rep.removed_vias == 0 and rep.removed_tracks == 0
    assert sum(1 for z in b.Zones() if z.IsTeardropArea()) == teardrops and len(list(b.Zones())) == len(zones)
    m = export_model(b, resolve_rules(b, dru_text=None, widths_before=record_widths(b)), name="eprom")
    assert len(m["vias"]) == facts["vias"] and sum(1 for v in m["vias"] if v["free"]) == facts["free_vias"]
    assert all(v["free"] for v in m["vias"]) == (facts["vias"] == facts["free_vias"])
    assert m["keepouts"] == []  # no rule areas: neither pour nor teardrops became obstacles
    assert_unchanged(src.parent, pc.src_hashes)


def test_effective_netclass_patterns_on_fixture(tmp_path):
    """Pattern assignment comes from pcbnew's effective netclass (fixture: *VCC* -> Power)."""
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={})
    vcc, sig = r["per_net"][_code(b, "VCC")], r["per_net"][_code(b, "SIG")]
    assert vcc["netclass"].split(",")[0] == "Power" and vcc["via_d"] == 800_000
    assert sig["netclass"] == "Default"


# ---------------------------------------------------------------- pure resolver (M4-R1)
def _pure_vs_swig(b, pro_path, dru_text=None):
    """Pure resolver fed from the .kicad_pro JSON + net names only == the SWIG front end."""
    from toporlib import rules as R
    pro = json.loads(Path(pro_path).read_text())
    nets = {str(n.GetNetname()): int(c) for c, n in b.GetNetsByNetcode().items() if int(c) != 0}
    kw = dict(widths_before={}, settings_table={"SIG": 300_000} if "SIG" in nets else None)
    swig = resolve_rules(b, dru_text=dru_text, **kw)
    pure = R.resolve_rules_from_project(
        pro, dru_text, netclass_of=R.netclass_of_from_pro(pro, nets), classes=R.classes_from_pro(pro),
        net_codes=nets, copper_layer_count=int(b.GetCopperLayerCount()), **kw)
    assert pure == swig
    return pure


def test_resolve_rules_pure_equals_swig(tmp_path):
    b = _board(tmp_path)
    pro = tmp_path / "w" / "f.kicad_pro"
    assert pro.is_file()
    r = _pure_vs_swig(b, pro, dru_text="(rule w (condition \"A.NetName == 'SIG'\") (constraint track_width (opt 0.3mm)))")
    assert r["per_net"][_code(b, "SIG")]["width_source"] == "custom_rule"
    assert r["per_net"][_code(b, "VCC")]["netclass"].split(",")[0] == "Power"
    _pure_vs_swig(b, pro)  # and with no custom rules (floors, settings table)


@pytest.mark.parametrize("board", ["eprom-to-kernal", "wiccon-2023"])
def test_resolve_rules_pure_equals_swig_snapshots(board, tmp_path):
    pc = copy_project(BENCH_BOARDS[board], tmp_path / "w")
    _pure_vs_swig(load_board(pc), pc.pro, dru_text=pc.dru.read_text() if pc.dru else None)


def test_net_codes_by_name_is_sorted_and_one_based():
    from toporlib.rules import net_codes_by_name
    assert net_codes_by_name(["b", "", "a", "b"]) == {"a": 1, "b": 2}


# ---------------------------------------------------------------- dru `disallow` -> keepouts
_SQ = [[0, 0], [10_000_000, 0], [10_000_000, 10_000_000], [0, 10_000_000]]
_AREAS = [{"name": "ant", "layers": [0, 2], "poly": _SQ, "no_tracks": False, "no_vias": False},
          {"name": "ant", "layers": [0, 2], "poly": [[p[0] + 20_000_000, p[1]] for p in _SQ],
           "no_tracks": False, "no_vias": False}]
_LAYERS = {"F.Cu": 0, "B.Cu": 2}


def test_dru_disallow_rule_becomes_keepout():
    from toporlib.dru import dru_keepouts
    rs = parse_dru('(rule no_ant (condition "A.insideArea(\'ant\')") (constraint disallow track via))')
    assert rs[0].disallow == ("track", "via")
    ks, warns = dru_keepouts(rs, _AREAS, _LAYERS)
    assert warns == []
    assert ks == [{"name": "dru:no_ant", "layers": [0, 2], "poly": a["poly"], "no_tracks": True, "no_vias": True,
                   "no_pour": False} for a in _AREAS]  # one keepout per outline of the named rule area
    # via only, one layer
    rs = parse_dru('(rule v (layer "B.Cu") (condition "B.intersectsArea(\'ant\') && A.Type == \'Via\'")'
                   ' (constraint disallow via))')
    ks, _ = dru_keepouts(rs, _AREAS, _LAYERS)
    assert [(k["layers"], k["no_tracks"], k["no_vias"]) for k in ks] == [([2], False, True)] * 2


def test_dru_disallow_skips_and_warns():
    from toporlib.dru import dru_keepouts
    ign = parse_dru('(rule a (severity ignore) (condition "A.insideArea(\'ant\')") (constraint disallow track))')
    assert dru_keepouts(ign, _AREAS, _LAYERS) == ([], [])
    pad_only = parse_dru('(rule a (condition "A.insideArea(\'ant\')") (constraint disallow pad footprint))')
    assert dru_keepouts(pad_only, _AREAS, _LAYERS) == ([], [])  # not routing relevant
    no_area = parse_dru('(rule b (constraint disallow track))')
    ks, w = dru_keepouts(no_area, _AREAS, _LAYERS)
    assert ks == [] and len(w) == 1 and w[0].startswith("REFUSE") and "b" in w[0]
    ors = parse_dru('(rule c (condition "A.insideArea(\'ant\') || A.NetName == \'X\'") (constraint disallow via))')
    ks, w = dru_keepouts(ors, _AREAS, _LAYERS)
    assert ks == [] and w and w[0].startswith("REFUSE")
    missing = parse_dru('(rule d (condition "A.insideArea(\'nope\')") (constraint disallow track))')
    ks, w = dru_keepouts(missing, _AREAS, _LAYERS)
    assert ks == [] and w and "nope" in w[0]


def test_resolve_rules_warns_disallow_without_area():
    from toporlib.rules import resolve_rules_from_project
    r = resolve_rules_from_project({}, '(rule b (constraint disallow via))', netclass_of={}, classes={},
                                   widths_before={})
    assert any(w.startswith("REFUSE") and "rule b" in w for w in r["warnings"])


def test_harness_export_merges_dru_keepouts(tmp_path):
    import pcbnew
    b = _board(tmp_path)
    z = pcbnew.ZONE(b)
    z.SetIsRuleArea(True)
    z.SetZoneName("ant")
    z.SetLayerSet(pcbnew.LSET.ExternalCuMask())
    z.SetDoNotAllowTracks(False)
    z.SetDoNotAllowVias(False)
    z.AppendCorner(pcbnew.VECTOR2I(0, 0), -1)
    for p in ((5_000_000, 0), (5_000_000, 5_000_000), (0, 5_000_000)):
        z.AppendCorner(pcbnew.VECTOR2I(*p), -1)
    b.Add(z)
    dru = '(rule no_ant (condition "A.insideArea(\'ant\')") (constraint disallow track))'
    r = resolve_rules(b, dru_text=dru, widths_before={})
    m = export_model(b, r, name="x", dru_text=dru)
    ant = [k for k in m["keepouts"] if k["name"] == "ant"]
    dk = [k for k in m["keepouts"] if k["name"] == "dru:no_ant"]
    assert len(ant) == 1 and not ant[0]["no_tracks"]
    assert len(dk) == 1 and dk[0]["no_tracks"] and not dk[0]["no_vias"] and dk[0]["poly"] == ant[0]["poly"]
    assert dk[0]["layers"] == ant[0]["layers"] == [0, 2]
    assert [k for k in export_model(b, r, name="x")["keepouts"] if k["name"].startswith("dru:")] == []


def test_courtyard_clearance_rule_is_picked_out():
    """A rule whose whole condition is a courtyard test and that sets a clearance minimum is a region
    rule (bornhack2026 `neck_down`); other conditions, ignored rules and other constraints are not."""
    from toporlib.dru import dru_courtyard_clearances, parse_dru

    dru = """(version 1)
(rule "neck_down" (condition "A.intersectsCourtyard('U6')") (constraint clearance (min 0.1mm)))
(rule "off" (condition "A.insideCourtyard('U1')") (constraint clearance (min 0.2mm)) (severity ignore))
(rule "mixed" (condition "A.intersectsCourtyard('U2') && A.NetClass == 'x'") (constraint clearance (min 0.2mm)))
(rule "width" (condition "A.intersectsCourtyard('U3')") (constraint track_width (min 0.1mm)))
"""
    assert dru_courtyard_clearances(parse_dru(dru)) == [("neck_down", "U6", 100_000)]
