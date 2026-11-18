import json
from pathlib import Path

import pcbnew
import pytest

from harness.copy import assert_unchanged, copy_project
from harness.fixtures import make_fixture_board, write_project_file
from harness.paths import BENCH_BOARDS
from harness.model import export_model, find_dangling_ends, load_board, write_model_json
from harness.rip import rip_tracks



def _codes(m):
    return {n["name"]: n["code"] for n in m["nets"]}


def test_export_fixture_counts(fixture_board):
    m = export_model(load_board(fixture_board), {}, name="fx")
    assert len(m["pads"]) == 7 and len(m["nets"]) == 3 and [l["name"] for l in m["layers"]] == ["F.Cu", "B.Cu"]
    assert all(l["routable"] for l in m["layers"])
    pad2 = next(p for p in m["pads"] if p["number"] == "2")
    assert list(pad2["shapes"]) == ["0"] and pad2["shapes"]["0"] == {"kind": "rect", "w": 1_000_000, "h": 2_000_000}
    pad1 = next(p for p in m["pads"] if p["number"] == "1")
    assert set(pad1["shapes"]) == {"0", "2"} and pad1["drill"] == {"w": 800_000, "h": 800_000}
    assert sum(1 for h in m["holes"] if h["kind"] == "npth") == 1
    assert any(o["kind"] == "arc" for o in m["outline"]) and len(m["outline"]) >= 4 + 8
    assert any(cs["net"] == 0 for cs in m["copper_shapes"])
    assert len(m["kept"]) == 2 and any(k["locked"] for k in m["kept"])
    assert len(m["vias"]) == 1 and m["vias"][0]["free"]
    # the fixture's Edge.Cuts strokes are 0.1 mm (exported, informational: KiCad 10 ignores the stroke for copper-to-edge)
    assert m["rules"]["outline_half_stroke"] == 50_000
    json.dumps(m)


def test_pad_shape_kinds_and_slot(fixture_board):
    m = export_model(load_board(fixture_board), {}, name="fx")
    by = {p["number"]: p for p in m["pads"]}
    assert by["3"]["shapes"]["0"]["kind"] == "roundrect" and by["3"]["shapes"]["0"]["rc"] == 250_000
    assert by["4"]["shapes"]["0"]["kind"] == "oval"
    assert by["5"]["shapes"]["0"]["kind"] == "poly" and len(by["5"]["shapes"]["0"]["pts"]) == 4
    assert by["6"]["rotation_deg"] == pytest.approx(30.0)
    slot = next(h for h in m["holes"] if h.get("slot"))
    assert slot["d"] == 1_200_000 and slot["slot"] == [600_000, 1_200_000] and slot["kind"] == "pth"


def test_model_net_tie_groups(tmp_path):
    pcb = make_fixture_board(tmp_path / "t.kicad_pcb", net_tie=True)
    write_project_file(pcb, rules={}, classes=[])
    m = export_model(load_board(copy_project(pcb, tmp_path / "w")), {}, name="t")
    c = _codes(m)
    assert m["net_ties"] == [{"ref": "NT1", "groups": [["1", "2"]], "nets": [[c["VCC"], c["SIG"]]]}]
    assert {p["net_tie_group"] for p in m["pads"] if p["ref"] == "NT1"} == {0}
    assert {p["net_tie_group"] for p in m["pads"] if p["ref"] == "U1"} == {None}


def test_model_power_layer_not_routable(fixture_board):
    b = load_board(fixture_board)
    b.SetLayerType(pcbnew.B_Cu, pcbnew.LT_POWER)
    m = export_model(b, {}, name="p")
    assert [(l["kind"], l["routable"]) for l in m["layers"]] == [("signal", True), ("power", False)]


def _half(tmp_path):
    pcb = make_fixture_board(tmp_path / "h.kicad_pcb", locked_track=False, arc=False, half_track=True)
    write_project_file(pcb, rules={}, classes=[])
    return load_board(copy_project(pcb, tmp_path / "w"))


def test_dangling_end_detector_half_drawn_track(tmp_path):
    b = _half(tmp_path)
    ends = find_dangling_ends(b)
    m = export_model(b, {}, name="h")
    assert len(ends) == 1 and ends[0]["net"] == _codes(m)["SIG"] and ends[0]["layer"] == 0
    assert ends[0]["width"] == 250_000
    assert ends[0]["end"] == [25_000_000, 28_000_000] and ends[0]["direction"] == pytest.approx([0.0, 1.0])
    assert m["dangling_ends"] == ends


def test_dangling_end_none_when_track_ends_on_pad(tmp_path):
    b = _half(tmp_path)
    for t in list(b.GetTracks()):
        b.Remove(t)
    t = pcbnew.PCB_TRACK(b)
    t.SetStart(pcbnew.VECTOR2I(25_000_000, 20_000_000))
    t.SetEnd(pcbnew.VECTOR2I(30_000_000, 20_000_000))  # pad 2 -> pad 3, both SIG
    t.SetWidth(250_000)
    t.SetLayer(pcbnew.F_Cu)
    t.SetNet(b.GetNetsByName()["SIG"])
    b.Add(t)
    assert find_dangling_ends(b) == []


def test_dangling_arc_tangent_direction(fixture_board):
    ends = find_dangling_ends(load_board(fixture_board))
    # arc (10,10)-(12,11)-(14,10) bulges toward +y: free tangent at its start points to -x,-y
    s = next(e for e in ends if e["end"] == [10_000_000, 10_000_000])
    assert s["direction"][0] < 0 and s["direction"][1] < 0
    assert sum(d * d for d in s["direction"]) == pytest.approx(1.0)


def test_kept_items_carry_net_and_locked(fixture_board):
    b = load_board(fixture_board)
    rip_tracks(b, mode="none")
    m = export_model(b, {}, name="k")
    assert len(m["kept"]) == 2 and len(m["vias"]) == 1
    assert all("net" in k and "locked" in k for k in m["kept"])
    assert {k["prim"]["kind"] for k in m["kept"]} == {"seg", "arc"}


def test_write_model_json(fixture_board, tmp_path):
    m = export_model(load_board(fixture_board), {}, name="fx")
    p = write_model_json(m, tmp_path / "m.json")
    assert json.loads(p.read_text())["board"] == "fx"


def test_eprom_gnd_pour_fill_outlines_via_swig(tmp_path):
    """Pins the SWIG names of the filled-polygon export (GetFilledPolysList, OutlineCount, Outline,
    PointCount, CPoint) against a copy of a real board with filled GND pours."""
    pc = copy_project(BENCH_BOARDS["eprom-to-kernal"], tmp_path / "w")
    m = export_model(load_board(pc), {}, name="eprom")
    gnd = _codes(m)["GND"]
    pours = [p for p in m["pours"] if p["net"] == gnd]
    assert pours and all("warning" not in p for p in pours)
    fills = [f for p in pours for f in p["fills"]]
    assert fills and {f["layer"] for f in fills} <= set(pours[0]["layers"])
    assert all(len(f["outer"]) >= 4 for f in fills)
    assert sum(len(f["outer"]) for f in fills) > 1000  # real fills are detailed polygons


@pytest.mark.gate
def test_export_eprom_counts(tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]
    pc = copy_project(src, tmp_path / "w")
    m = export_model(load_board(pc), {}, name="eprom")
    assert_unchanged(src.parent, pc.src_hashes)
    # edit-proof: expectations come from pcbnew on the same copy, not from pinned numbers
    from harness.measure import copper_facts

    b0 = load_board(pc)
    facts = copper_facts(b0)
    assert len(m["pads"]) == sum(len(fp.Pads()) for fp in b0.GetFootprints())
    assert len(m["nets"]) == len({str(n.GetNetname()) for _, n in b0.GetNetsByNetcode().items() if n.GetNetname()})
    assert len(m["kept"]) == facts["segments"] + facts["arcs"] and len(m["vias"]) == facts["vias"]
    assert sum(1 for v in m["vias"] if v["free"]) == facts["free_vias"]
    got = {}
    for k in m["kept"]:
        got[k["width"]] = got.get(k["width"], 0) + 1
    assert got == facts["widths"]
    # 5 footprints on the board, but two are pad-less "REF**" graphics, so pads name 3 refs
    assert {p["ref"] for p in m["pads"]} == {fp.GetReference() for fp in b0.GetFootprints() if len(fp.Pads())}
    assert [l["name"] for l in m["layers"]] == ["F.Cu", "B.Cu"]


def test_footprint_rule_area_exported_as_keepout(fixture_board):
    """T9c: a rule area owned by a footprint (e.g. a module's antenna keepout) is a keepout too."""
    b = load_board(fixture_board)
    fp = next(iter(b.GetFootprints()))
    z = pcbnew.ZONE(fp)
    z.SetIsRuleArea(True)
    z.SetDoNotAllowTracks(True)
    z.SetDoNotAllowVias(False)
    z.SetLayer(pcbnew.F_Cu)
    z.SetZoneName("antenna keepout")
    ol = pcbnew.SHAPE_LINE_CHAIN()
    for x, y in ((2, 2), (4, 2), (4, 4), (2, 4)):
        ol.Append(pcbnew.VECTOR2I(int(x * 1e6), int(y * 1e6)))
    ol.SetClosed(True)
    z.AddPolygon(ol)
    fp.Add(z)
    m = export_model(b, {}, name="fx")
    ko = [k for k in m["keepouts"] if k["name"] == "antenna keepout"]
    assert len(ko) == 1 and ko[0]["layers"] == [int(pcbnew.F_Cu)]
    assert ko[0]["no_tracks"] and not ko[0]["no_vias"] and len(ko[0]["poly"]) == 4


def test_model_pours_schema(fixture_board):
    m = export_model(load_board(fixture_board), {}, name="fx")
    assert len(m["pours"]) == 1
    z = m["pours"][0]
    assert set(z) == {"uuid", "net", "layers", "pads", "vias", "fills", "clearance",
                      "min_thickness", "thermal_gap", "spoke_width", "pad_connection",
                      "island_removal", "min_island_area"}
    assert z["pads"] == sorted(z["pads"]) and z["vias"] == sorted(z["vias"])
    assert z["min_thickness"] > 0 and z["thermal_gap"] > 0 and z["spoke_width"] > 0
    assert z["pad_connection"] in (0, 1, 2, 3)
    assert all(set(f) == {"layer", "outer", "holes"} for f in z["fills"])
    pads = [p for p in m["pads"] if p["net"] == z["net"]]
    assert pads and all("zone_connection" not in p and p["spoke_angle_deg"] in (45.0, 90.0)
                        for p in pads)


def test_model_pours_exclude_teardrop_zones(tmp_path):
    from harness.fixtures import add_teardrop_zone

    pcb = make_fixture_board(tmp_path / "td.kicad_pcb", half_track=True)
    add_teardrop_zone(pcb, "SIG", "F.Cu", [(24, 19), (26, 19), (26, 29), (24, 29)])
    b = pcbnew.LoadBoard(str(pcb))
    assert sum(1 for z in b.Zones() if z.IsTeardropArea()) == 1
    m = export_model(b, {}, name="fx")
    assert len(m["pours"]) == 1 and m["pours"][0]["layers"] == [int(pcbnew.B_Cu)]


def test_net_codes_match_plugin_ipc_numbering(tmp_path):
    """User 2026-11-17 "match KiCad": the harness numbers nets as the plugin does through IPC
    (``toporlib.rules.net_codes_by_name``: sorted names, 1-based, the table model_ipc.export_model_ipc and the
    rule resolver use), not by pcbnew's file order. Pads, kept copper and vias carry the code of their net NAME."""
    from harness.rules import _swig_inputs
    from toporlib.rules import net_codes_by_name

    pc = copy_project(BENCH_BOARDS["eprom-to-kernal"], tmp_path / "w")
    b = load_board(pc)
    m = export_model(b, {}, name="eprom")
    names = [str(n.GetNetname()) for c, n in b.GetNetsByNetcode().items() if int(c) != 0]
    want = net_codes_by_name(names)
    assert _codes(m) == want and [n["name"] for n in m["nets"]] == sorted(names)
    assert _swig_inputs(b)[3] == want  # the rule resolver's per_net keys use the same table
    pcb_order = [str(n.GetNetname()) for c, n in sorted(b.GetNetsByNetcode().items()) if int(c) != 0]
    assert pcb_order != sorted(pcb_order)  # the board really is in file order (else this proves nothing)
    for p in m["pads"]:
        pad = next(q for f in b.GetFootprints() if f.GetReference() == p["ref"] for q in f.Pads()
                   if q.GetNumber() == p["number"])
        assert p["net"] == want.get(str(pad.GetNetname()), 0)
    by_uuid = {str(t.m_Uuid.AsString()): t for t in b.GetTracks()}
    for k in m["kept"] + m["vias"]:
        assert k["net"] == want[str(by_uuid[k["uuid"]].GetNetname())]


def test_writer_maps_engine_codes_back_to_pcbnew(tmp_path):
    """write_items takes engine (IPC) net codes and sets the pcbnew code of the same NAME."""
    from harness.model import engine_net_codes
    from harness.writer import write_items

    b = load_board(copy_project(BENCH_BOARDS["eprom-to-kernal"], tmp_path / "w"))
    eng = engine_net_codes(b)
    name = sorted(eng)[3]
    seg = {"kind": "seg", "a": [0, 0], "b": [1_000_000, 0]}
    write_items(b, [{"net": eng[name], "layer": 0, "width": 250_000, "prim": seg}])
    t = [t for t in b.GetTracks() if t.GetStart().x == 0 and t.GetEnd().x == 1_000_000][0]
    assert str(t.GetNetname()) == name


def _non_sig_codes(b):
    """(engine code, pcbnew code) of GND: they differ on the fixture (engine GND 1, pcbnew GND 3), SIG is 2 in both."""
    from harness.model import engine_net_codes
    eng, pcb = engine_net_codes(b)["GND"], b.GetNetcodeFromNetname("GND")
    assert eng != pcb
    return eng, pcb


def test_dangling_ends_carry_engine_code_of_non_sig_net(tmp_path):
    b = _half(tmp_path)
    eng, _ = _non_sig_codes(b)
    for t in b.GetTracks():
        t.SetNet(b.GetNetsByName()["GND"])
    ends = find_dangling_ends(b)
    assert ends and {e["net"] for e in ends} == {eng}


def test_find_dangling_ends_standalone_ignores_stale_table(tmp_path):
    """The code table is refreshed per call: an export of another board must not leak into a later standalone call."""
    from harness import model
    b = _half(tmp_path)
    eng, _ = _non_sig_codes(b)
    for t in b.GetTracks():
        t.SetNet(b.GetNetsByName()["GND"])
    model._CODES.clear()
    model._CODES.update({"GND": 99, "SIG": 98, "VCC": 97})
    ends = find_dangling_ends(b)
    assert ends and {e["net"] for e in ends} == {eng}


def test_teardrops_and_copper_shapes_carry_engine_code(fixture_board):
    b = load_board(fixture_board)
    eng, pcb = _non_sig_codes(b)
    from harness.writer import save
    from tests.test_smoother import _td_zone
    path = save(b, fixture_board.pcb.parent / "td_nc.kicad_pcb")
    path.write_text(path.read_text().rstrip()[:-1] + _td_zone("GND", "F.Cu", 1, 1, 3, 2) + ")\n")
    b = load_board(type("P", (), {"pcb": path, "pro": None})())
    r = pcbnew.PCB_SHAPE(b)
    r.SetShape(pcbnew.SHAPE_T_RECT)
    r.SetStart(pcbnew.VECTOR2I(30_000_000, 5_000_000))
    r.SetEnd(pcbnew.VECTOR2I(32_000_000, 6_000_000))
    r.SetLayer(pcbnew.F_Cu)
    r.SetFilled(True)
    r.SetNet(b.GetNetsByName()["GND"])
    b.Add(r)
    m = export_model(b, {}, name="td")
    assert [t["net"] for t in m["teardrops"]] == [eng]
    mine = [c for c in m["copper_shapes"] if c["uuid"] == str(r.m_Uuid.AsString())]
    assert mine and all(c["net"] == eng for c in mine)


def test_footprint_clearance_override_inherited_by_pads(tmp_path):
    """KiCad PAD::GetClearanceOverrides (pcbnew/pad.cpp:1675-1681): pad override, else the footprint's. The
    export used the pad's only, so the engine checked the net class clearance (bornhack U6, F-6)."""
    pcb = make_fixture_board(tmp_path / "fx.kicad_pcb", fp_clearance_mm=0.127, pad_clearance_mm=0.3)
    m = export_model(pcbnew.LoadBoard(str(pcb)), {}, name="fx")
    lc = {p["number"]: p["local_clearance"] for p in m["pads"] if p["ref"] == "U1" and p["number"]}
    assert lc["3"] == 300_000  # own override wins
    assert {lc[n] for n in ("1", "2", "4", "5", "6")} == {127_000}  # inherited from the footprint
    nt = make_fixture_board(tmp_path / "nt.kicad_pcb", net_tie=True)
    assert all(p["local_clearance"] is None for p in export_model(pcbnew.LoadBoard(str(nt)), {}, name="x")["pads"])


def test_footprint_clearance_pad_explicit_zero_wins(tmp_path):
    """A pad's own explicit 0 is an override (has_value in KiCad), not "unset": it must not inherit the footprint's."""
    pcb = make_fixture_board(tmp_path / "z.kicad_pcb", fp_clearance_mm=0.127, pad_clearance_mm=0.0)
    m = export_model(pcbnew.LoadBoard(str(pcb)), {}, name="fx")
    lc = {p["number"]: p["local_clearance"] for p in m["pads"] if p["ref"] == "U1" and p["number"]}
    assert lc["3"] == 0 and lc["1"] == 127_000
