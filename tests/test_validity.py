"""M0v test validity: ``[rules] fab_profile = "board"`` (Board Setup + net classes + custom rules are the authority, no
fab floors) and the harness gate that compares a run's effective rules with the rules an expected answer assumes."""
from __future__ import annotations

import json

import pytest

from harness import validity as va
from toporlib.rules import NetClassVals, resolve_rules_from_project

# bga_neckdown's Board Setup (harness/bga_patterns.py, branch bga-pattern): 0.15 / 0.15, via 0.40 / 0.20, hole clr 0.20
PRO = {"board": {"design_settings": {"rules": {
    "min_clearance": 0.15, "min_track_width": 0.15, "min_via_diameter": 0.4, "min_through_hole_diameter": 0.2,
    "min_via_annular_width": 0.1, "min_hole_clearance": 0.2, "min_hole_to_hole": 0.25,
    "min_copper_edge_clearance": 0.5}}}}
CLASSES = {"Default": NetClassVals("Default", 150_000, 150_000, 400_000, 200_000)}
NECKDOWN = """(version 1)
(rule "BGA neckdown"
  (condition "A.intersectsCourtyard('U1')")
  (constraint track_width (min 0.1mm) (opt 0.15mm))
  (constraint clearance (min 0.1mm)))
"""
OF = {"": "Default", "A1": "Default", "B2": "Default"}


def _resolve(profile, dru=NECKDOWN):
    return resolve_rules_from_project(PRO, dru, netclass_of=OF, classes=CLASSES, widths_before={},
                                      copper_layer_count=4, fab_profile=profile)


def test_default_profile_floors_board_and_region():
    r = _resolve("jlcpcb_1oz")
    assert r["profile"] == "jlcpcb_1oz"
    assert r["min_track_width"] == 200_000 and r["min_via_diameter"] == 450_000 and r["hole_clearance"] == 350_000
    assert r["per_net"]["1"]["width"] == 200_000 and r["per_net"]["1"]["via_d"] == 450_000
    assert r["custom_regions"][0]["clearance"] == 150_000  # floored at the fab clearance
    assert any(n.startswith("floor: custom rule 'BGA neckdown' clearance 0.1 mm -> 0.15 mm") for n in r["custom_notes"])
    assert r["routing_vias"] and r["floors_applied"]


def test_board_authority_uses_board_setup_and_custom_rules_as_given():
    r = _resolve("board")
    assert r["profile"] == "board" and r["floors_applied"] == []
    assert (r["min_clearance"], r["min_track_width"], r["min_via_diameter"], r["min_via_drill"]) == \
        (150_000, 150_000, 400_000, 200_000)
    assert r["hole_clearance"] == r["via_hole_clearance"] == 200_000 and r["edge_clearance"] == 500_000
    assert (r["per_net"]["1"]["width"], r["per_net"]["1"]["clearance"], r["per_net"]["1"]["via_d"],
            r["per_net"]["1"]["via_drill"]) == (150_000, 150_000, 400_000, 200_000)
    # KiCad: the matching custom rule wins, also below the Board Setup minimum clearance (0.15)
    assert r["custom_regions"][0]["clearance"] == 100_000
    assert not any(str(n).startswith("floor:") for n in r.get("custom_notes") or [])
    assert r["routing_vias"] == [] and r["drill_list"] == []


def test_board_authority_keeps_zero_minimums_and_custom_values_below_board_minimums():
    pro = {"board": {"design_settings": {"rules": {"min_clearance": 0.0, "min_track_width": 0.2}}}}
    dru = '(rule w (condition "A.NetName == \'A1\'") (constraint track_width (opt 0.12mm)) (constraint clearance (min 0.09mm)))'
    r = resolve_rules_from_project(pro, dru, netclass_of=OF, classes={"Default": NetClassVals("Default", 200_000)},
                                   widths_before={}, fab_profile="board")
    assert r["min_clearance"] == 0
    assert r["per_net"]["1"]["width"] == 120_000 and r["per_net"]["1"]["clearance"] == 90_000
    assert r["per_net"]["2"]["width"] == 200_000 and r["per_net"]["2"]["clearance"] == 200_000


def test_unknown_profile_is_an_error():
    with pytest.raises(ValueError, match="fab_profile"):
        _resolve("acme")


def _rs(profile, dru=NECKDOWN):
    r = _resolve(profile, dru)
    return va.rule_set(r, {1: "A1", 2: "B2"})


DECL = {"authority": "board",
        "board": {"min_clearance": 0.15, "min_track_width": 0.15, "min_via_diameter": 0.4, "min_via_drill": 0.2,
                  "hole_clearance": 0.2, "via_hole_clearance": 0.2, "hole_to_hole": 0.25, "edge_clearance": 0.5},
        "nets": {"*": {"clearance": 0.15, "width": 0.15, "vias": [[0.4, 0.2]]}},
        "regions": [{"rule": "BGA neckdown", "arg": "U1", "clearance": 0.1}], "margin": 0.0,
        "not_needed": ["'BGA neckdown': track_width inside"]}


def test_gate_flags_fab_floors_and_floored_region_as_invalid():
    eff, brd = _rs("jlcpcb_1oz"), _rs("board")
    v = va.judge(DECL, eff, brd)
    assert not v["valid"]
    rules = {d["rule"] for d in v["invalid"]}
    assert {"board.min_track_width", "board.min_via_diameter", "board.hole_clearance", "net.width", "net.vias",
            "regions"} <= rules
    region = next(d for d in v["invalid"] if d["rule"] == "regions")
    assert region["expected"] == ["U1 0.1"] and region["effective"] == ["U1 0.15"]
    assert "INVALID" in va.verdict_line(v)


def test_gate_valid_under_board_authority_with_declared_not_needed_region_width():
    eff = brd = _rs("board")
    v = va.judge(DECL, eff, brd)
    assert v["valid"], v["invalid"]
    assert any("track_width inside" in n for n in v["notes"])  # the gap is reported, not hidden
    # without the not_needed entry the unsupported region width makes the test invalid
    v2 = va.judge({k: x for k, x in DECL.items() if k != "not_needed"}, eff, brd)
    assert not v2["valid"] and v2["invalid"][0]["rule"] == "unsupported custom rule"


def test_gate_derives_the_declaration_from_the_board_files():
    v = va.judge(None, _rs("jlcpcb_1oz", dru=None), _rs("board", dru=None))
    assert not v["valid"] and v["source"].startswith("board files")
    assert va.judge(None, _rs("board", dru=None), _rs("board", dru=None))["valid"]


def test_declaration_that_the_board_files_do_not_carry_is_invalid():
    decl = json.loads(json.dumps(DECL))
    decl["board"]["min_track_width"] = 0.1
    v = va.judge(decl, _rs("board"), _rs("board"))
    assert not v["valid"]
    assert any(d["rule"].startswith("declaration vs board files: board.min_track_width") for d in v["invalid"])


def test_margin_counts_as_a_rule_difference():
    eff = dict(_rs("board"), margin=0.02)
    v = va.judge(DECL, eff, _rs("board"))
    assert [d["rule"] for d in v["invalid"]] == ["search.margin_um (mm)"]


@pytest.mark.parametrize("via_size,want", [
    ("net_class", [[0.6, 0.3], [0.45, 0.25]]),  # net class via, then the fab-profile 0.25 drill (vias.rs sizes_for)
    ("board_min", [[0.45, 0.25]]),             # a 0.25 trace: pi x 0.25 -> drill 0.08, floored to 0.25 (rule_size)
])
def test_via_list_mirrors_the_engine(via_size, want):
    rules = {"min_via_diameter": 450_000, "min_via_drill": 250_000, "min_annular": 100_000,
             "routing_vias": [250_000, 300_000, 400_000, 500_000], "predefined_vias": [], "via_fallback": [800_000, 400_000]}
    nr = {"via_d": 600_000, "via_drill": 300_000, "width": 250_000}
    assert va.via_list(rules, nr, 1, {"via_size": via_size, "grid_nm": 10_000, "allowed_nm": []}) == want


def test_partial_board_and_net_declaration_keeps_the_omitted_fields():
    """Verifier repro (M0v review): a declaration that names only some board / net fields still compares the
    omitted ones with the board files' values (merged field by field, not one level deep)."""
    decl = {"authority": "board", "board": {"min_clearance": 0.15}, "nets": {"*": {"clearance": 0.15}},
            "not_needed": ["'BGA neckdown': track_width inside"]}
    v = va.judge(decl, _rs("jlcpcb_1oz"), _rs("board"))
    assert not v["valid"]
    rules = {d["rule"] for d in v["invalid"]}
    assert {"board.min_track_width", "board.min_via_diameter", "net.width", "net.vias", "regions"} <= rules
    assert va.judge(decl, _rs("board"), _rs("board"))["valid"]
    m = va.merged_declaration(decl, _rs("board"))
    assert m["board"]["min_track_width"] == 0.15 and m["nets"]["A1"]["width"] == 0.15
    assert m["nets"]["A1"]["vias"] == [[0.4, 0.2]] and m["margin"] == 0.0


def test_find_expected(tmp_path):
    (tmp_path / "pat" / "reference").mkdir(parents=True)
    (tmp_path / "pat" / "expected.json").write_text(json.dumps({"rules_assumed": DECL}))
    pcb = tmp_path / "pat" / "reference" / "pat.kicad_pcb"
    pcb.write_text("")
    assert va.find_expected(pcb) == tmp_path / "pat" / "expected.json"
    assert va.declared_of(va.find_expected(pcb)) == DECL
    assert va.find_expected(tmp_path / "x.kicad_pcb") is None


def test_partial_declaration_takes_the_rest_from_the_board_files():
    nn = ["'BGA neckdown': track_width inside"]
    v = va.judge({"authority": "board", "not_needed": nn}, _rs("jlcpcb_1oz"), _rs("board"))
    assert not v["valid"] and {"net.width", "regions"} <= {d["rule"] for d in v["invalid"]}
    assert va.judge({"authority": "board", "not_needed": nn}, _rs("board"), _rs("board"))["valid"]


def test_declared_options_are_compared_with_the_run_config():
    from topor.config import Config
    decl = {"authority": "board", "not_needed": ["'BGA neckdown': track_width inside"],
            "options": {"vias.size": "board_min"}}
    cfg = Config.defaults()
    v = va.judge(decl, _rs("board"), _rs("board"), cfg=cfg)
    assert [d["rule"] for d in v["invalid"]] == ["option vias.size"]
    assert va.options_of(decl) == {("vias", "size"): "board_min"}
    assert va.judge(decl, _rs("board"), _rs("board"), cfg=cfg.with_values(va.options_of(decl)))["valid"]
