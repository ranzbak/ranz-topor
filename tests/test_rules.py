import json
from pathlib import Path

import pytest

from harness.copy import assert_unchanged, copy_project
from harness.fixtures import make_fixture_board, write_project_file
from harness.model import export_model, load_board
from harness.rip import NetWidths, record_widths, rip_tracks
from harness.rules import JLCPCB_1OZ, match_condition, parse_dru, resolve_rules

EPROM = "/home/paul/work/kicad/projects/eprom-to-kernal/eprom-to-kernal/eprom-to-kernal.kicad_pcb"

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
    assert JLCPCB_1OZ["drill_list"] == [200_000, 250_000, 300_000, 400_000, 500_000, 800_000, 1_000_000] and JLCPCB_1OZ["routing_vias"] == [250_000, 300_000, 400_000, 500_000]
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
    assert r["edge_clearance"] == 500_000 and r["min_track_width"] == 200_000 and r["hole_clearance"] == 350_000  # 0.25 design, raised to the PTH floor
    assert r["per_net"][vcc]["width"] == 400_000 and r["per_net"][vcc]["width_source"] == "netclass"
    assert r["per_net"][sig]["width"] == 250_000 and r["per_net"][sig]["width_source"] == "default"
    assert r["default"]["clearance"] == 200_000 and r["profile"] == "jlcpcb_1oz"


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


def test_unknown_condition_is_strictest_with_warning(tmp_path):
    b = _board(tmp_path)
    r = resolve_rules(
        b,
        dru_text='(rule c (condition "A.intersectsCourtyard(\'U1\')") (constraint clearance (min 0.35mm)))',
        widths_before={},
    )
    assert r["default"]["clearance"] == 350_000
    assert all(v["clearance"] == 350_000 for v in r["per_net"].values())
    assert any("unparsed" in w for w in r["warnings"])


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


def test_profile_fills_zero_floors(tmp_path):
    b = _board(tmp_path, rules={"min_clearance": 0, "min_via_diameter": 0})
    r = resolve_rules(b, dru_text=None, widths_before={})
    assert r["min_clearance"] == 150_000 and r["min_via_diameter"] == 450_000


def test_output_parses_with_engine_rules(tmp_path, engine):
    b = _board(tmp_path)
    r = resolve_rules(b, dru_text=None, widths_before={})
    rep = json.loads(engine.check_items(json.dumps(export_model(b, r, name="x")), "[]", "{}"))
    assert rep["checked"] == 0
    json.dumps(r)


def test_fab_floors_raise_and_report(tmp_path):
    """Fab minimums are floors by default: raised and reported; values above are used as-is."""
    b = _board(tmp_path / "a", rules={"min_copper_edge_clearance": 0.025, "min_clearance": 0.1,
                                      "min_track_width": 0.1, "min_hole_clearance": 0.2,
                                      "min_hole_to_hole": 0.1})
    r = resolve_rules(b, dru_text=None, widths_before={})
    assert r["edge_clearance"] == JLCPCB_1OZ["edge"] == 200_000
    assert r["min_clearance"] == 150_000 and r["min_track_width"] == 200_000
    assert r["hole_clearance"] == 350_000 and r["hole_to_hole"] == 250_000
    got = {f["name"]: (f["design"], f["floor"]) for f in r["floors_applied"]}
    assert got["edge_clearance"] == (25_000, 200_000) and got["min_clearance"] == (100_000, 150_000)
    assert "floor: edge_clearance 0.025 -> 0.2 mm" in r["warnings"]
    b2 = _board(tmp_path / "b", rules={"min_copper_edge_clearance": 0.5})  # above the floor: as-is
    r2 = resolve_rules(b2, dru_text=None, widths_before={})
    assert r2["edge_clearance"] == 500_000
    assert "edge_clearance" not in {f["name"] for f in r2["floors_applied"]}


def test_fab_floors_opt_out(tmp_path):
    b = _board(tmp_path, rules={"min_copper_edge_clearance": 0.025, "min_clearance": 0.1})
    r = resolve_rules(b, dru_text=None, widths_before={}, apply_fab_floors=False)
    assert r["edge_clearance"] == 25_000 and r["min_clearance"] == 100_000
    assert r["floors_applied"] == [] and not any(w.startswith("floor:") for w in r["warnings"])


@pytest.mark.gate
def test_rules_eprom_kicad10_project(tmp_path):
    """Re-measured 2026-10-30 (R2): project values come through the .kicad_pro; floors reported."""
    src = Path(EPROM)
    pc = copy_project(src, tmp_path / "w")
    assert pc.pro is not None
    b = load_board(pc)
    widths = record_widths(b)
    r = resolve_rules(b, dru_text=None, widths_before=widths)
    assert r["edge_clearance"] == 250_000  # board setup 0.25 mm, above the 0.2 mm fab floor
    assert r["min_clearance"] == 150_000
    assert {(f["name"], f["design"], f["floor"]) for f in r["floors_applied"]} == {
        ("min_track_width", 150_000, 200_000), ("hole_clearance", 250_000, 350_000)}
    assert "floor: min_track_width 0.15 -> 0.2 mm" in r["warnings"]
    assert resolve_rules(b, dru_text=None, widths_before=widths,
                         apply_fab_floors=False)["edge_clearance"] == 250_000
    assert r["default"]["width"] == 250_000
    codes = {str(n.GetNetname()): str(int(c)) for c, n in b.GetNetsByNetcode().items()}
    for net in ("+5V", "GND"):
        assert r["per_net"][codes[net]]["width"] == 400_000
        assert r["per_net"][codes[net]]["width_source"] == "existing_copper"
    assert len(rip_tracks(b, mode="all").nets_to_route) == 26
    assert_unchanged(src.parent, pc.src_hashes)


@pytest.mark.gate
def test_eprom_rip_none_keeps_free_vias_and_exports_no_pour(tmp_path):
    """rip=none: 15 free GND stitching vias survive and are exported (fixed obstacles); pour and
    teardrop zones are not obstacles (only rule areas are exported as keepouts)."""
    src = Path(EPROM)
    pc = copy_project(src, tmp_path / "w")
    b = load_board(pc)
    zones = list(b.Zones())
    assert sum(1 for z in zones if z.IsTeardropArea()) == 54
    assert any(not z.IsTeardropArea() and not z.GetIsRuleArea() for z in zones)  # the GND pour
    rep = rip_tracks(b, mode="none")
    assert rep.kept_free_vias == 15 and rep.removed_vias == 0 and rep.removed_tracks == 0
    assert len(list(b.Zones())) == len(zones)
    m = export_model(b, resolve_rules(b, dru_text=None, widths_before=record_widths(b)), name="eprom")
    gnd = next(n["code"] for n in m["nets"] if n["name"] == "GND")
    assert len(m["vias"]) == 15 and all(v["free"] and v["net"] == gnd for v in m["vias"])
    assert {(v["d"], v["drill"]) for v in m["vias"]} == {(800_000, 400_000)}
    assert m["keepouts"] == []  # no rule areas: neither pour nor teardrops became obstacles
    assert_unchanged(src.parent, pc.src_hashes)
