"""harness/copper_check.py: every own check on a tiny synthetic board, the generated rules through real kicad-cli
(isolated), the recall driver, and (when the file exists) the recall on the user's qmtech run of 2026-11-27."""
from __future__ import annotations

import json
import math
import shutil
from pathlib import Path

import pytest

from harness import copper_check as cc

ROOT = Path(__file__).resolve().parents[1]
REAL = ROOT.parent / "reports/pad-entry-spots/user-qm-routed.kicad_pcb"   # worktree = <repo>/scratch/agent-wt/wt-x
PRO = Path.home() / "work/kicad/projects/qmtech-minimig (copy)/qmtech-minimig.kicad_pro"


def fp(ref, x, y, pads, ang=0):
    body = "".join(
        f'(pad "{n}" smd rect (at {px} {py} {pa}) (size {w} {h}) (layers "F.Cu" "F.Mask" "F.Paste") (net "{net}"))'
        for n, px, py, pa, w, h, net in pads)
    return f'(footprint "X:{ref}" (layer "F.Cu") (uuid "u-{ref}") (at {x} {y} {ang}) (property "Reference" "{ref}") {body})'


def seg(x1, y1, x2, y2, w, net, layer="F.Cu"):
    return f'(segment (start {x1} {y1}) (end {x2} {y2}) (width {w}) (layer "{layer}") (net "{net}") (uuid "s{x1}{y1}{x2}{y2}"))'


def via(x, y, net, size=0.6, drill=0.3):
    return f'(via (at {x} {y}) (size {size}) (drill {drill}) (layers "F.Cu" "B.Cu") (net "{net}") (uuid "v{x}{y}"))'


def board(tmp_path, items, width=0.5):
    pcb = tmp_path / "t.kicad_pcb"
    pcb.write_text('(kicad_pcb (version 20260206) (layers (0 "F.Cu" signal) (2 "B.Cu" signal)) ' + " ".join(items) + ")")
    pro = tmp_path / "t.kicad_pro"
    pro.write_text(json.dumps({"net_settings": {"classes": [
        {"name": "Default", "track_width": 0.2, "clearance": 0.15, "via_diameter": 0.6, "via_drill": 0.3},
        {"name": "P", "track_width": width, "clearance": 0.15, "via_diameter": 0.6, "via_drill": 0.3}],
        "netclass_assignments": {}, "netclass_patterns": [{"netclass": "P", "pattern": "PWR*"}]},
        "board": {"design_settings": {"rules": {"min_clearance": 0.1}}}}))
    return pcb, pro


def kinds(rows):
    return sorted({r["type"] for r in rows})


def test_sexpr_and_arc():
    t = cc.parse_sexpr('(a (b "x y" 1.5) (c))')
    assert t == ["a", ["b", "x y", "1.5"], ["c"]]
    ch = cc._arc_chords((0, 0), (1, 1), (2, 0))
    assert math.dist(ch[0][0], (0, 0)) < 1e-9 and math.dist(ch[-1][1], (2, 0)) < 1e-9
    assert all(abs(math.hypot(a[0] - 1, a[1]) - 1.0) < 1e-6 for a, _ in ch)


def test_rotated_footprint_pad_position(tmp_path):
    pcb, pro = board(tmp_path, [fp("C1", 10, 10, [("1", -0.95, 0, 90, 1.0, 1.45, "A")], ang=90)])
    b = cc.load_board(pcb, pro)
    p = b.pads[0]
    assert (round(p.x, 3), round(p.y, 3)) == (10.0, 10.95)  # KiCad y-down: a left pad of a 90 deg part sits below
    assert p.poly.bounds[3] - p.poly.bounds[1] == pytest.approx(1.0, abs=1e-6)  # pad frame x (1.0) is global y after 90 deg... long side 1.45 on x
    assert p.poly.bounds[2] - p.poly.bounds[0] == pytest.approx(1.45, abs=1e-6)


def test_via_in_pad_and_overlap(tmp_path):
    pcb, pro = board(tmp_path, [fp("R1", 0, 0, [("1", 0, 0, 0, 1.0, 1.0, "A")]), via(0, 0, "A"), via(0.65, 0, "A")])
    rows = cc.check_vias(cc.Ctx(cc.load_board(pcb, pro)))
    assert sorted(r["type"] for r in rows) == ["via_in_pad", "via_overlaps_pad"]


def test_neck_with_room_and_forced(tmp_path):
    items = [seg(0, 0, 5, 0, 0.2, "PWR1"), seg(0, 3, 5, 3, 0.2, "PWR2"), seg(0, 3.3, 5, 3.3, 0.2, "OTHER"), seg(0, 2.7, 5, 2.7, 0.2, "OTHER")]
    pcb, pro = board(tmp_path, items)
    rows = cc.check_necks(cc.Ctx(cc.load_board(pcb, pro)))
    by = {r["net"]: r for r in rows}
    assert by["PWR1"]["type"] == "neck_room_fits"           # free corridor, class 0.5 fits
    assert by["PWR2"]["type"] == "neck_forced"   # neighbours on both sides 0.3 away leave no room
    assert by["PWR2"]["corridor_fit_mm"] < 0.5
    assert by["PWR1"]["size_mm"] == pytest.approx(5.0)


def test_double_entry_and_off_centre(tmp_path):
    pads = [fp("U1", 0, 0, [("1", 0, 0, 0, 1.4, 0.3, "A")]), fp("U2", 10, 0, [("1", 0, 0, 0, 1.4, 0.3, "A")])]
    # a wire from the centre, and a second wire that ENDS 0.5 mm off the centre inside the same pad (own end, not a T)
    items = pads + [seg(0, 0, 5, 0, 0.1, "A"), seg(0.5, 0.1, 3, 0.1, 0.05, "A"), seg(5, 0, 10, 0, 0.1, "A")]
    pcb, pro = board(tmp_path, items)
    rows = cc.check_pad_entries(cc.Ctx(cc.load_board(pcb, pro)))
    assert "double_entry" in kinds(rows)
    # one wire ending off the centre only
    items = pads + [seg(0.5, 0, 5, 0, 0.2, "A"), seg(5, 0, 10, 0, 0.2, "A")]
    pcb, pro = board(tmp_path, items)
    rows = cc.check_pad_entries(cc.Ctx(cc.load_board(pcb, pro)))
    assert "end_off_centre" in kinds(rows) and "double_entry" not in kinds(rows)


def test_off_axis_entry_line_misses_centre(tmp_path):
    pad = fp("U1", 0, 0, [("1", 0, 0, 90, 1.4, 0.3, "A")])   # long axis vertical, 0.3 wide
    def rows(*segs):
        return cc.check_pad_entries(cc.Ctx(cc.load_board(*board(tmp_path, [pad, *segs]))))
    assert "off_axis_entry" not in kinds(rows(seg(0, 0, 0, 3, 0.2, "A")))           # on the axis
    assert "off_axis_entry" not in kinds(rows(seg(0, 0, 2, 2, 0.2, "A")))           # 45 deg leg aimed at the centre: legal
    assert "off_axis_entry" in kinds(rows(seg(0, 0, 0.13, 0.4, 0.2, "A"), seg(0.13, 0.4, 0.13, 3, 0.2, "A")))  # lane beside the axis
    assert "off_axis_entry" in kinds(rows(seg(0.14, -0.5, 2.14, 1.5, 0.2, "A")))           # corner entry, line misses the centre


def test_pair_member_is_judged_at_the_pair_width(tmp_path):
    items = [seg(0, 0, 5, 0, 0.16, "D_P"), seg(0, 1, 5, 1, 0.16, "D_N")]
    pcb, pro = board(tmp_path, items)
    d = json.loads(pro.read_text())
    d["net_settings"]["classes"][0]["diff_pair_width"] = 0.16
    d["net_settings"]["classes"][0]["track_width"] = 0.2
    pro.write_text(json.dumps(d))
    b = cc.load_board(pcb, pro)
    assert cc.expected_width(b, "D_P") == 0.16
    assert cc.check_necks(cc.Ctx(b)) == []
    items = [seg(0, 0, 5, 0, 0.16, "SOLO")]              # same width, no partner: a real neck
    pcb, pro2 = board(tmp_path, items)
    pro2.write_text(json.dumps(d))
    rows = cc.check_necks(cc.Ctx(cc.load_board(pcb, pro2)))
    assert [r["net"] for r in rows] == ["SOLO"]


def test_detour_ratio(tmp_path):
    pads = [fp("U1", 0, 0, [("1", 0, 0, 0, 1, 1, "A")]), fp("U2", 10, 0, [("1", 0, 0, 0, 1, 1, "A")])]
    straight = [seg(0, 0, 10, 0, 0.2, "A")]
    loop = [seg(0, 0, 0, 5, 0.2, "A"), seg(0, 5, 10, 5, 0.2, "A"), seg(10, 5, 10, 0, 0.2, "A")]
    assert cc.check_detours(cc.Ctx(cc.load_board(*board(tmp_path, pads + straight))), 1.3) == []
    rows = cc.check_detours(cc.Ctx(cc.load_board(*board(tmp_path, pads + loop))), 1.3)
    assert len(rows) == 1 and rows[0]["ratio"] == pytest.approx(2.0)


def test_power_chain_through_narrow_pin(tmp_path):
    mid = fp("U1", 5, 0, [("1", 0, 0, 0, 0.4, 1.4, "PWR1")])
    ends = [fp("C1", 0, 0, [("1", 0, 0, 0, 1, 1, "PWR1")]), fp("C2", 10, 0, [("1", 0, 0, 0, 1, 1, "PWR1")])]
    items = [mid] + ends + [seg(0, 0, 5, 0, 1.0, "PWR1"), seg(5, 0, 10, 0, 1.0, "PWR1")]
    rows = cc.check_power_chain(cc.Ctx(cc.load_board(*board(tmp_path, items, width=1.0))))
    assert [r["connection"] for r in rows] == ["U1.1"]
    # a narrow pin that only ENDS the net (copper on one side only) is not a chain
    end_only = [mid] + ends[:1] + [seg(0, 0, 5, 0, 1.0, "PWR1")]
    assert cc.check_power_chain(cc.Ctx(cc.load_board(*board(tmp_path, end_only, width=1.0)))) == []
    # a pad as wide as the trace is fine
    wide = fp("U1", 5, 0, [("1", 0, 0, 0, 0.9, 1.4, "PWR1")])
    items = [wide] + ends + [seg(0, 0, 5, 0, 1.0, "PWR1"), seg(5, 0, 10, 0, 1.0, "PWR1")]
    assert cc.check_power_chain(cc.Ctx(cc.load_board(*board(tmp_path, items, width=1.0)))) == []


def test_bends_acute_and_free_angle(tmp_path):
    pads = [fp("U1", 0, 0, [("1", 0, 0, 0, 1, 1, "A")]), fp("U2", 10, 0, [("1", 0, 0, 0, 1, 1, "A")])]
    stair = [seg(0, 0, 1, 0, 0.2, "A"), seg(1, 0, 2, 1, 0.2, "A"), seg(2, 1, 3, 1, 0.2, "A"), seg(3, 1, 4, 0, 0.2, "A"),
             seg(4, 0, 5, 0, 0.2, "A"), seg(5, 0, 6, 1, 0.2, "A"), seg(6, 1, 7, 1, 0.2, "A"), seg(7, 1, 8, 0, 0.2, "A"),
             seg(8, 0, 10, 0, 0.2, "A")]
    rows = cc.check_shape(cc.Ctx(cc.load_board(*board(tmp_path, pads + stair))))
    assert "bend_excess" in kinds(rows) and all(r["severity"] == "info" for r in rows if r["type"] == "bend_excess")
    spike = [seg(0, 0, 6, 0, 0.2, "A"), seg(6, 0, 5, 0.2, 0.2, "A"), seg(5, 0.2, 10, 0, 0.2, "A")]
    rows = cc.check_shape(cc.Ctx(cc.load_board(*board(tmp_path, pads + spike))))
    assert "acute_angle" in kinds(rows) and "free_angle" in kinds(rows)


def test_min_clearance_run(tmp_path):
    items = [seg(0, 0, 3, 0, 0.2, "A"), seg(0, 0.35, 3, 0.35, 0.2, "B")]  # gap 0.15 = the class clearance
    rows = cc.check_hug(cc.Ctx(cc.load_board(*board(tmp_path, items))))
    assert rows and rows[0]["size_mm"] >= 2.5
    items = [seg(0, 0, 3, 0, 0.2, "A"), seg(0, 0.6, 3, 0.6, 0.2, "B")]
    assert cc.check_hug(cc.Ctx(cc.load_board(*board(tmp_path, items)))) == []


def test_decap_and_skew(tmp_path):
    ic = fp("U1", 0, 0, [("1", 0, 0, 0, 1, 1, "VDD")])
    cap = fp("C1", 12, 0, [("1", 0, 0, 0, 1, 1, "VDD"), ("2", 1, 0, 0, 1, 1, "GND")])
    rows = cc.check_decap(cc.Ctx(cc.load_board(*board(tmp_path, [ic, cap]))))
    assert [r["type"] for r in rows] == ["decap_far"]
    items = [seg(0, 0, 10, 0, 0.2, "D_P"), seg(0, 1, 12, 1, 0.2, "D_N")]
    rows = cc.check_diffpairs(cc.Ctx(cc.load_board(*board(tmp_path, items))))
    assert len(rows) == 1 and rows[0]["size_mm"] == pytest.approx(2.0)


def test_review_rules_text(tmp_path):
    pads = [fp("U1", 0, 0, [("1", 0, 0, 0, 1, 1, "PWR1")]), fp("U2", 10, 0, [("1", 0, 0, 0, 1, 1, "PWR1")])]
    b = cc.load_board(*board(tmp_path, pads + [seg(0, 0, 10, 0, 0.5, "PWR1")]))
    cx = cc.Ctx(b)
    txt = cc.review_rules(b, cx)
    assert "(severity warning)" in txt and "track_angle" not in txt
    assert "rv_neck_P" in txt and "rv_via_in_pad" in txt and 'constraint length (max 13.000mm)' in txt


def test_drc_rows_classify_and_filter():
    d = {"violations": [
        {"type": "clearance", "severity": "error", "description": "Clearance violation (actual 0,0673 mm)",
         "items": [{"description": "Track [A] on F.Cu", "pos": {"x": 1, "y": 2}, "uuid": "u"}]},
        {"type": "lib_footprint_issues", "severity": "warning", "description": "lib", "items": []},
        {"type": "track_width", "severity": "warning", "description": "Track width (rule 'rv_neck_P' min width 1; actual 0,3 mm)",
         "items": [{"description": "Track [B] on F.Cu", "pos": {"x": 3, "y": 4}, "uuid": "w"}]},
        {"type": "clearance", "severity": "warning", "description": "Clearance violation (rule 'rv_via_in_pad' clearance 0,001 mm; actual 0,2799 mm)",
         "items": []},
        {"type": "clearance", "severity": "warning", "description": "Clearance violation (rule 'rv_via_in_pad' clearance 0,001 mm; actual 0,0000 mm)",
         "items": [{"description": "Via [C] on F.Cu - B.Cu", "pos": {"x": 5, "y": 6}, "uuid": "x"}]}]}
    rows = cc.drc_rows(d)
    assert [(r["type"], r["source"]) for r in rows] == [("clearance", "kicad"), ("track_width_rule", "rule"), ("via_in_pad_rule", "rule")]
    assert rows[0]["size_mm"] == pytest.approx(0.0673) and rows[0]["net"] == "A" and rows[0]["minutes"] == 3.0


def test_recall_driver():
    rows = [{"type": "via_in_pad", "minutes": 4, "source": "own", "net": "N1", "items": [{"pos": {"x": 1.0, "y": 1.0}}]}]
    spots = {"spots": [{"id": "S1", "what": "w", "x": 1.2, "y": 1.0, "r": 1.0, "net": "N1", "kinds": ["via_in_pad"]},
                       {"id": "S2", "what": "w", "x": 9.0, "y": 1.0, "r": 1.0, "kinds": ["via_in_pad"]},
                       {"id": "S3", "what": "w", "x": 1.0, "y": 1.0, "r": 1.0, "net": "OTHER", "kinds": ["via_in_pad"]},
                       {"id": "S4", "what": "w", "x": 1.0, "y": 1.0, "r": 1.0, "kinds": ["detour"]}]}
    r = cc.recall(rows, spots)
    assert [s["found"] for s in r["spots"]] == [True, False, False, False]


@pytest.mark.skipif(not shutil.which("kicad-cli"), reason="kicad-cli missing")
def test_generated_rules_fire_in_real_kicad(tmp_path):
    """The review rules parse in KiCad 10 and emit markers with coordinates: neck, via in pad, length."""
    # the via (0.6) is wider than the pad (0.4): physical_clearance sees the touching outlines (a via fully inside a pad
    # has a positive outline distance and is left to the own check)
    pads = [fp("U1", 0, 0, [("1", 0, 0, 0, 0.4, 0.4, "PWR1")]), fp("U2", 10, 0, [("1", 0, 0, 0, 1, 1, "PWR1")])]
    items = pads + [via(0, 0, "PWR1"), seg(0, 0, 5, 0.5, 0.2, "PWR1"), seg(5, 0.5, 10, 0, 0.2, "PWR1")]
    pcb, pro = board(tmp_path, items)
    doc = cc.run(pcb, tmp_path / "out", pro)
    kk = {(r["type"], r["source"]) for r in doc["violations"]}
    assert ("track_width_rule", "rule") in kk and ("via_in_pad_rule", "rule") in kk and ("via_in_pad", "own") in kk
    assert (tmp_path / "out/copper-check.md").is_file()
    assert not (tmp_path / "t.kicad_dru").exists()   # never into the user's project
    r = next(r for r in doc["violations"] if r["type"] == "track_width_rule")
    assert r["items"][0]["pos"]["x"] == pytest.approx(2.5, abs=2.6) and r["net"] == "PWR1"


@pytest.mark.skipif(not (REAL.is_file() and PRO.is_file()), reason="qmtech user run not on this machine")
def test_recall_on_the_user_circled_spots(tmp_path):
    doc = cc.run(REAL, tmp_path, PRO, no_drc=False, spots=ROOT / "harness/copper_check_spots.json")
    miss = [s["id"] for s in doc["recall"]["spots"] if not s["found"]]
    assert not miss, miss


def test_neck_room_partial(tmp_path):
    items = [seg(0, 3, 5, 3, 0.2, "PWR2"), seg(0, 3.3, 5, 3.3, 0.2, "OTHER"), seg(0, 2.6, 5, 2.6, 0.2, "OTHER")]
    rows = cc.check_necks(cc.Ctx(cc.load_board(*board(tmp_path, items))))
    r = next(r for r in rows if r["net"] == "PWR2")
    assert r["type"] == "neck_room_partial" and 0.22 < r["corridor_fit_mm"] < 0.5


def test_pure_axis_offset_entry(tmp_path):
    pad = fp("U1", 0, 0, [("1", 0, 0, 90, 1.4, 0.3, "A")])   # vertical long axis, 0.3 wide
    rows = cc.check_pad_entries(cc.Ctx(cc.load_board(*board(tmp_path, [pad, seg(0.13, 0, 0.13, 3, 0.2, "A")]))))
    r = next(r for r in rows if r["type"] == "off_axis_entry")
    assert r["heading_dev_deg"] < 1 and r["axis_offset_mm"] == pytest.approx(0.13, abs=1e-3)


def test_t_junction_pieces_are_not_detours(tmp_path):
    pads = [fp("A", 0, 0, [("1", 0, 0, 0, 1, 1, "N")]), fp("B", 10, 0, [("1", 0, 0, 0, 1, 1, "N")]),
            fp("C", 5, 5, [("1", 0, 0, 0, 1, 1, "N")])]
    items = pads + [seg(0, 0, 10, 0, 0.2, "N"), seg(5, 0, 5, 5, 0.2, "N")]
    cx = cc.Ctx(cc.load_board(*board(tmp_path, items)))
    assert len(cx.conns["N"]) == 2 and cc.check_detours(cx, 1.3) == []   # the T joins (end on a track body) were split


def test_wrong_side_entry_from_the_body_side(tmp_path):
    pads = [("%d" % i, x, y, 0, 1.0, 0.3, "A%d" % i) for i, (x, y) in enumerate([(-2, -1), (-2, 0), (-2, 1), (2, -1), (2, 0), (2, 1)])]
    ic = fp("U1", 0, 0, pads)
    outer = cc.check_pad_entries(cc.Ctx(cc.load_board(*board(tmp_path, [ic, seg(2.5, -1, 5, -1, 0.2, "A3")]))))
    assert "wrong_side_entry" not in kinds(outer)
    straight_in = cc.check_pad_entries(cc.Ctx(cc.load_board(*board(tmp_path, [ic, seg(1.5, -1, -0.5, -1, 0.2, "A3")]))))
    r = [r for r in straight_in if r["type"] == "wrong_side_entry"]
    assert r and r[0]["severity"] == "info" and r[0]["minutes"] == 0.5          # straight out of the inner tip
    angled = cc.check_pad_entries(cc.Ctx(cc.load_board(*board(tmp_path, [ic, seg(1.5, -1, 0.5, 0, 0.2, "A3")]))))
    r = [r for r in angled if r["type"] == "wrong_side_entry"]
    assert r and r[0]["severity"] == "warning" and r[0]["minutes"] == 4.0
