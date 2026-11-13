"""Post-run findings (K1, plugin/topor/postrun.py): the engine report lines below are copied from real runs of the
topological core (qmtech and a 2-layer badge at 6eb1584), so a change of their wording breaks this test, not the user's
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


def test_violations_finding_lists_place_type_and_cause():
    """M-B1 never drop: the engine's ``violations`` (engine/tests/topo_mb1.rs shape) become one VIOLATIONS
    warning, one item per violation."""
    viol = [{"conn": 0, "net": 1, "name": "SIG", "from": "A1.1", "to": "B1.1", "kind": "clearance", "layer": "F.Cu",
             "at": [14395833, 9931591], "depth_nm": 193758, "length_nm": 1500000, "against": "pad:WNF:1",
             "cause": "drawing: clearance gap 0.016 mm < 0.200 mm against pad:WNF:1 on layer F.Cu", "price_mm": 30063.7},
            {"conn": 3, "net": 2, "name": "SDA", "from": "U1.4", "to": "U2.7", "kind": "hole_clearance",
             "layer": "all layers", "at": [1000000, 2000000], "depth_nm": 50000, "length_nm": 600000,
             "against": "pad:U1:3", "cause": "via 1 of 2: no site in its interval (no free disc for a via)",
             "price_mm": 4000.0}]
    fs = {f.code: f for f in postrun.route_findings({"connections": [_c("SIG", "routed")], "warnings": [],
                                                    "violations": viol})}
    f = fs["VIOLATIONS"]
    assert f.severity == "warning" and f.message.startswith("2 rule violation(s) in the copper of 2 connection(s)")
    assert "1 clearance" in f.message and "1 hole_clearance" in f.message
    assert f.items[0] == ("SIG A1.1 -> B1.1: clearance at (14.396, 9.932) mm on F.Cu, 0.194 mm below the rule over "
                          "1.500 mm against pad:WNF:1; first failed check of the connection: drawing: clearance gap "
                          "0.016 mm < 0.200 mm against pad:WNF:1 on layer F.Cu")
    assert "VIOLATIONS" in postrun.POSTRUN_CODES and "VIOLATIONS" in MESSAGES
    assert "Cleanup Tracks" not in f.message, "no short, no cleanup note"


def test_violations_finding_says_what_kicad_cleanup_does_to_a_short():
    """qmtech 2026-11-11: KiCad's Cleanup Tracks & Vias removed the shorting tracks and then the rest of their
    connections as "unconnected at one end"; the finding names the nets with a short and says so."""
    viol = [{"conn": 0, "net": 7, "name": "SDRAM_A7", "from": "U4.U244", "to": "U2.3", "kind": "short",
             "layer": "B.Cu", "at": [1000000, 2000000], "depth_nm": 200000, "length_nm": 400000,
             "against": "track:SDRAM_A6", "cause": "drawing: crossing", "price_mm": 4000.0},
            {"conn": 1, "net": 8, "name": "SDRAM_A6", "from": "U4.U243", "to": "U2.4", "kind": "short",
             "layer": "B.Cu", "at": [1000000, 2000000], "depth_nm": 200000, "length_nm": 400000,
             "against": "track:SDRAM_A7", "cause": "drawing: crossing", "price_mm": 4000.0}]
    f = {f.code: f for f in postrun.route_findings({"connections": [_c("SDRAM_A7", "routed")], "warnings": [],
                                                   "violations": viol})}["VIOLATIONS"]
    assert "The 2 net(s) with a short (SDRAM_A6, SDRAM_A7)" in f.message
    assert "Delete tracks connecting different nets" in f.message and "unconnected at one end" in f.message


def test_island_join_is_a_stitch():
    """[planes] join_islands (user 2026-11-12): a pad-less island joined to a connected pad of its net."""
    res = {"connections": [_c("GND", "routed")], "warnings": [
        "pour stitching: 1 pour pieces cut off by the routed copper before, 0 after; 0 island vias, 0 pad stubs, "
        "1 island joins (0.01 s)",
        "pour piece cut off: GND: 1 island(s) 7.50 mm², pads [], 0 via(s), near (0.000, 0.000)",
        "island join R1.2 -> (8.600, 14.000) (2.600 mm) for GND: 1 island(s) 7.50 mm², pads [], 0 via(s), "
        "near (0.000, 0.000)"]}
    fs = {f.code: f for f in postrun.route_findings(res)}
    assert set(fs) == {"POUR_STITCHED"}
    assert "1 isolated island(s)" in fs["POUR_STITCHED"].message and fs["POUR_STITCHED"].items[0].startswith("island join R1.2")
    assert "check them by hand" not in fs["POUR_STITCHED"].message
    res["warnings"].append("long island join R1.2 -> (8.600, 14.000): 4.100 mm from the pad edge, over the 3.0 mm "
                           "scale (check it by hand)")
    msg = {f.code: f for f in postrun.route_findings(res)}["POUR_STITCHED"].message
    assert "1 of those join(s) run more than 3 mm from the pad" in msg


def test_route_findings_quiet_on_a_clean_run():
    res = {"connections": [_c("A", "routed")], "warnings": [
        "pour stitching: 0 pour pieces cut off by the routed copper before, 0 after; 0 island vias, 0 pad stubs, "
        "0 island joins (0.01 s)",
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
    assert _diff_pairs(model, Config.defaults()) == []  # D7: pairs are routed by default
    cfg = Config.defaults().with_values({("diffpair", "enabled"): False})
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
    """K1 (live preview): a pad missing its GND pour was drawn as a line to the zone's anchor, the board corner."""
    from topor.preview import ratsnest_svg
    unc = [{"items": [{"description": "Pad 1 [GND] of SW1 on F.Cu", "pos": {"x": 58.0, "y": 71.5}},
                      {"description": "Zone [GND] on F.Cu and B.Cu, priority 0", "pos": {"x": 50.0, "y": 50.0}}]}]
    out = ratsnest_svg("<svg></svg>", unc, (50_000_000, 50_000_000))
    assert "<line" not in out and '<circle cx="8.0000" cy="21.5000"' in out


# ------------------------------------------------------------------------- KiCad DRC of the result (user 2026-11-11)
# Real entries of KiCad's DRC of the eprom run where the engine was given the Default clearance 0.2 mm while KiCad
# required 0.25 mm (kicad-cli in a locale with a decimal comma; ec-runs/before, 20261111-124631/drc.json).
EPROM_DRC = [
    {"description": "Clearance violation ( clearance 0,2500 mm; actual 0,2163 mm)", "severity": "error",
     "type": "clearance", "items": [
         {"description": "Track [+5V] on B.Cu, length 4,3821 mm", "pos": {"x": 198.36347, "y": 62.558657},
          "uuid": "9833f7ab"},
         {"description": "PTH pad 3 [A13] of J1", "pos": {"x": 195.33, "y": 63.75}, "uuid": "9eb57d86"}]},
    {"description": "Clearance violation ( clearance 0,2500 mm; actual 0,2000 mm)", "severity": "error",
     "type": "clearance", "items": [
         {"description": "Track [A14] on B.Cu, length 4,7659 mm", "pos": {"x": 201.24, "y": 69.79}, "uuid": "f2d5b17c"},
         {"description": "Track [A13] on B.Cu, length 1,9036 mm", "pos": {"x": 199.4618, "y": 70.5518},
          "uuid": "f451990a"}]},
    {"description": "Clearance violation ( clearance 0,2500 mm; actual 0,2000 mm)", "severity": "error",
     "type": "clearance", "items": [
         {"description": "Track [A13] on B.Cu, length 5,8432 mm", "pos": {"x": 199.4618, "y": 68.648211},
          "uuid": "5b57d279"},
         {"description": "Track [A14] on B.Cu, length 4,7659 mm", "pos": {"x": 201.24, "y": 69.79}, "uuid": "f2d5b17c"}]},
]


def _eprom_model(clearance_nm):
    names = ["+5V", "A13", "A14"]
    return {"nets": [{"code": i + 1, "name": n} for i, n in enumerate(names)],
            "rules": {"default": {"clearance": clearance_nm},
                      "per_net": {str(i + 1): {"clearance": clearance_nm} for i in range(len(names))}},
            "pads": [], "pours": []}


def test_required_clearance_parses_decimal_comma_point_and_units():
    assert postrun.required_clearance_nm("Clearance violation ( clearance 0,2500 mm; actual 0,2163 mm)") == \
        (250_000, 216_300)
    assert postrun.required_clearance_nm("Clearance violation (netclass 'X' clearance 0.2000 mm; actual 0.1500 mm)") \
        == (200_000, 150_000)
    assert postrun.required_clearance_nm("Clearance violation ( clearance 10,0000 mils; actual 8,0000 mils)") == \
        (254_000, 203_200)
    assert postrun.required_clearance_nm("Track has unconnected end") is None


def test_rules_mismatch_eprom_engine_0_2_kicad_0_25():
    """The eprom case: the engine got 0.2 mm for +5V / A13 / A14, KiCad's DRC required 0.25 mm -> a loud error that
    names the nets and both values."""
    f = postrun.rules_mismatch(EPROM_DRC, _eprom_model(200_000))
    assert f is not None and f.code == "RULES_MISMATCH" and f.severity == "error"
    assert "topor read the design rules wrong — please report this" in f.message
    assert "+5V – A13: KiCad requires 0.2500 mm, topor routed with 0.2000 mm" in f.message
    assert "A13 – A14: KiCad requires 0.2500 mm, topor routed with 0.2000 mm (actual 0.2000 mm)" in f.message
    assert len(f.items) == 2  # one row per net pair, the worst violation of each
    # the engine had KiCad's value: a violation of the rule it knew is not a mismatch (DRC_NEW_VIOLATIONS covers it)
    assert postrun.rules_mismatch(EPROM_DRC, _eprom_model(250_000)) is None
    # a pad's own clearance override or a stricter net counts as what the engine was given
    m = _eprom_model(200_000)
    m["rules"]["per_net"]["2"]["clearance"] = 250_000  # A13
    assert postrun.rules_mismatch(EPROM_DRC, m) is None
    # the two nets of a differential pair: their gap is the clearance between them by design
    m = _eprom_model(200_000)
    m["rules"]["diff_pairs"] = [{"p_name": "A13", "n_name": "A14"}]
    f = postrun.rules_mismatch(EPROM_DRC, m)
    assert f is not None and [r.split(":")[0] for r in f.items] == ["+5V – A13"]
    # only clearance violations are checked
    assert postrun.rules_mismatch([{**EPROM_DRC[1], "type": "shorting_items"}], _eprom_model(200_000)) is None


def test_drc_new_violations_counts_types_and_places():
    """Costly, not failed: a warning with counts by type and the first places (nets, position in mm), whatever the
    reason; [] without new violations."""
    viol = EPROM_DRC + [{"description": "Items shorting two nets (nets A13 and A14)", "type": "shorting_items",
                         "severity": "error", "items": [
                             {"description": "Via [A13] on F.Cu - B.Cu", "pos": {"x": 1.0, "y": 2.0}, "uuid": "v1"},
                             {"description": "Track [A14] on F.Cu", "pos": {"x": 1.0, "y": 3.0}, "uuid": "t1"}]}]
    tracks = {"f2d5b17c": [(201.24, 69.79, 201.24, 75.0)], "f451990a": [(199.4618, 70.5518, 200.84, 70.5518)]}
    fs = postrun.drc_new_findings(viol, tracks)
    assert [f.code for f in fs] == ["DRC_NEW_VIOLATIONS"] and fs[0].severity == "warning"
    f = fs[0]
    assert f.message.startswith("KiCad's DRC finds 4 new violations in the result (clearance 3, shorting_items 1): ")
    assert "+5V – A13 at (196.847, 63.154) mm on B.Cu: clearance 0.2500 mm; actual 0.2163 mm" in f.message
    assert "… and 1 more" in f.message and len(f.items) == 4
    assert f.items[1].startswith("clearance A14 – A13 at (201.040, 70.552) mm on B.Cu")  # between the two tracks
    assert "Inspect → Design Rules Checker" in f.fix
    assert postrun.drc_new_findings([]) == [] and postrun.drc_new_findings(None) == []
