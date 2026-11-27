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
                   "J1.1", "timeout_s = 5.0", "W1** (warning, accepted)", "converged", "applied to the board"):
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
    (run.refill_after_accept = false) and that the refill is a second undo step when it did."""
    from topor.report import RunReport, outcome_text

    rep = RunReport(tmp_path, {"zones_refilled": False})
    rep.set_outcome("accepted")
    line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    assert line.startswith(f"Outcome: {outcome_text('accepted')}") and "press B (Edit → Fill All Zones)" in line
    rep.facts["zones_refilled"] = True
    line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    assert "zones refilled" in line and "second undo step" in line
    for why, text in (("no_copy", "refilled copy of the result is missing"), ("other_items", "judged for other items"),
                      ("not_applied", "some zone fills could not be written")):
        rep.facts.update(zones_refilled=False, zone_fills_committed=0, zone_fills_skipped=why)
        line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
        assert "zone fills NOT written" in line and text in line and "press B" in line
    rep.facts.update(zone_fills_skipped="off")
    assert "zones not refilled: press B" in [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    rep.set_outcome("rejected")
    line = [ln for ln in rep.summary_lines() if ln.startswith("Outcome:")][0]
    assert line == f"Outcome: {outcome_text('rejected')}"


def test_report_settings_list_fab_floors(tmp_path):
    """F I-4: the design values the opt-in fab profile raised (toporlib.rules floors_applied) are in Settings."""
    rep = build_report(_result(), DRC, Config.defaults(), [], "whole board", {}, {}, dir=tmp_path,
                       facts={"fab_profile": "jlcpcb_1oz",
                              "floors_applied": [{"name": "min_track_width", "design": 150_000, "floor": 200_000},
                                                 {"name": "hole_clearance", "design": 250_000, "floor": 350_000}]})
    md = rep.to_markdown()
    settings = md.split("## Settings\n", 1)[1].split("\n## ", 1)[0]
    assert ('Fab floors raised ([rules] fab_profile = "jlcpcb_1oz", opt-in): min_track_width 0.15 → 0.2 mm, '
            "hole_clearance 0.25 → 0.35 mm") in settings, settings
    none = build_report(_result(), DRC, Config.defaults(), [], "whole board", {}, {}, dir=tmp_path,
                        facts={"fab_profile": "jlcpcb_1oz", "floors_applied": []}).to_markdown()
    assert 'Fab floors raised ([rules] fab_profile = "jlcpcb_1oz", opt-in): none' in none


def test_report_settings_board_rules_and_fallbacks(tmp_path):
    """Default (the board's rules): no floor line; every rule fallback is one Settings line (toporlib.rules)."""
    from toporlib.rules import RULE_FALLBACK, fallback_text

    t = fallback_text(RULE_FALLBACK["min_clearance"])
    md = build_report(_result(), DRC, Config.defaults(), [], "whole board", {}, {}, dir=tmp_path,
                      facts={"fab_profile": "board", "floors_applied": [], "rule_fallbacks": [t]}).to_markdown()
    settings = md.split("## Settings\n", 1)[1].split("\n## ", 1)[0]
    assert "Fab floors raised" not in settings and "Design rules: the board's KiCad configuration" in settings
    assert "Rule fallbacks (fields the board does not set): 1" in settings and f"- {t}" in settings
    assert "Board Setup → Design Rules → Constraints → Minimum clearance" in t and "0.15 mm used" in t
    md = build_report(_result(), DRC, Config.defaults(), [], "whole board", {}, {}, dir=tmp_path,
                      facts={"fab_profile": "board", "rule_fallbacks": []}).to_markdown()
    assert "Rule fallbacks (fields the board does not set): none" in md


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
         "necking": 12.5, "area_mm2": 210.0, "mm_per_via": None, "champion": ["fewest errors", "shortest"], "picked": True},
        {"label": "round 2 as drawn", "errors": 1, "vias": 26, "length_mm": 490.0, "shrinkages": 1, "warnings": 1,
         "area_mm2": 230.0, "mm_per_via": None, "champion": [], "picked": False}]
    res["moves"] = {"reroute": 3, "channel swap": 1}
    res["warnings"] = ["time budget hit: result may differ between runs (run.timeout_s = 5 s ...)"]
    rep = build_report(res, DRC, Config.defaults(), [], "whole board", None, {"router_s": 1.0}, dir=tmp_path)
    rep.set_outcome("accepted")
    rep.write()
    md = (tmp_path / "report.md").read_text()
    assert "## Variants\n" in md
    assert "| round 2 | 1 | 20 | 500.0 | 0 | 12.5 | 0 | 210.0 | - | fewest errors, shortest | yes |" in md
    assert "| round 2 as drawn |" in md and "reroute 3, channel swap 1" in md
    assert any("time budget hit" in ln for ln in rep.summary_lines())
    assert len(rep.summary_lines()) <= SUMMARY_MAX_LINES


def test_time_limit_text_names_a_dropped_cycle_even_with_no_wait():
    """limit_grace_s: a dropped TopoR loop cycle is in the text whatever the (rounded) wait is."""
    from topor.report import time_limit_text
    base = {"final_s": 60.0, "start_s": 60.0, "changes": [], "clock": "wall", "reached": True, "step": "TopoR loop on the topology",
            "mode": "soft", "overrun_s": 0.0, "grace_s": 0.0, "wait_s": 0.0}
    assert "dropped" in time_limit_text({**base, "cycle_dropped": True})
    assert "past it" in time_limit_text({**base, "wait_s": 2.5, "cycle_dropped": False})
    assert "that step ran" not in time_limit_text({**base, "cycle_dropped": False})


def test_time_limit_text_takes_any_grace_the_schema_accepts():
    """A None grace (limit_grace_s negative = unbounded) or a garbled one never raises; the cause is true for the mode."""
    from topor.report import grace_text, time_limit_text, wait_log_text
    base = {"final_s": 60.0, "start_s": 60.0, "changes": [], "clock": "wall", "reached": True, "step": "TopoR loop on the topology",
            "mode": "soft", "overrun_s": 1.0, "wait_s": 0.5, "cycle_dropped": True}
    for g in (None, -1, -5, "x", float("nan")):
        t = time_limit_text({**base, "grace_s": g, "cap_hit": True})
        assert "overrun cap" in t and "limit_grace_s = 0 s" not in t and "ran out" not in t, (g, t)
        assert "unbounded" in grace_text(g), g
        assert "limit_grace_s = 0 s" not in wait_log_text({**base, "grace_s": g}), g
    assert grace_text(30) == "run.limit_grace_s = 30 s" and grace_text(0.5) == "run.limit_grace_s = 0.5 s"
    # the grace is named as the cause only when it ran out
    hit = wait_log_text({**base, "grace_s": 30.0, "grace_hit": True})
    assert "run.limit_grace_s = 30 s ran out" in hit and "was dropped" in hit
    cap = wait_log_text({**base, "grace_s": 30.0, "grace_hit": False, "cap_hit": True})
    assert "ran out" not in cap and "overrun cap" in cap and "was dropped" in cap
    hard = wait_log_text({**base, "mode": "hard", "grace_s": 30.0})
    assert "timeout_mode = hard" in hard and "ran out" not in hard
    none = wait_log_text({**base, "cycle_dropped": False, "grace_s": 30.0})
    assert "no cycle was dropped" in none
    draw = wait_log_text({**base, "step": "drawing the traces", "cycle_dropped": False, "draw_grace_hit": True, "draw_grace_s": 60.0,
                          "draw_grace_erbs": 2})
    assert "run.drawing_grace_s = 60 s ran out" in draw and "every wire was drawn" in draw and "2 ERBS stretch" in draw
    assert "drawing_grace_s unbounded" in wait_log_text({**base, "draw_grace_hit": True, "draw_grace_s": None})
    assert "drawing_grace_s = 60 s ran out" in time_limit_text({**base, "cycle_dropped": False, "draw_grace_hit": True, "draw_grace_s": 60})


def test_draw_grace_text_drops_every_wire_drawn_when_the_cap_then_cut_the_traces():
    """verify-limit-wait r5 finding 1 (vf_lw5 s_both_g0_bound_first / s_both_g03_bound_first: the drawing bound ran out,
    then the overrun cap cut the drawing, routed 0/3): neither the log's wait line nor the report's limit phrase says
    'every wire was drawn'; a cut of the via reduction alone (cut 'drawing', cut_steps without 'drawing') still does."""
    from topor.report import draw_grace_text, time_limit_text, wait_log_text
    for g in (0.0, 0.3):
        tl = {"clock": "wall", "start_s": 5.0, "final_s": 5.0, "changes": [], "cap_s": 1.5, "mode": "soft", "reached": True,
              "step": "drawing the traces", "overrun_s": 2.0, "cap_hit": True, "cut": "drawing", "cut_steps": ["drawing"],
              "wait_s": 1.9, "cycle_dropped": False, "grace_s": 30.0, "grace_hit": False, "draw_grace_s": g,
              "draw_grace_hit": True, "draw_grace_erbs": 4}
        for t in (wait_log_text(tl), time_limit_text(tl), draw_grace_text(tl)):
            assert "every wire was drawn" not in t and "drawing_grace_s" in t and "cut the drawing of the traces" in t, (g, t)
        old = {**tl, "cut_steps": None}  # a record without cut_steps: the cut alone says it
        assert "every wire was drawn" not in draw_grace_text(old)
        via = {**tl, "cut_steps": ["via_reduction"]}
        assert "every wire was drawn" in draw_grace_text(via)
        free = {**tl, "cap_hit": False, "cut": "none", "cut_steps": []}
        assert "every wire was drawn" in draw_grace_text(free)


# ------------------------------------------------------------------ TW3: the four bests (verify tw3 r1 V3, DoD 13)
_ROW = {"roles": ["best", "errors", "shortest", "vias"], "drawn_roles": ["best", "errors", "shortest", "vias"],
        "row": "cycle 9 (gen 6)", "run": 2, "applied": True,
        "predicted": {"errors": 3, "vias": 31, "length_mm": 929.34},
        "drawn": {"errors": 24, "vias": 45, "length_mm": 998.71, "shorts": 6, "open": 0,
                   "necking": 52.5, "neck_mm": 32.5, "neck_vias": 2}}


def test_bests_summary_line_without_bests_is_empty():
    from topor.report import bests_summary_line
    assert bests_summary_line(None) == "" and bests_summary_line([]) == ""


def test_bests_summary_line_one_row_with_every_role_is_marked_applied():
    from topor.report import bests_summary_line
    line = bests_summary_line([_ROW])
    assert line == ("Four bests drawn (* = applied, [optimize] pick): "
                    "*best, errors, shortest, vias: 24 errors / 45 vias / 998.7 mm"), line


def test_bests_summary_line_several_rows_and_missing_drawn_fields():
    from topor.report import bests_summary_line
    other = {"roles": ["shortest"], "applied": False, "drawn": {"errors": 19}}
    bare = {}  # no roles, nothing drawn: every value n/a, never a crash
    line = bests_summary_line([{**_ROW, "roles": ["best", "errors", "vias"]}, other, bare])
    parts = line.split(": ", 1)[1].split(" · ")
    assert parts == ["*best, errors, vias: 24 errors / 45 vias / 998.7 mm / necking 52.5",
                     "shortest: 19 errors / n/a vias / n/a mm",
                     "-: n/a errors / n/a vias / n/a mm"], parts
    assert sum(x.startswith("*") for x in parts) == 1, "only the applied row is marked"


def test_bests_table_md_rows_predicted_drawn_and_applied():
    from topor.report import bests_table_md
    other = {"roles": ["shortest"], "drawn_roles": [], "row": "cycle 27 (gen 6)", "run": None,
             "predicted": {"errors": 0, "vias": 41, "length_mm": 865.6}, "drawn": None}
    rows = bests_table_md([_ROW, other])
    assert rows[0].startswith("| roles (table) | roles (drawn) |") and rows[1].startswith("|---|")
    assert rows[2] == ("| best, errors, shortest, vias | best, errors, shortest, vias | cycle 9 (gen 6) | 2 | "
                       "3 errors / 31 vias / 929.3 mm | 24 errors / 45 vias / 998.7 mm | 6 | 0 | 52.5 | 32.5 | 2 | yes |"), rows[2]
    assert rows[3] == ("| shortest |  | cycle 27 (gen 6) | n/a | 0 errors / 41 vias / 865.6 mm | "
                       "n/a errors / n/a vias / n/a mm | n/a | n/a | n/a | n/a | n/a |  |"), rows[3]
    assert len(rows) == 4 and bests_table_md([]) == rows[:2]


# ------------------------------------------------------------------ time-limit-fix: the Stopped line, one clock
TL_WALL = {"clock": "wall", "start_s": 3600.0, "final_s": 3600.0, "changes": [], "reached": False}


def _stopped(**facts):
    from topor.report import stopped_line
    return stopped_line(facts)


def test_stopped_line_names_the_share_and_the_limit_not_reached():
    """User 2026-11-22 (qmtech, 3600 s): the TopoR loop's share ended the run at 2357.5 s router time; the line said
    "Stopped: timeout_wall after 1 iterations, 2357.5 s, 1 threads ... limit 3600 s wall". Now: the share as the stop
    reason in words, the picked run's cycles, the optimizing time (the limit's clock) and the drawing apart, router
    time = both, the engine's threads, the limit as not reached with the seconds left, and the whole run apart."""
    line = _stopped(stop_reason="loop_time_share", iterations=9, elapsed_s=2357.5, optimize_s=1999.0, draw_s=358.5,
                    threads=8, router_cpu_s=11480.1, time_limit=TL_WALL, run_s=2400.8)
    assert line == ("Stopped: loop_time_share (the TopoR loop's own time share ended it, the limit was not reached) "
                    "after 9 cycles, optimizing 1999.0 s, drawing 358.5 s (after the limit, not counted), router time "
                    "2357.5 s, 8 threads, CPU 11480.1 s · limit 3600 s wall, not reached (1601 s left) · whole run "
                    "2400.8 s to the result (pre-flight and KiCad's DRC not counted)")


@pytest.mark.parametrize("stop,words", [
    ("timeout_wall", "the optimizing time limit was reached"),
    ("timeout_cpu", "the optimizing time limit was reached, CPU clock"),
    ("stable", "the routes were stable"),
    ("round cap", "the rounds safety cap ([optimize] rounds)"),
    ("cancel", "Accept / Cancel stopped it"),
    ("re-lay budget", "the re-lay work cap"),
])
def test_stopped_line_says_why_in_words_and_not_reached_only_for_the_share(stop, words):
    """Rule 13: every stop reason, in words (time-limit-fix r2: the limit reached versus the other stops); only the
    share's stop adds "not reached" (the limit fired, or nothing timed)."""
    line = _stopped(stop_reason=stop, iterations=3, elapsed_s=10.0, optimize_s=8.0, draw_s=2.0, threads=4,
                    time_limit=TL_WALL)
    assert line.startswith(f"Stopped: {stop} ({words}) after 3 cycles, optimizing 8.0 s, drawing 2.0 s (after the "
                           "limit, not counted), router time 10.0 s, 4 threads · limit 3600 s wall")
    assert "not reached" not in line and "whole run" not in line and "CPU" not in line.replace(words, "")


def test_stopped_line_on_the_cpu_clock_names_the_optimizing_cpu_seconds():
    line = _stopped(stop_reason="timeout_cpu", iterations=3, elapsed_s=700.0, optimize_s=600.0, optimize_cpu_s=2400.0,
                    draw_s=100.0, threads=4, router_cpu_s=2700.0, time_limit={**TL_WALL, "clock": "cpu"})
    assert "optimizing 600.0 s (CPU 2400.0 s, the limit's clock), drawing 100.0 s" in line
    assert ", CPU 2700.0 s · limit 3600 s cpu" in line


def test_stopped_line_without_a_limit_or_facts():
    # no limit (0 = none): the share's stop names no limit as not reached; no drawing figure: none shown
    assert _stopped(stop_reason="loop_time_share", iterations=2, elapsed_s=5.0, threads=1,
                    time_limit={**TL_WALL, "final_s": 0.0}) == (
        "Stopped: loop_time_share (the TopoR loop's own time share ended it, the limit was not reached) after 2 "
        "cycles, optimizing 5.0 s, router time 5.0 s, 1 threads · no time limit")
    # the limit reached (timeout_scope = all: a soft limit passed while the drawing ran after the share): no "not reached"
    assert "not reached (" not in _stopped(stop_reason="loop_time_share", iterations=2, elapsed_s=3700.0, threads=1,
                                           time_limit={**TL_WALL, "reached": True, "overrun_s": 100.0})
    # nothing known: n/a, never a crash
    assert _stopped() == "Stopped: n/a after n/a cycles, optimizing n/a s, router time n/a s, n/a threads"


def test_stopped_line_shows_measurements_apart_and_counts_them_out_of_the_left_time():
    """[capacity] compass_cut_check: its time moves the engine's limit later (not optimizing time): shown apart, and
    the seconds left of the limit are those of the optimizing time without it."""
    line = _stopped(stop_reason="loop_time_share", iterations=4, elapsed_s=150.0, optimize_s=130.0, draw_s=20.0,
                    threads=2, measure_s=30.0, time_limit={**TL_WALL, "final_s": 600.0})
    assert "optimizing 130.0 s (+30.0 s measurements, not counted), drawing 20.0 s" in line
    assert line.endswith("limit 600 s wall, not reached (500 s left)")


@pytest.mark.parametrize("share", [0.5, 0.05, 1.0, None])
def test_time_budget_line_for_the_share_names_it_not_the_limit(share):
    from topor.report import time_budget_line
    line = time_budget_line({"stop_reason": "loop_time_share", "loop_time_share": share, "time_limit": TL_WALL})
    want = 1.0 if share is None else share
    assert f"[optimize] loop_time_share = {want:g} of the time left when the loop began" in line
    assert "the optimizing time limit of 3600 s wall was not reached" in line and "stopped the topological core" not in line
    assert "default 1.0 = the loop runs to the limit" in line
    assert "the optimizing time limit was not reached" in time_budget_line({"stop_reason": "loop_time_share"})
    # the limit passed later (timeout_scope = all, in the drawing after the share): never called unreached
    late = time_budget_line({"stop_reason": "loop_time_share", "time_limit": {**TL_WALL, "reached": True}})
    assert "the optimizing time limit of 3600 s wall passed only later" in late and "not reached" not in late


@pytest.mark.parametrize("stop,clock", [("timeout_wall", "wall"), ("timeout_cpu", "cpu")])
def test_time_budget_line_for_the_limit_names_optimizing_time(stop, clock):
    from topor.report import time_budget_line
    line = time_budget_line({"stop_reason": stop, "timeout_clock": clock, "time_limit": {**TL_WALL, "clock": clock}})
    assert line == ("Warning: time budget hit: result may differ between runs (optimizing time limit 3600 s, "
                    f"run.timeout_s, clock {clock}, stopped the topological core before its [optimize] work caps; the "
                    "final drawing after it is not counted)")


def test_build_report_carries_the_separate_times_and_the_share(tmp_path):
    res = {**_result(), "stop_reason": "loop_time_share", "iterations": 9, "threads_used": 8,
           "warnings": ["time budget hit: result may differ between runs (the TopoR loop's own time share ...)"]}
    rep = build_report(res, DRC, Config.defaults(), [], "whole board", None,
                       {"router_s": 2357.46, "router_cpu_s": 11480.1, "measure_s": 0.0, "run_s": 2400.83,
                        "optimize_s": 1999.04, "optimize_cpu_s": 9800.0, "draw_s": 358.42},
                       dir=tmp_path, facts={"time_limit": TL_WALL})
    f = rep.facts
    assert (f["elapsed_s"], f["run_s"], f["threads"], f["iterations"], f["loop_time_share"]) == (2357.5, 2400.8, 8, 9, 1.0)
    assert (f["optimize_s"], f["optimize_cpu_s"], f["draw_s"]) == (1999.0, 9800.0, 358.4)
    assert "measure_s" not in f  # 0 = none: not shown
    lines = rep.summary_lines()
    assert any(ln.startswith("Stopped: loop_time_share (") and "after 9 cycles, optimizing 1999.0 s, drawing 358.4 s"
               in ln and "router time 2357.5 s, 8 threads" in ln for ln in lines)
    assert any("loop_time_share = 1 of the time left" in ln for ln in lines if ln.startswith("Warning: time budget"))
    assert len(lines) <= SUMMARY_MAX_LINES
