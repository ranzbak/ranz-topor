"""D7: KiCad's diff-pair name rule (MatchDpSuffix) and the pair dimensions (toporlib.diffpair), pure Python."""
from toporlib.diffpair import base_name, detect_pairs, match_dp_suffix, resolve_pairs, smallest_via


def test_match_dp_suffix_kicad_rule():
    assert match_dp_suffix("USB+") == (1, "USB-", 3)
    assert match_dp_suffix("USB-") == (-1, "USB+", 3)
    assert match_dp_suffix("CLK_N1") == (-1, "CLK_P1", 4)  # trailing digits are skipped
    assert match_dp_suffix("D0P_1") == (1, "D0N_1", 2)  # digits and _ skipped, then P
    assert match_dp_suffix("/hdmi/TX0_P") == (1, "/hdmi/TX0_N", 10)  # full name, sheet path kept
    assert match_dp_suffix("usb_p") is None  # lower case is no polarity
    assert match_dp_suffix("123") is None and match_dp_suffix("") is None
    assert match_dp_suffix("EN") == (-1, "EP", 1)  # matches, but has no partner
    assert base_name("CLK_N1") == "CLK_1" and base_name("VCC") == "VCC"


def test_detect_pairs_needs_both_halves_and_one_style():
    names = ["USB+", "USB-", "CLK_P1", "CLK_N1", "EN", "SIG_P1", "MIX+", "MIX_N", "/a/RXP", "/b/RXN",
             "/x/TXP", "/x/TXN", "D0P", "D0N"]
    assert detect_pairs(names) == [("/x/TXP", "/x/TXN"), ("CLK_P1", "CLK_N1"), ("D0P", "D0N"), ("USB+", "USB-")]


def test_resolve_pairs_netclass_rule_and_floors():
    pro = {"net_settings": {"classes": [
        {"name": "Default", "diff_pair_width": 0.2, "diff_pair_gap": 0.25, "diff_pair_via_gap": 0.25},
        {"name": "HDMI_TX", "diff_pair_width": 0.16, "diff_pair_gap": 0.2, "diff_pair_via_gap": 0.25}]}}
    codes = {"ADV_TX0+": 1, "ADV_TX0-": 2, "USB_C_D+": 3, "USB_C_D-": 4, "X": 5}
    of = {"ADV_TX0+": "HDMI_TX", "ADV_TX0-": "HDMI_TX"}
    rules = {"min_track_width": 90_000, "min_clearance": 90_000, "hole_to_hole": 250_000,
             "via_hole_clearance": 200_000, "per_net": {"1": {"via_d": 450_000, "via_drill": 300_000},
                                                        "3": {"via_d": 800_000, "via_drill": 400_000}}}
    dru = """(version 1)
(rule hdmi_gap (condition "A.inDiffPair('ADV_TX*')") (constraint diff_pair_gap (min 0.15mm) (opt 0.18mm) (max 0.3mm)))
(rule unc (condition "A.inDiffPair('*')") (constraint diff_pair_uncoupled (max 5mm)))
(rule sk (condition "A.inDiffPair('USB_C_D*')") (constraint skew (max 0.1mm)))
"""
    out, notes = resolve_pairs(codes, pro=pro, dru_text=dru, netclass_of=of, rules=rules)
    by = {d["p_name"]: d for d in out}
    hd, usb = by["ADV_TX0+"], by["USB_C_D+"]
    assert (hd["p"], hd["n"], hd["width"], hd["gap"]) == (1, 2, 160_000, 180_000)  # rule opt wins over the class
    assert (hd["gap_min"], hd["gap_max"], hd["rule_gap"]) == (150_000, 300_000, "hdmi_gap")
    # via gap (user 2026-11-12: from the net class): max(class diff_pair_via_gap, hole-to-hole - 2 annular,
    # copper-to-hole - annular) = max(0.25, 0.1, 0.125); before: the pair gap 0.18
    assert hd["via_gap"] == 250_000 and hd["max_uncoupled"] == 5_000_000 and hd["max_skew"] == 0
    # no Board Setup via minimum and no pre-defined via here: the smallest allowed via is the class via
    assert (hd["via_d"], hd["via_drill"], hd["sources"]["via"]) == (450_000, 300_000, "net class via")
    assert (usb["width"], usb["gap"], usb["max_skew"], usb["rule_skew"]) == (200_000, 250_000, 100_000, "sk")
    assert usb["gap_max"] == 0 and usb["rule_gap"] == ""  # netclass gap: no max, no violations
    assert len(notes) == 2
    # board minimums raise width and gap (KiCad: max(min track, rule), max(min clearance, rule))
    floored, _ = resolve_pairs(codes, pro=pro, dru_text=None, netclass_of=of,
                               rules={**rules, "min_track_width": 200_000, "min_clearance": 220_000})
    hdf = {d["p_name"]: d for d in floored}["ADV_TX0+"]
    assert hdf["width"] == 200_000 and hdf["gap"] == 220_000
    # settings: detect off, manual pairs, numeric gap override, width from the track
    off, _ = resolve_pairs(codes, pro=pro, dru_text=None, netclass_of=of, rules=rules, settings={"detect": "off"})
    assert off == []
    man, _ = resolve_pairs({"DP": 1, "DM": 2}, pro=pro, dru_text=None, netclass_of={}, rules=rules,
                           settings={"pairs": {"DP": "DM"}, "gap_mm": 0.3})
    assert len(man) == 1 and man[0]["gap"] == 300_000


def test_pair_via_is_the_smallest_the_board_allows():
    """User 2026-11-12 ("the smaller the via the better, to keep impedance disruption to a minimum"): a pair's via is
    the Board Setup minimum via carrying the pair width (vias.rs rule_size) or a smaller legal pre-defined via,
    never the larger net class via; [diffpair] via = "net_class" keeps the class via. The via gap uses its ring."""
    pro = {"net_settings": {"classes": [{"name": "Default", "diff_pair_width": 0.2, "diff_pair_gap": 0.15,
                                         "diff_pair_via_gap": 0.25}]}}
    codes = {"USB_P": 1, "USB_N": 2}
    rules = {"min_track_width": 200_000, "min_clearance": 150_000, "hole_to_hole": 250_000, "hole_clearance": 250_000,
             "via_hole_clearance": 250_000, "min_via_diameter": 450_000, "min_via_drill": 250_000, "min_annular": 100_000,
             "predefined_vias": [], "default": {"via_d": 600_000, "via_drill": 300_000}, "per_net": {}}
    (dp,), notes = resolve_pairs(codes, pro=pro, dru_text=None, netclass_of={}, rules=rules,
                                 settings={"drill_grid_mm": 0.01})
    # the dp_* patterns' via: 0.45 / 0.25 (pi x 0.25 >= 0.2), pitch 0.45 + max(0.25, 0.25 - 0.2, 0.25 - 0.1) = 0.70
    assert (dp["via_d"], dp["via_drill"], dp["via_gap"]) == (450_000, 250_000, 250_000)
    assert dp["sources"]["via"] == "Board Setup minimum via" and "via 0.45 / 0.25 mm" in notes[0]
    (cls,), _ = resolve_pairs(codes, pro=pro, dru_text=None, netclass_of={}, rules=rules, settings={"via": "net_class"})
    assert (cls["via_d"], cls["via_drill"]) == (600_000, 300_000)
    # pre-defined vias: one below the minimums never wins; without Board Setup via minimums (0: no minimum via
    # candidate) the smallest pre-defined via that carries the width does
    r2 = {**rules, "predefined_vias": [[400_000, 250_000], [500_000, 300_000]]}
    assert smallest_via(r2, 200_000, (600_000, 300_000)) == (450_000, 250_000, "Board Setup minimum via")
    r3 = {**r2, "min_via_diameter": 0, "min_via_drill": 0, "min_annular": 0}
    assert smallest_via(r3, 200_000, (600_000, 300_000)) == (400_000, 250_000, "pre-defined via")
    assert smallest_via(r3, 900_000, (600_000, 300_000))[:2] == (500_000, 300_000)  # pi x 0.25 < 0.9
    # a wide pair needs a bigger drill (pi x drill >= width), rounded up on the drill grid, through the drill filter
    assert smallest_via(rules, 1_000_000, (900_000, 500_000), drill_grid_nm=10_000)[:2] == (520_000, 320_000)
    assert smallest_via(rules, 1_000_000, (900_000, 500_000), drill_grid_nm=10_000,
                        allowed_drills_nm=[300_000, 400_000])[:2] == (600_000, 400_000)
    # never larger than the class via; no Board Setup minimum (0) -> no minimum-via candidate
    assert smallest_via({**rules, "min_via_diameter": 0}, 200_000, (500_000, 300_000))[:2] == (500_000, 300_000)


def test_pair_via_respects_custom_rule_via_minimums():
    """A custom rule's via_diameter / hole_size minimum on a pair net replaces the Board Setup minimum in KiCad's
    DRC (drc_engine.cpp: a matching rule returns before the board-minimum clamp). Before this fix the pair got the
    Board Setup minimum via 0.45 / 0.25 anyway: a KiCad DRC violation. Both nets count (KiCad checks each via
    against its own net)."""
    from toporlib.rules import NetClassVals, resolve_rules_from_project

    pro = {"board": {"design_settings": {"rules": {
        "min_clearance": 0.15, "min_track_width": 0.2, "min_via_diameter": 0.45, "min_through_hole_diameter": 0.25,
        "min_via_annular_width": 0.1, "min_hole_clearance": 0.25, "min_hole_to_hole": 0.25,
        "min_copper_edge_clearance": 0.5, "min_connection": 0, "max_error": 0.005}}},
        "net_settings": {"classes": [{"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.8,
                                      "via_drill": 0.4, "diff_pair_width": 0.2, "diff_pair_gap": 0.15,
                                      "diff_pair_via_gap": 0.25}]}}
    dru = """(version 1)
(rule usb_vias (condition "A.NetName == 'USB_N'")
  (constraint via_diameter (min 0.6mm)) (constraint hole_size (min 0.35mm)))
"""
    codes = {"USB_P": 1, "USB_N": 2}
    classes = {"Default": NetClassVals("Default", 200_000, 250_000, 800_000, 400_000)}
    rules = resolve_rules_from_project(pro, dru, netclass_of={n: "Default" for n in codes}, classes=classes,
                                       widths_before={}, net_codes=codes)
    assert rules["per_net"]["2"]["via_d_min"] == 600_000 and rules["per_net"]["2"]["via_drill_min"] == 350_000
    assert "via_d_min" not in rules["per_net"]["1"]
    (dp,), _ = resolve_pairs(codes, pro=pro, dru_text=dru, netclass_of={}, rules=rules)
    # drill = the N net's minimum 0.35, diameter = max(0.6, 0.35 + 2 x 0.1); not 0.45 / 0.25
    assert (dp["via_d"], dp["via_drill"]) == (600_000, 350_000), dp
    # without the rule: the Board Setup minimum via
    plain = resolve_rules_from_project(pro, None, netclass_of={n: "Default" for n in codes}, classes=classes,
                                       widths_before={}, net_codes=codes)
    (dq,), _ = resolve_pairs(codes, pro=pro, dru_text=None, netclass_of={}, rules=plain)
    assert (dq["via_d"], dq["via_drill"]) == (450_000, 250_000)
    # [diffpair] via = "net_class": the class via 0.8 / 0.4 already meets the minimums
    (dc,), _ = resolve_pairs(codes, pro=pro, dru_text=dru, netclass_of={}, rules=rules, settings={"via": "net_class"})
    assert (dc["via_d"], dc["via_drill"]) == (800_000, 400_000)
