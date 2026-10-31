import pcbnew

from harness.copy import ProjectCopy
from harness.fixtures import MM, make_fixture_board, write_project_file
from harness.model import export_model, load_board
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


def test_progress_m2_caption_and_path(engine, fixture_board, tmp_path):
    res = progress.render_progress("T9", board="fixture", date="2026-10-31",
                                   out_dir=tmp_path / "prog", pcb_override=fixture_board.pcb,
                                   router="topor")
    assert (tmp_path / "prog" / "2026-10-31-m2-T9.png").is_file()
    l1, l2 = res["caption"].split("\n")
    assert l1.startswith("T9: ") and " connections, " in l1 and "partial " in l1 and "arcs " in l1
    assert l2.startswith("DRC routing ") and " vs hand " in l2 and l2.endswith(" s")


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
    assert r["routed_drc"]["incomplete"] == {"track_dangling": 2, "via_dangling": base_vias,
                                             "unconnected_items": 1}
    assert r["routed_drc"]["routing_new"] == 0
    assert abs(r["router_report"]["connections"][0]["blocking"][1] / 1e6 - 24.0) <= 1.0
    assert r["route"]["partial"] == 1
