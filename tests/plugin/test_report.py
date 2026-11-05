"""B8 run report: sections, values, summary <= 20 lines, and the real flow on the eprom fixture."""
import pytest

from topor.config import Config
from topor.findings import Finding
from topor.report import SUMMARY_MAX_LINES, before_stats_from_model, build_report

MM = 1_000_000
SECTIONS = ["Summary", "Connections", "Vias", "Length", "DRC", "Findings", "Settings", "Files", "Versions"]


def _conn(i, status, name="N1", ghost=1000, length=1500, reason=None, blocking=None):
    return {"id": i, "name": name, "status": status, "reason": reason, "blocking": blocking,
            "from": {"kind": "pad", "label": f"U1.{i}", "at": [0, 0]},
            "to": {"kind": "pad", "label": f"U2.{i}", "at": [1, 1]},
            "length_nm": length if status == "routed" else None, "ghost_len_nm": ghost}


def _result(n_routed=3):
    conns = [_conn(i, "routed") for i in range(n_routed)]
    conns += [_conn(90, "partial", "VCC", reason="no path", blocking=[2 * MM, 3 * MM]),
              _conn(91, "failed", "GND", reason="blocked by keepout")]
    return {"connections": conns, "connections_total": len(conns), "routed": n_routed, "partial": 1, "failed": 1,
            "nets_total": 3, "vias": 2, "length_outside_pads_nm": 12 * MM, "stop_reason": "converged",
            "iterations": 7, "threads_used": 4,
            "nets": [{"name": "N1", "vias": 2, "length_nm": 13 * MM}],
            "zero_slack_wires": [{"conn": 1, "name": "N1"}],
            "items": [{"kind": "via", "net": 1, "at": [100, 100], "d": 600000}]}


DRC = {"routing_new": 2, "routing_preexisting": 5, "by_type_new": {"clearance": 2},
       "by_type_preexisting": {"shorting_items": 5},
       "incomplete": {"unconnected_new": 1, "unconnected_preexisting": 4},
       "unconnected_preexisting_items": [["a", "b"]]}


def test_report_sections_present_and_values(tmp_path):
    cfg = Config.defaults().with_values({("run", "timeout_s"): 5.0})
    model = {"nets": [{"code": 1, "name": "N1"}], "vias": [{"net": 1, "at": [0, 0]}] * 4,
             "kept": [{"net": 1, "prim": {"kind": "seg", "a": [0, 0], "b": [10 * MM, 0]}}],
             "pads": [{"net": 1, "ref": "U1", "number": "1", "center": [100, 100],
                       "shapes": {"0": {"w": 1000, "h": 1000}}}]}
    fnd = [Finding("W1", "warning", "careful", "do x"), Finding("I1", "info", "fyi")]
    rep = build_report(_result(), DRC, cfg, fnd, {"mode": "whole board", "ignored_pads": ["J1.1"],
                                                   "accepted_codes": ["W1"]},
                       before_stats_from_model(model), {"router_s": 3.14, "drc_rounds": 2}, dir=tmp_path)
    rep.set_outcome("accepted")
    rep.write(result={"x": 1})
    md = (tmp_path / "report.md").read_text()
    for s in SECTIONS:
        assert f"## {s}\n" in md
    f = rep.facts
    assert (f["routed"], f["total"], f["vias"], f["vias_before"], f["length_before_mm"]) == (3, 5, 2, 4, 10.0)
    assert f["partial_routes"] == ["VCC"] and f["zero_slack_wires"] == ["N1"] and f["vias_in_pads"] == ["U1.1"]
    for needle in ("no path", "2.000, 3.000 mm", "blocked by keepout", "clearance 2", "shorting_items 5",
                   "J1.1", "timeout_s = 5.0", "W1** (warning, accepted)", "converged", "applied as one undo step"):
        assert needle in md, needle
    assert "Detours" in "\n".join(rep.summary_lines())
    assert (tmp_path / "result.json").is_file()


def test_report_lists_exit_stubs_and_necks(tmp_path):
    """A6/NX: every used exit stub, necked ones separately (pad, length x width, net width, offset, bridge)."""
    res = _result()
    res["stubs"] = [
        {"pad": "U7.4", "width": 200_000, "net_width": 200_000, "length_nm": 617_500, "necked": False,
         "offset_nm": -50_000, "bridge": None, "used": True},
        {"pad": "U4.14", "width": 240_000, "net_width": 500_000, "length_nm": 867_500, "necked": True,
         "offset_nm": 0, "bridge": None, "used": True},
        {"pad": "U7.2", "width": 200_000, "net_width": 200_000, "length_nm": 775_000, "necked": False,
         "offset_nm": 50_000, "bridge": "U7.9", "used": True},
        {"pad": "J1.A6", "width": 300_000, "net_width": 500_000, "length_nm": 1_000_000, "necked": True,
         "offset_nm": 0, "bridge": None, "used": False},
    ]
    rep = build_report(res, DRC, Config.defaults(), [], "whole board", None, None, dir=tmp_path)
    f = rep.facts
    assert f["exit_stubs"] == ["U7.4 0.62 mm x 0.20 mm, 0.050 mm off centre",
                               "U4.14 0.87 mm x 0.24 mm (net 0.50 mm)",
                               "U7.2 0.78 mm x 0.20 mm, 0.050 mm off centre, to U7.9"]
    assert f["necked_stubs"] == ["U4.14 0.87 mm x 0.24 mm (net 0.50 mm)"]
    md = rep.to_markdown()
    assert "Necked stubs (narrower than the net width): U4.14 0.87 mm x 0.24 mm (net 0.50 mm)" in md
    rep2 = build_report(_result(), DRC, Config.defaults(), [], "whole board", None, None, dir=tmp_path)
    assert "Exit stubs: none" in rep2.to_markdown()


def test_report_outcome_line_says_how_zones_are_refilled(tmp_path):
    """Ruling C28: the summary's Outcome line after Accept tells the user to press B when topor did not refill
    (run.refill_after_accept = false, the default) and that the refill is a second undo step when it did."""
    from topor.report import RunReport, outcome_text

    rep = RunReport(tmp_path, {"zones_refilled": False})
    rep.set_outcome("accepted")
    line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    assert line.startswith(f"Outcome: {outcome_text('accepted')}") and "press B (Edit → Fill All Zones)" in line
    rep.facts["zones_refilled"] = True
    line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    assert "zones refilled" in line and "second undo step" in line
    rep.set_outcome("rejected")
    line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    assert line == f"Outcome: {outcome_text('rejected')}"


def test_report_settings_list_fab_floors(tmp_path):
    """F I-4: the design values the fab profile raised (toporlib.rules floors_applied) are in Settings."""
    rep = build_report(_result(), DRC, Config.defaults(), [], "whole board", {}, {}, dir=tmp_path,
                       facts={"floors_applied": [{"name": "min_track_width", "design": 150_000, "floor": 200_000},
                                                 {"name": "hole_clearance", "design": 250_000, "floor": 350_000}]})
    md = rep.to_markdown()
    settings = md.split("## Settings\n", 1)[1].split("\n## ", 1)[0]
    assert "Fab floors raised (JLCPCB 1 oz profile): min_track_width 0.15 → 0.2 mm, hole_clearance 0.25 → 0.35 mm" \
        in settings, settings
    none = build_report(_result(), DRC, Config.defaults(), [], "whole board", {}, {}, dir=tmp_path,
                        facts={"floors_applied": []}).to_markdown()
    assert "Fab floors raised (JLCPCB 1 oz profile): none" in none


def test_report_summary_lines_le_20(tmp_path):
    res = _result(2000)
    res["connections"] += [_conn(1000 + i, "failed", f"NET{i}", reason="x" * 200) for i in range(500)]
    fnd = [Finding(f"F{i}", ("error", "warning", "info")[i % 3], "m" * 300, "f") for i in range(300)]
    rep = build_report(res, DRC, Config.defaults(), fnd, "whole board", {}, {}, dir=tmp_path)
    assert 0 < len(rep.summary_lines()) <= SUMMARY_MAX_LINES
    empty = build_report({}, None, None, [], None, None, None, dir=tmp_path)
    assert len(empty.summary_lines()) <= SUMMARY_MAX_LINES
    rep.write()
    assert (tmp_path / "report.md").read_text().count("\n") > 500


def test_report_lists_the_variant_table_and_the_time_budget_warning(tmp_path):
    """S3: the topological core's Pareto archive goes into report.md (one row per variant, the pick marked),
    with the moves applied, and a time-budget stop is a summary warning."""
    res = _result()
    res["variants"] = [
        {"label": "round 2", "errors": 1, "vias": 20, "length_mm": 500.0, "shrinkages": 0, "warnings": 0,
         "area_mm2": 210.0, "mm_per_via": None, "champion": ["fewest errors", "shortest"], "picked": True},
        {"label": "round 2 as drawn", "errors": 1, "vias": 26, "length_mm": 490.0, "shrinkages": 1, "warnings": 1,
         "area_mm2": 230.0, "mm_per_via": None, "champion": [], "picked": False}]
    res["moves"] = {"reroute": 3, "channel swap": 1}
    res["warnings"] = ["time budget hit: result may differ between runs (run.timeout_s = 5 s ...)"]
    rep = build_report(res, DRC, Config.defaults(), [], "whole board", None, {"router_s": 1.0}, dir=tmp_path)
    rep.set_outcome("accepted")
    rep.write()
    md = (tmp_path / "report.md").read_text()
    assert "## Variants\n" in md
    assert "| round 2 | 1 | 20 | 500.0 | 0 | 0 | 210.0 | - | fewest errors, shortest | yes |" in md
    assert "| round 2 as drawn |" in md and "reroute 3, channel swap 1" in md
    assert any("time budget hit" in ln for ln in rep.summary_lines())
    assert len(rep.summary_lines()) <= SUMMARY_MAX_LINES
