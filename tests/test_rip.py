import pcbnew

from harness.fixtures import make_fixture_board
from harness.rip import record_widths, rip_tracks


def test_record_widths_fixture(fixture_board):
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    w = record_widths(b)
    assert w["SIG"].median_nm == 250_000 and w["SIG"].count == 2 and "VCC" not in w
    assert w["SIG"].min_nm == w["SIG"].max_nm == 250_000


def test_rip_keeps_locked_free_and_rf(fixture_board):
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    r = rip_tracks(b, rf_nets={"GND"})
    assert r.removed_arcs == 1 and r.removed_tracks == 0
    assert r.kept_locked_tracks == 1 and r.kept_free_vias == 1 and r.removed_vias == 0
    assert "SIG" in r.nets_to_route and "GND" in r.kept_nets
    assert len(list(b.GetTracks())) == 2  # locked track + free via


def test_rip_removes_transition_via(tmp_path):
    pcb = make_fixture_board(tmp_path / "t.kicad_pcb", locked_track=False, arc=False,
                             transition_via=True)
    b = pcbnew.LoadBoard(str(pcb))
    r = rip_tracks(b)
    assert r.removed_vias == 1 and r.removed_tracks == 2 and r.kept_free_vias == 1
    assert [t.GetClass() for t in b.GetTracks()] == ["PCB_VIA"]


def test_rip_keeps_via_touched_by_locked_track(tmp_path):
    pcb = make_fixture_board(tmp_path / "t.kicad_pcb", locked_track=False, arc=False,
                             transition_via=True)
    b = pcbnew.LoadBoard(str(pcb))
    for t in b.GetTracks():
        if t.GetClass() != "PCB_VIA" and t.GetLayer() == pcbnew.B_Cu:
            t.SetLocked(True)
    r = rip_tracks(b)
    assert r.removed_vias == 0 and r.kept_locked_tracks == 1 and r.removed_tracks == 1


def test_rip_mode_none_keeps_everything(fixture_board):
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    n = len(list(b.GetTracks()))
    r = rip_tracks(b, mode="none")
    assert (r.removed_tracks, r.removed_arcs, r.removed_vias) == (0, 0, 0)
    assert len(list(b.GetTracks())) == n
    assert "SIG" in r.nets_to_route and "GND" not in r.nets_to_route


def test_rip_scoped_by_pad_ref(fixture_board):
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    r = rip_tracks(b, mode="scoped", scope={"pads": ["U1.2"]})
    assert r.removed_arcs == 1 and r.removed_tracks == 0 and r.removed_vias == 0
    assert r.kept_locked_tracks == 1
    assert r.nets_to_route == ["SIG"]
