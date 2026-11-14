"""Diff-pair patterns (harness/diffpair_patterns.py): hand numbers agree with the geometry (build() runs check()),
the board files carry the pair class values, KiCad DRC passes on problem / reference / alternative."""
import json
import math

import pcbnew
import pytest

from harness import diffpair_patterns as dp
from harness.paths import kicad_cli


@pytest.fixture(scope="module")
def generated(tmp_path_factory):
    out = tmp_path_factory.mktemp("dp")
    return {n: dp.generate(n, out, png=False, drc=False) for n in dp.NAMES}


@pytest.mark.parametrize("name", dp.NAMES)
def test_checks(name):
    pat = dp.build(name)
    assert pat.expected["interpretation"]["built"]
    via = pat.extra.get("ref_via", (dp.VIA_D, dp.VIA_DRILL))  # dp_esd_flowthrough: qmtech's smallest allowed via
    assert all((v.dia, v.drill) == via for v in pat.ref.vias)


def test_parity_and_vias():
    assert dp.turn_backs(dp.build("dp_gateway").ref, "USB_P") == 0
    sw = dp.build("dp_swap_needed")
    assert len(sw.ref.vias) == 4 and dp.turn_backs(sw.ref, "USB_P") == dp.turn_backs(sw.ref, "USB_N") == 1
    ut = dp.build("dp_uturn_parity")
    assert len(ut.ref.vias) == 2 and dp.turn_backs(ut.ref, "USB_N") == 1
    nr = dp.build("dp_swap_no_room")
    assert nr.expected["cost"]["cost"] < nr.expected["price_table"]["S1_swap_elsewhere_4_vias (alternative panel)"]["cost"]


def test_project_file_values(generated):
    pro = json.loads(generated["dp_gateway"]["reference"].with_suffix(".kicad_pro").read_text())
    usb = next(c for c in pro["net_settings"]["classes"] if c["name"] == "USB")
    assert (usb["diff_pair_width"], usb["diff_pair_gap"], usb["diff_pair_via_gap"]) == (0.2, 0.15, 0.25)
    assert usb["track_width"] == 0.25 and usb["via_diameter"] == 0.6
    rules = pro["board"]["design_settings"]["rules"]
    assert rules["min_through_hole_diameter"] == 0.25 and rules["min_via_diameter"] == 0.45


def test_reference_board_read_back(generated):
    b = pcbnew.LoadBoard(str(generated["dp_swap_needed"]["reference"]))
    vias = [t for t in b.GetTracks() if t.Type() == pcbnew.PCB_VIA_T]
    assert len(vias) == 4 and all(v.GetDrillValue() == 250000 for v in vias)


@pytest.mark.skipif(not kicad_cli().exists(), reason="no kicad-cli")
@pytest.mark.parametrize("name", dp.NAMES)
def test_kicad_drc(name, generated):
    g = generated[name]
    pr = dp.drc_of(g["problem"])
    pat = dp.build(name)
    if name in dp.GND_PATS:  # the B.Cu GND pour is still empty (isolated copper); n GND pads = n - 1 unconnected
        n_gnd = sum(1 for c in pat.connections if c[0] == "GND")
        assert pr["types"] == ["isolated_copper"] and pr["unconnected"] == len(pat.connections) - 1
        assert n_gnd >= 2
    else:
        assert pr["violations"] == 0 and pr["unconnected"] == len(pat.connections)
    r = dp.drc_of(g["reference"])
    assert r["violations"] == 0 and r["unconnected"] == 0
    if "alternative" in g:
        r = dp.drc_of(g["alternative"])
        assert r["violations"] == 0 and r["unconnected"] == 0


def test_bus_crossing():
    pat = dp.build("dp_bus_crossing")  # check_bus: gap, skew, vias outside the bus, crossing angles, hand lengths
    assert len(pat.ref.vias) == 6 and len(pat.alt.vias) == 16
    assert not any(v.net.startswith("TX") for v in pat.ref.vias)
    assert all(t.layer == "F.Cu" for t in pat.ref.tracks if t.net.startswith("TX"))
    assert pat.expected["cost"]["pair_skew_max"] < 1e-6
    assert pat.expected["cost"]["cost"] < pat.expected["price_table"]["careless (pairs dive, 16 vias)"]["cost"]
    assert pat.expected["qmtech_grounding"]["copper_layers"] == 4


def test_gnd_between():
    pat = dp.build("dp_gnd_between")  # check_gnd: widths, vias, one stub + via per GND pad, gap, skew, clearances
    gnd_vias = [v for v in pat.ref.vias if v.net == "GND"]
    assert len(gnd_vias) == len(pat.ref.vias) == 2  # the pairs have no vias
    j1 = pat.fps[0]
    for v in gnd_vias:  # behind the pad: north of the GND pad's back end (pads exit south)
        pad = next(p for p in j1.pads if p.net == "GND" and abs(p.pos[0] - v.pos[0]) < 1e-9)
        assert abs(v.pos[1] - (pad.pos[1] - pad.size[1] / 2 - dp.GND_VIA_D)) < 1e-9
    c = pat.expected["cost"]
    assert c["uncoupled_pairs"] == 0 and c["pair_skew_max"] == 0
    assert pat.extra["d0"] == [(0.1, 0.05), (0.1, 0.05)]  # run-out past the GND pad corner / at the QFP
    careless = pat.expected["price_table"]["careless (GND vias in front, between P and N)"]
    assert careless["uncoupled_pairs"] > 1.5 and careless["cost"] > c["cost"]
    assert pat.expected["gnd"]["via_in_pad_margin"] < 0  # no via in the 0.3 pad at 0.5 pitch


def test_esd_flowthrough():
    pat = dp.build("dp_esd_flowthrough")  # check_gnd + finish_esd: hand values, DRC-free shapely check, prices
    # smallest via the qmtech Board Setup allows: drill = min through hole, diameter = max(min via, drill + 2 annular)
    assert (dp.QM_DRILL, dp.QM_VIA_D) == (0.2, 0.45) and math.pi * dp.QM_DRILL >= dp.GND_W
    assert len(pat.ref.vias) == 6 and all(v.net == "GND" and (v.dia, v.drill) == (0.45, 0.2) for v in pat.ref.vias)
    u2 = next(f for f in pat.fps if f.ref == "U2")
    yt, yb = pat.extra["yt"], pat.extra["yb"]
    for p in u2.pads:  # U2 vias between P and N on the inner side, 0.15 beyond the pad end
        if p.net == "GND":
            inner = 1 if abs(p.pos[1] - yt) < 1e-9 else -1
            v = min(pat.ref.vias, key=lambda v: math.dist(v.pos, p.pos))
            assert abs(v.pos[0] - p.pos[0]) < 1e-9
            assert abs(v.pos[1] - (p.pos[1] + inner * (p.size[1] / 2 + 0.15))) < 1e-9
            assert yt < v.pos[1] < yb
    g = pat.expected["gnd"]["fit_between_straight_tracks"]
    assert abs(g["margin"] - 0.025) < 1e-9 and g["with_clearance_0.2"] > g["have"]  # fits at 0.15, not at 0.2
    for k in range(2):  # the smallest via does not widen the pair
        for f in ("top_via_fan", "bottom_via_fan"):
            assert pat.extra["info"][k][f]["step_out"] == 0
            assert pat.extra["info_careless"][k][f]["step_out"] == 0.15  # the 0.8 class via does
    c = pat.expected["cost"]
    assert c["uncoupled_pairs"] == 0 and c["pair_skew_max"] == 0 and c["pair_vias"] == 0
    careless = next(v for k, v in pat.expected["price_table"].items() if k.startswith("careless"))
    assert careless["uncoupled_pairs"] > 5 and careless["cost"] > c["cost"] + 10
    assert pat.expected["lengths"]["body_between_rows"] > pat.extra["info_careless"][0]["body_between_rows"]
    assert pat.rules["min_via_diameter"] == 0.45 and pat.rules["min_through_hole_diameter"] == 0.2




@pytest.mark.parametrize("name", dp.NAMES)
def test_rules_assumed_pair_via_is_the_smallest(name, generated):
    """User 2026-11-12: a pair's vias are the smallest the board allows. The declaration (worked out by hand from the
    Board Setup minimums) names that via for every pair net, and the router's own resolution
    (toporlib.diffpair.smallest_via, `[diffpair] via = "smallest"`) arrives at the same size."""
    from toporlib.diffpair import smallest_via

    pat = dp.build(name)
    doc = json.loads(generated[name]["expected"].read_text())
    ra = doc["rules_assumed"]
    assert ra["authority"] == "board" and ra["margin"] == 0.0
    nm = lambda v: int(round(v * 1e6))  # noqa: E731
    R = pat.rules
    rules = {"min_via_diameter": nm(R["min_via_diameter"]), "min_via_drill": nm(R["min_through_hole_diameter"]),
             "min_annular": nm(R["min_via_annular_width"]), "predefined_vias": []}
    pair_cls = next(c for c in pat.classes if c.get("diff_pair_width"))
    d, drill, _src = smallest_via(rules, nm(pair_cls["diff_pair_width"]),
                                  (nm(pair_cls["via_diameter"]), nm(pair_cls["via_drill"])))
    assert ra["nets"] and all(v == {"vias": [[d / 1e6, drill / 1e6]]} for v in ra["nets"].values())
    # the class via is never the pair's via here (dp_esd_flowthrough: same 0.45 diameter, drill 0.2 < 0.3)
    assert (d, drill) < (nm(pair_cls["via_diameter"]), nm(pair_cls["via_drill"]))
    for v in pat.ref.vias:  # the reference pair vias are that via
        if v.net in ra["nets"]:
            assert (nm(v.dia), nm(v.drill)) == (d, drill)


@pytest.mark.parametrize("name", dp.NAMES)
def test_rules_assumed_single_net_small_vias_need_board_min(name):
    """A single net whose reference via is the smallest Board Setup via (not its net class via) can only be valid
    under ``[vias] size = "board_min"``: the board has no pre-defined vias, so the router's via list is the class
    via otherwise (the engine's hidden JLCPCB list that offered 0.45 / 0.25 is gone, 2026-11-12)."""
    from toporlib.diffpair import pair_nets

    pat = dp.build(name)
    pn = pair_nets(pat.nets)
    small = [v for v in pat.ref.vias if v.net not in pn]
    want = {"vias.size": "board_min"} if small else None
    assert dp.rules_assumed(pat).get("options") == want
