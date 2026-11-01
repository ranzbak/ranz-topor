import re
import subprocess

import pcbnew
import pytest

from harness.copy import ProjectCopy
from harness.fixtures import MM, make_fixture_board, write_project_file
from harness.model import export_model, load_board
from harness.render import render_png
from harness.rip import record_widths
from harness.rules import resolve_rules


def _export(pc):
    b = load_board(pc)
    rules = resolve_rules(b, dru_text=None, widths_before=record_widths(b))
    return export_model(b, rules, name="fx")


def _board(path, **kw):
    pcb = make_fixture_board(path, **kw)
    return ProjectCopy(pcb=pcb, pro=write_project_file(pcb, rules={}, classes=[]), dru=None, src_hashes={})


def _code(m, name):
    return {n["name"]: n["code"] for n in m["nets"]}[name]


def test_export_pours_lists_connected_pads_and_vias(fixture_board):
    m = _export(fixture_board)
    pours = m["pours"]
    assert len(pours) == 1 and pours[0]["net"] == _code(m, "GND")
    assert pours[0]["pads"] == ["U1.4"]
    assert len(pours[0]["vias"]) == 1
    assert pours[0]["layers"] == [int(pcbnew.B_Cu)]
    assert pours[0]["uuid"]


def test_export_no_zone_no_pours(tmp_path):
    pc = _board(tmp_path / "n.kicad_pcb", zone=False)
    assert _export(pc)["pours"] == []


def test_fixture_vcc_stub_is_locked_and_touches_pad1(tmp_path):
    m = _export(_board(tmp_path / "s.kicad_pcb", vcc_stub=True))
    vcc = _code(m, "VCC")
    stubs = [k for k in m["kept"] if k["net"] == vcc]
    assert len(stubs) == 1 and stubs[0]["locked"]
    assert stubs[0]["prim"]["a"] == [20 * MM, 20 * MM]
    assert stubs[0]["prim"]["b"] == [27 * MM, 24 * MM]
    assert stubs[0]["width"] == int(0.25 * MM) and stubs[0]["layer"] == int(pcbnew.F_Cu)
    base = _board(tmp_path / "b.kicad_pcb")
    assert not [k for k in _export(base)["kept"] if k["net"] == vcc]


# ---- T9 (M2): route_router, DRC spots, hand stats, progress ----

import json  # noqa: E402

from harness import bench, progress  # noqa: E402
from harness.drc import Violation  # noqa: E402
from harness.router import drc_spots, route_router  # noqa: E402

MM_ = 1_000_000


def _viol(type_, *items):
    return Violation(type=type_, severity="error", description="",
                     items=[{"description": d, "pos": {"x": x, "y": y}, "uuid": u}
                            for d, x, y, u in items])


def _trk(net, y_mm, width=250_000, layer=0):
    return {"net": net, "layer": layer, "width": width,
            "prim": {"kind": "seg", "a": [0, y_mm * MM_], "b": [10 * MM_, y_mm * MM_]}}


def test_route_router_on_fixture(engine, fixture_board, tmp_path):
    r = bench.run_bench("fixture", tmp_path, router=route_router, rip="all",
                        pcb_override=fixture_board.pcb)
    rep = r["router_report"]
    assert rep["connections_total"] >= 3 and r["routed_drc"]["routing_new"] == 0
    assert r["hand_stats"]["wires"] >= 1 and r["route"]["routed"] == rep["routed"]
    assert r["route"]["hand_outside_mm"] is not None


def test_route_router_on_b_pads_fixture(engine, tmp_path):
    """M3: SIG pads on B.Cu need a via to reach the F.Cu pads; the via lands in the board as a
    through PCB_VIA, the whole board routes and KiCad DRC finds nothing new."""
    pc = _board(tmp_path / "bp" / "fixture.kicad_pcb", b_pads=True)
    r = bench.run_bench("fixture", tmp_path / "out", router=route_router, rip="all",
                        pcb_override=pc.pcb)
    rep = r["router_report"]
    assert rep["routed"] == rep["connections_total"] and rep["failed"] == 0
    assert r["routed_drc"]["routing_new"] == 0
    assert r["route"]["vias"] >= 1 and rep["vias"] == r["route"]["vias"]
    assert set(r["route"]["length_by_layer_mm"]) >= {"B.Cu"}
    routed_pcb = next((tmp_path / "out" / "fixture-work").glob("*.kicad_pcb"))
    b = pcbnew.LoadBoard(str(routed_pcb))
    vias = [t for t in b.GetTracks() if t.Type() == pcbnew.PCB_VIA_T]
    assert any(v.GetViaType() == pcbnew.VIATYPE_THROUGH and v.GetNetname() == "SIG" for v in vias)


def test_drc_spots_from_violation_names_written_item():
    created = {"u1": _trk(3, 5, width=300_000)}
    model = {"nets": [{"code": 3, "name": "SIG"}]}
    v = _viol("clearance", ("Track [SIG] on F.Cu", 7.0, 5.0, "u1"), ("Pad 1 [X] of U1", 8.0, 5.2, "pad"))
    spots = drc_spots([v], created, model, 1_500_000, 200_000)
    assert spots == [{"layer": 0, "at": [7 * MM_, 5 * MM_], "r": 500_000, "net": 3}]


def test_drc_spots_zone_neck_uses_reach():
    created = {"u1": _trk(3, 5), "u2": _trk(4, 40)}
    model = {"nets": [{"code": 3, "name": "SIG"}, {"code": 4, "name": "OTHER"}, {"code": 5, "name": "GND"}]}
    v = _viol("connection_width", ("Via [GND] on F.Cu - B.Cu", 5.0, 5.5, "via"),
              ("Zone [GND] on F.Cu and B.Cu, priority 0", 50.0, 50.0, "zone"))
    spots = drc_spots([v], created, model, 1_500_000, 200_000)
    assert spots == [{"layer": 0, "at": [5 * MM_, int(5.5 * MM_)], "r": 1_500_000, "net": 3}]


def _fake_topor(model, options):
    net = next(n["code"] for n in model["nets"] if n["name"] == "SIG")
    item = {**_trk(net, 40, width=100_000), "wire": 0}
    item["prim"] = {"kind": "seg", "a": [40 * MM_, 40 * MM_], "b": [41 * MM_, 40 * MM_]}
    _fake_topor.seen.append(list(options.get("drc_spots") or []))
    return {"items": [item], "remove_uuids": [],
            "report": {"connections": [], "connections_total": 1, "routed": 1, "partial": 0,
                       "failed": 0, "nets_total": 1, "nets_routed": 1, "iterations": 3,
                       "clinches": {"regular": 0, "u": 0}}}


_fake_topor.seen = []


def test_bench_drc_loop_reroutes_with_spots(fixture_board, tmp_path):
    _fake_topor.seen.clear()
    r = bench.run_bench("fixture", tmp_path / "o", router=_fake_topor, rip="none",
                        pcb_override=fixture_board.pcb)
    # round 1 emits a too-thin track (new track_width violation naming it); round 2 carries a spot
    assert r["drc_loop"]["rounds"] == 2, r["drc_loop"]
    assert _fake_topor.seen[0] == [] and len(_fake_topor.seen[1]) == 1
    assert r["drc_loop"]["spots"] == _fake_topor.seen[1]


def test_progress_m3_caption_and_path(engine, fixture_board, tmp_path):
    res = progress.render_progress("T4", board="fixture", date="2026-10-31",
                                   out_dir=tmp_path / "prog", pcb_override=fixture_board.pcb,
                                   router="topor", milestone="m3")
    assert (tmp_path / "prog" / "2026-10-31-m3-T4.png").is_file()
    lines = res["caption"].split("\n")
    l1, l2 = lines[0], lines[1]
    assert l1.startswith("T4: ") and " connections, " in l1 and "partial " in l1
    assert "vias " in l1 and "arcs " in l1 and "iters " in l1
    # the input pcb sha12 ends line 1 (res carries the full sha256)
    assert l1.endswith(f", pcb {res['pcb_sha256'][:12]}")
    assert l2.startswith("DRC routing ") and " vs hand " in l2 and "B.Cu" in l2
    assert l2.endswith(" threads")
    # M3-R11: line 3 is the detour summary, present only when detour stats exist
    d = progress.detour_caption(res.get("detour"))
    tail = [x for x in lines[2:] if not x.startswith("stop_reason: ")]  # P1: optional last line
    assert tail == ([d] if d else [])
    if d:
        assert lines[2].startswith("detours > 1.5: ")


def test_export_pours_have_fill_outlines(fixture_board):
    m = _export(fixture_board)
    fills = m["pours"][0]["fills"]
    assert fills and fills[0]["layer"] == int(pcbnew.B_Cu)
    ring = fills[0]["outer"]
    assert len(ring) >= 4
    xs, ys = [p[0] for p in ring], [p[1] for p in ring]
    # the 50x40 mm zone outline is inset 1 mm; the fill stays inside it
    assert min(xs) >= 1 * MM and max(xs) <= 49 * MM and min(ys) >= 1 * MM and max(ys) <= 39 * MM


def test_fixture_b_pads_puts_sig_pads_on_b_cu(tmp_path):
    m = _export(_board(tmp_path / "b.kicad_pcb", b_pads=True))
    by = {p["number"]: p for p in m["pads"]}
    assert by["2"]["layers"] == [int(pcbnew.B_Cu)] and by["3"]["layers"] == [int(pcbnew.B_Cu)]
    assert by["5"]["layers"] == [int(pcbnew.F_Cu)]
    b = pcbnew.LoadBoard(str(tmp_path / "b.kicad_pcb"))
    p2 = next(p for p in b.GetPads() if p.GetNumber() == "2")
    assert p2.IsOnLayer(pcbnew.B_Cu) and not p2.IsOnLayer(pcbnew.F_Cu)


def test_fixture_pour_gap_pad_is_not_reached(tmp_path):
    m = _export(_board(tmp_path / "g.kicad_pcb", pour_gap_pad=True))
    p9 = next(p for p in m["pads"] if p["number"] == "9")
    assert p9["net"] == _code(m, "GND") and p9["layers"] == [int(pcbnew.F_Cu)]
    assert "U1.9" not in m["pours"][0]["pads"]


def test_drc_spots_via_item_spots_every_layer():
    via = {"kind": "via", "net": 3, "at": [7 * MM_, 5 * MM_], "d": 600_000, "drill": 300_000,
           "layers": [0, 31]}
    model = {"nets": [{"code": 3, "name": "SIG"}], "layers": [{"id": 0}, {"id": 31}]}
    v = _viol("clearance", ("Via [SIG] on F.Cu - B.Cu", 7.0, 5.0, "uv"),
              ("Pad 1 [X] of U1", 8.0, 5.2, "pad"))
    spots = drc_spots([v], {"uv": via}, model, 1_500_000, 200_000)
    assert spots == [{"layer": l, "at": [7 * MM_, 5 * MM_], "r": 800_000, "net": 3}
                     for l in (0, 31)]


def test_route_summary_has_vias_and_layer_lengths():
    seg = lambda layer, x: {"net": 1, "layer": layer, "width": 250_000,
                            "prim": {"kind": "seg", "a": [0, 0], "b": [x * MM_, 0]}}
    via = lambda x: {"kind": "via", "net": 1, "at": [x, 0], "d": 600_000, "layers": [0, 31]}
    rep = {"connections": [], "connections_total": 1, "routed": 1, "items": [
        seg(0, 3), seg(31, 4), via(0), via(1), {**seg(0, 0), "prim": {
            "kind": "arc", "start": [0, 0], "mid": [MM_, MM_], "end": [2 * MM_, 0]}}],
           "threads_used": 2}
    s = bench._route_summary(rep, [{"id": 0, "name": "F.Cu"}, {"id": 31, "name": "B.Cu"}])
    assert s["vias"] == 2 and s["threads_used"] == 2
    assert set(s["length_by_layer_mm"]) == {"F.Cu", "B.Cu"}
    assert s["length_by_layer_mm"]["B.Cu"] == 4.0
    assert s["length_by_layer_mm"]["F.Cu"] == pytest.approx(3.0 + 3.14159265, abs=1e-3)  # + half circle r=1


def test_render_b_cu_copper_is_distinct(tmp_path):
    def board(name, layer):
        pcb = make_fixture_board(tmp_path / name / "b.kicad_pcb", locked_track=False, arc=False,
                                 zone=False, pads=None, free_via=False, copper_text=False,
                                 npth=False)
        if layer is not None:
            b = pcbnew.LoadBoard(str(pcb))
            t = pcbnew.PCB_TRACK(b)
            t.SetStart(pcbnew.VECTOR2I(10 * MM, 20 * MM))
            t.SetEnd(pcbnew.VECTOR2I(40 * MM, 20 * MM))
            t.SetWidth(int(1.0 * MM))
            t.SetLayer(layer)
            b.Add(t)
            pcbnew.SaveBoard(str(pcb), b)
        return render_png(pcb, tmp_path / f"{name}.png")

    def rmse(a, b):
        out = subprocess.run(["compare", "-metric", "RMSE", str(a), str(b), "null:"],
                             capture_output=True, text=True).stderr
        return float(re.search(r"\(([0-9.e+-]+)\)", out).group(1))

    f, bk, none = board("f", pcbnew.F_Cu), board("b", pcbnew.B_Cu), board("n", None)
    assert rmse(f, bk) > 0.002 and rmse(f, none) > 0.002 and rmse(bk, none) > 0.002


def test_bench_summary_prints_partial_line(fixture_board, tmp_path, capsys):
    def router(model, options):
        return {"items": [], "remove_uuids": [],
                "report": {"connections": [], "connections_total": 4, "routed": 2, "partial": 2,
                           "failed": 0, "nets_total": 2, "nets_routed": 1, "iterations": 1,
                           "clinches": {"regular": 0, "u": 1}}}

    bench.run_bench("fixture", tmp_path / "o", router=router, rip="none",
                    pcb_override=fixture_board.pcb)
    out = capsys.readouterr().out
    assert "  partial=2" in out
    assert "  route: routed 2/4 connections, 1/2 nets, partial 2, failed 0" in out


# ---- T10 (M2): partial_routes ----


def _wall_pcb(tmp_path):
    return make_fixture_board(tmp_path / "src" / "wall.kicad_pcb", pads=("1", "6"), wall=True,
                              locked_track=False, arc=False)


def test_partial_routes_off_leaves_net_unconnected(engine, tmp_path):
    pcb = _wall_pcb(tmp_path)
    r = bench.run_bench("wall", tmp_path / "o", router=route_router, rip="all", pcb_override=pcb,
                        options={"partial_routes": False})
    assert r["routed_items"] == 0 and r["router_report"]["connections"][0]["status"] == "failed"
    assert r["routed_drc"]["incomplete"]["unconnected_items"] == 1 and r["routed_drc"]["routing_new"] == 0


def test_partial_routes_on_writes_grouped_stubs(engine, tmp_path):
    pcb = _wall_pcb(tmp_path)
    r = bench.run_bench("wall", tmp_path / "o", router=route_router, rip="all", pcb_override=pcb,
                        options={"partial_routes": True})
    assert r["routed_items"] >= 2
    b = pcbnew.LoadBoard(str(tmp_path / "o" / "wall-work" / "wall.kicad_pcb"))
    grp = [g for g in b.Groups() if g.GetName() == "topor-partial"]
    assert len(grp) == 1
    # the fixture's free GND via is dangling in the source board already (baseline, not new)
    base_vias = r["orig_drc"]["violations"].get("via_dangling", 0)
    inc = r["routed_drc"]["incomplete"]
    assert {k: inc[k] for k in ("track_dangling", "via_dangling", "unconnected_items")} == {
        "track_dangling": 2, "via_dangling": base_vias, "unconnected_items": 1}
    assert inc["unconnected_new"] + inc["unconnected_preexisting"] == 1
    assert r["routed_drc"]["routing_new"] == 0
    assert abs(r["router_report"]["connections"][0]["blocking"][1] / 1e6 - 24.0) <= 1.0
    assert r["route"]["partial"] == 1


def _fake_zero_slack(model, options):
    """R6 judge: with the retry on, a zero-slack wire that KiCad DRC rejects (too thin);
    with it off (second call), a clean wire and no zero-slack wire."""
    net = next(n["code"] for n in model["nets"] if n["name"] == "SIG")
    on = options.get("zero_slack_retry") is not False
    _fake_zero_slack.seen.append(on)
    width = 100_000 if on else 250_000
    item = {**_trk(net, 20, width=width), "wire": 0}
    item["prim"] = {"kind": "seg", "a": [30 * MM_, 20 * MM_], "b": [31 * MM_, 20 * MM_]}
    zs = [{"conn": 0, "net": net, "name": "SIG"}] if on else []
    return {"items": [item], "remove_uuids": [],
            "report": {"connections": [], "connections_total": 1, "routed": 1, "partial": 0,
                       "failed": 0, "nets_total": 1, "nets_routed": 1, "iterations": 3,
                       "clinches": {"regular": 0, "u": 0}, "zero_slack_wires": zs,
                       "zero_slack": "kept" if on else "off"}}


_fake_zero_slack.seen = []


def test_bench_zero_slack_rejected_by_drc_reverts(fixture_board, tmp_path):
    _fake_zero_slack.seen.clear()
    r = bench.run_bench("fixture", tmp_path / "o", router=_fake_zero_slack, rip="none",
                        pcb_override=fixture_board.pcb)
    # round 1: retry on, DRC flags the wire -> board restored, round 2 with the retry off
    assert _fake_zero_slack.seen == [True, False]
    assert r["zero_slack"] == {"status": "rejected by DRC",
                               "wires": [{"id": 0, "net": "SIG"}]}
    assert r["routed_drc"]["routing_new"] == 0
    assert r["route"]["zero_slack_wires"] == [] and r["route"]["zero_slack"] == "off"
    assert r["drc_loop"]["rounds"] == 2
    metrics = json.loads((tmp_path / "o" / "fixture--metrics.json").read_text())
    assert metrics["zero_slack"]["status"] == "rejected by DRC"



def test_progress_m4_name_and_caption_lines(engine, fixture_board, tmp_path):
    # default milestone is m4: <date>-m4-<task>-<board>.png; --options merge over board defaults
    res = progress.render_progress("A0", board="fixture", date="2026-11-01",
                                   out_dir=tmp_path / "prog", pcb_override=fixture_board.pcb,
                                   router="topor", options={"wall_clock_s": 30})
    assert (tmp_path / "prog" / "2026-11-01-m4-A0-fixture.png").is_file()
    lines = res["caption"].split("\n")
    stop = (res["router_report"] or {}).get("stop_reason")
    assert stop, "the engine reports a stop_reason"
    assert lines[-1] == f"stop_reason: {stop}"  # the last caption line
    assert lines[0].startswith("A0: ") and lines[1].startswith("DRC routing ")
    # bench columns of the result: detour (DRC-feasible reference, A7), detour_ghost, stop_reason and vias/net
    cols = res["columns"]
    assert set(cols) == {"detour", "detour_ghost", "stop_reason", "vias_per_net"}
    assert cols["stop_reason"] == stop
    assert {"median", "p90", "max", "count_gt_1_5"} <= set(cols["detour"]), cols["detour"]  # A7: filled, not None
    assert cols["detour"] == res["detour"] and cols["detour_ghost"] == res["detour_ghost"]  # A7: res["detour"] = reference
    assert cols["detour"]["field"] == "ref_len_nm" and cols["detour_ghost"]["field"] == "ghost_len_nm"


def test_progress_options_and_names_are_pure_helpers():
    assert progress.board_options("dcsgonebadge", "m4") == {"wall_clock_s": 600, "partial_routes": False}
    assert progress.board_options("dcsgonebadge", "m3") == {"wall_clock_s": 60, "partial_routes": False}
    assert progress.board_options("eprom-to-kernal", "m4") is None
    assert progress.merged_options("dcsgonebadge", "m4", {"wall_clock_s": 120}) == {
        "wall_clock_s": 120, "partial_routes": False}
    assert progress.merged_options("eprom-to-kernal", "m4", None) is None
    assert progress.merged_options("eprom-to-kernal", "m4", {"threads": 2}) == {"threads": 2}
    assert progress.progress_png_name("D", "m4", "A0", "wiccon") == "D-m4-A0-wiccon.png"
    assert progress.progress_png_name("D", "m3", "T4", "x") == "D-m3-T4.png"
    assert progress.progress_png_name("D", "m1", "T1") == "D-m1-T1.png"
    assert progress.progress_png_name("D", "m4", "A0") == "D-m4-A0.png"


def test_bench_dangling_fixture_g1(engine, tmp_path):
    """A14: the hand-started SIG track of the fixture ends in the open; the router continues it with the
    same width and layer, leaving the end within 1 degree of the hand direction, and keeps the hand copper."""
    pcb = make_fixture_board(tmp_path / "dg" / "fixture.kicad_pcb", dangling_trace=True, locked_track=False,
                             arc=False, zone=False)
    pro = write_project_file(pcb, rules={}, classes=[])
    pc = ProjectCopy(pcb=pcb, pro=pro, dru=None, src_hashes={})
    m = _export(pc)
    sig = _code(m, "SIG")
    assert [d for d in m["dangling_ends"] if d["net"] == sig and d["end"] == [22 * MM, int(26.5 * MM)]]
    r = bench.run_bench("fixture", tmp_path / "o", router=route_router, rip="none", pcb_override=pcb,
                        options={"wall_clock_s": 30})
    conns = [c for c in r["router_report"]["connections"] if c["from"]["kind"] == "dangling"]
    assert len(conns) == 1 and conns[0]["status"] == "routed", r["router_report"]["connections"]
    assert conns[0]["g1_angle_deg"] is not None and conns[0]["g1_angle_deg"] <= 1.0
    assert r["routed_drc"]["routing_new"] == 0
    # fixed mode (M3): no G1 promise
    rf = bench.run_bench("fixture", tmp_path / "of", router=route_router, rip="none", pcb_override=pcb,
                         options={"wall_clock_s": 30, "dangling_ends": "fixed"})
    cf = [c for c in rf["router_report"]["connections"] if "dangling" in (c["from"]["kind"], c["to"]["kind"])]
    assert cf and cf[0]["g1_angle_deg"] is None
