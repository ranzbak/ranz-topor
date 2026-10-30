import json
import subprocess

import pcbnew
import pytest

from harness.fixtures import make_fixture_board
from harness.paths import kicad_cli


def test_fixture_board_counts(fixture_board):
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    assert len(b.GetFootprints()) == 1 and len(b.GetPads()) == 7 and b.GetNetCount() == 4
    assert sum(1 for t in b.GetTracks() if t.GetClass() == "PCB_ARC") == 1
    assert sum(1 for t in b.GetTracks() if t.GetClass() == "PCB_VIA") == 1
    assert any(d.GetClass() == "PCB_TEXT" and d.GetLayer() == pcbnew.F_Cu for d in b.GetDrawings())
    shapes = {d.GetShape() for d in b.GetDrawings() if d.GetClass() == "PCB_SHAPE"}
    assert pcbnew.SHAPE_T_ARC in shapes and pcbnew.SHAPE_T_BEZIER in shapes


def test_fixture_options(tmp_path):
    p = make_fixture_board(tmp_path / "o.kicad_pcb", net_tie=True, half_track=True)
    b = pcbnew.LoadBoard(str(p))
    assert len(b.GetFootprints()) == 2
    assert any(f.IsNetTie() for f in b.GetFootprints())
    assert "net_tie_pad_groups" in p.read_text()
    assert sum(1 for t in b.GetTracks() if t.GetClass() == "PCB_TRACK") == 2


@pytest.mark.skipif(not kicad_cli().exists(), reason="no kicad-cli")
def test_fixture_drc_has_no_routing_violations(fixture_board, tmp_path):
    out = tmp_path / "drc.json"
    r = subprocess.run(
        [str(kicad_cli()), "pcb", "drc", "--format", "json", "--refill-zones", "--all-track-errors",
         "--exit-code-violations", "-o", str(out), str(fixture_board.pcb)],
        capture_output=True, text=True)
    assert r.returncode in (0, 5), r.stderr
    rep = json.loads(out.read_text())
    bad = {"clearance", "copper_edge_clearance", "hole_clearance", "track_width"}
    found = [v for v in rep.get("violations", []) if v["type"] in bad]
    assert not found, found
