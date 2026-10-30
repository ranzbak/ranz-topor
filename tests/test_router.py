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
