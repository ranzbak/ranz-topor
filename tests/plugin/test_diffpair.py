"""D7: KiCad's diff-pair name rule (MatchDpSuffix) and the pair dimensions (toporlib.diffpair), pure Python."""
from toporlib.diffpair import base_name, detect_pairs, match_dp_suffix, resolve_pairs


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
    # via gap: max(gap, hole-to-hole - 2 annular, copper-to-hole - annular) = max(0.18, 0.1, 0.125)
    assert hd["via_gap"] == 180_000 and hd["max_uncoupled"] == 5_000_000 and hd["max_skew"] == 0
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
