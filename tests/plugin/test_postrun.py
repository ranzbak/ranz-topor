"""Post-run findings (K1, plugin/topor/postrun.py): the engine report lines below are copied from real runs of the
topological core (qmtech / dcsgonebadge at 6eb1584), so a change of their wording breaks this test, not the user's
findings list silently."""
from topor import postrun
from topor.findings import Finding
from topor.messages import MESSAGES
from topor.preflight import _diff_pairs, _keepout_pads
from topor.config import Config
from topor.report import RunReport

QMTECH_WARNINGS = [
    "pad U9.25: no via site inside the pour's fill and no track into a fill on its own layer within 3 mm for a stub "
    "into its plane: joined to its net's copper by a signal connection instead",
    "VIN5V (F3.2): via in a narrow pass at (89.436, 109.773) mm: 0.300 mm of slack next to it (via 0.900 mm)",
    "SDRAM_A7 (IC1.32): via in a narrow pass at (159.291, 85.486) mm: 0.000 mm of slack next to it (via 0.800 mm)",
    "via_squeeze: 27 of 101 wire vias in a narrow pass (less slack than half the via diameter next to them)",
    "diff pairs skipped: 26 connections (diff pair: not in the rough version)",
    "VIN5V (U6.18): necked from 0.500 mm to 0.204 mm over 3.52 mm",
    "pour stitching: 13 pour pieces cut off by the routed copper before, 5 after; 6 island vias, 2 pad stubs (0.71 s)",
    "pour piece cut off: GND: 1 island(s) 0.76 mm², pads [C54.2], 0 via(s), near (157.025, 96.400)",
    "pad stub C57.2 -> (156.142, 92.224) (0.900 mm, via) for GND: 1 island(s) 1.15 mm², pads [C57.2], 0 via(s), "
    "near (157.025, 92.400)",
    "stitch via 0.8 mm at (95.277, 126.250) for GND: 1 island(s) 78.71 mm², pads [C30.2, SW2.2, J5.6], 0 via(s), "
    "near (93.077, 126.200)",
    "no stitch site for GND: 1 island(s) 0.76 mm², pads [C54.2], 0 via(s), near (157.025, 96.400)",
    "time budget hit: result may differ between runs (run.timeout_s = 60 s stopped the run before its work caps)",
    "ignored_rule: 3 track segments violate the copper edge clearance (check set to ignore in KiCad); priced by the "
    "ignored_rule factor",
]


def _c(name, status, reason="", a="U1.1", b="U2.1"):
    return {"name": name, "status": status, "reason": reason, "from": {"label": a}, "to": {"label": b}}


CONNS = [
    _c("ADV_TX0_P", "failed", "diff pair: not in the rough version"),
    _c("ADV_TX0_N", "failed", "diff pair: not in the rough version"),
    _c("SDA", "failed", "via 1 of 2: no site in its interval (no free disc for a via)"),
    _c("SCL", "failed", "drawing: a track grazes a pad or via of its own net (KiCad minimum connection width)"),
    _c("D0", "failed", "drawing: clearance gap 0.075 mm < 0.150 mm against pad:U1:2 on layer F.Cu"),
    _c("GND", "routed"),
]


def test_route_findings_from_real_engine_lines():
    fs = {f.code: f for f in postrun.route_findings({"connections": CONNS, "warnings": QMTECH_WARNINGS})}
    assert set(fs) <= set(postrun.POSTRUN_CODES) and set(fs) <= set(MESSAGES)
    d = fs["DIFF_PAIRS_SKIPPED"]
    assert d.severity == "warning" and d.items == ("ADV_TX0_N", "ADV_TX0_P") and "by hand" in d.message
    u = fs["UNROUTED"]
    assert u.message.startswith("3 of 6 connection(s)") and "1 no room for a via" in u.message
    assert "1 minimum connection width" in u.message and "1 no room at the clearance when drawn" in u.message
    assert fs["CONNECTION_WIDTH"].items == ("SCL U1.1 -> U2.1",)
    assert fs["POUR_ISLANDS"].message.startswith("5 piece(s)") and fs["POUR_ISLANDS"].items[0].startswith("GND:")
    assert fs["POUR_STITCHED"].severity == "info" and len(fs["POUR_STITCHED"].items) == 2
    assert "U9.25" in fs["PLANE_PAD_SIGNAL"].message
    assert fs["VIA_SQUEEZE"].message.startswith("27 of 101") and len(fs["VIA_SQUEEZE"].items) == 2
    assert fs["NECKED"].severity == "info" and len(fs["NECKED"].items) == 1
    assert fs["IGNORED_RULE"].message.startswith("3 track segments")
    assert fs["TIME_BUDGET_HIT"].severity == "warning"
    for f in fs.values():
        assert f.fix == MESSAGES[f.code].fix and ("KiCad →" in f.fix or "." in f.fix)


def test_route_findings_quiet_on_a_clean_run():
    res = {"connections": [_c("A", "routed")], "warnings": [
        "pour stitching: 0 pour pieces cut off by the routed copper before, 0 after; 0 island vias, 0 pad stubs (0.01 s)",
        "via_squeeze: 0 of 23 wire vias in a narrow pass (less slack than half the via diameter next to them)"]}
    assert postrun.route_findings(res) == []
    assert postrun.route_findings(None) == []


def test_summary_names_the_post_run_warnings(tmp_path):
    r = RunReport(tmp_path, {"findings": [Finding("UNROUTED", "warning", "m", "f", ("a", "b")),
                                          Finding("POUR_STITCHED", "info", "m", "f", ("x",)),
                                          Finding("POWER_BY_NAME", "info", "m", "f")]})
    lines = r.summary_lines()
    assert "After routing, check: UNROUTED (2) (report.md → Findings)" in lines
    md = r.to_markdown()
    assert "  - a" in md and "  - x" in md


def _pad(ref, num, net, at=(100, 100), layers=(0,)):
    return {"ref": ref, "number": num, "net": net, "center": list(at), "layers": list(layers)}


def test_preflight_diff_pairs_and_keepout_pads():
    model = {"nets": [{"code": 1, "name": "/USB/D+"}, {"code": 2, "name": "/USB/D-"}, {"code": 3, "name": "CLK_P"},
                      {"code": 4, "name": "LED"}],
             "pads": [_pad("U1", "1", 1), _pad("J1", "1", 1), _pad("U1", "2", 2), _pad("J1", "2", 2),
                      _pad("U1", "3", 3), _pad("U2", "3", 3), _pad("D1", "1", 4, (5, 5)), _pad("R1", "1", 4, (50, 50))],
             "keepouts": [{"layers": [0], "no_tracks": True, "poly": [[0, 0], [10, 0], [10, 10], [0, 10]]}]}
    cfg = Config.defaults()
    d = _diff_pairs(model, cfg)
    assert len(d) == 1 and d[0].items == ("/USB/D+", "/USB/D-") and d[0].message.startswith("1 differential pair")
    assert _diff_pairs(model, cfg.with_values({("nets", "do_not_touch"): ["/USB/*"]})) == []
    k = _keepout_pads(model)
    assert len(k) == 1 and k[0].items == ("D1.1",)


def test_preview_ratsnest_svg_draws_unconnected_items_blue():
    from topor.preview import RATSNEST_RGB, ratsnest_svg
    svg = '<svg width="10mm" height="10mm"></svg>'
    unc = [{"items": [{"pos": {"x": 101.0, "y": 52.0}}, {"pos": {"x": 103.5, "y": 54.0}}]}]
    out = ratsnest_svg(svg, unc, (100_000_000, 50_000_000))
    assert '<line x1="1.0000" y1="2.0000" x2="3.5000" y2="4.0000" />' in out
    assert out.count("<circle") == 2 and "#%02X%02X%02X" % RATSNEST_RGB in out and out.endswith("</svg>")
    assert ratsnest_svg(svg, [], (0, 0)) == svg


def test_preview_ratsnest_to_a_zone_is_a_ring_not_a_line_to_its_anchor():
    """K1 (live dcs preview): a pad missing its GND pour was drawn as a line to the zone's anchor, the board corner."""
    from topor.preview import ratsnest_svg
    unc = [{"items": [{"description": "Pad 0 [GND] of SW5 on F.Cu", "pos": {"x": 55.5, "y": 97.25}},
                      {"description": "Zone [GND] on F.Cu and B.Cu, priority 0", "pos": {"x": 50.0, "y": 50.0}}]}]
    out = ratsnest_svg("<svg></svg>", unc, (50_000_000, 50_000_000))
    assert "<line" not in out and '<circle cx="5.5000" cy="47.2500"' in out
