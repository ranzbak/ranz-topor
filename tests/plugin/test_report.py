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
