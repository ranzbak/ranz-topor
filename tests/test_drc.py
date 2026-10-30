import pcbnew
import pytest

from harness.drc import classify, is_clean, run_drc, summarise

MM = 1_000_000


def _add_clearance_track(pcb):
    """Add a SIG F.Cu track passing 50 um from the edge of VCC pad 1 at (20, 20)."""
    b = pcbnew.LoadBoard(str(pcb))
    t = pcbnew.PCB_TRACK(b)
    t.SetStart(pcbnew.VECTOR2I(17 * MM, int(20.975 * MM)))
    t.SetEnd(pcbnew.VECTOR2I(23 * MM, int(20.975 * MM)))
    t.SetWidth(int(0.25 * MM))
    t.SetLayer(pcbnew.F_Cu)
    t.SetNet(b.FindNet("SIG"))
    b.Add(t)
    pcbnew.SaveBoard(str(pcb), b)


def test_run_drc_fixture_exit_5_is_ok(fixture_board, tmp_path):
    r = run_drc(fixture_board.pcb, tmp_path / "d.json")
    assert r.exit_code in (0, 5)
    assert r.by_type.get("track_dangling", 0) >= 1
    assert (tmp_path / "d.json").is_file()


def test_summary_only_incomplete_is_clean(fixture_board, tmp_path):
    r = run_drc(fixture_board.pcb, tmp_path / "d.json")
    c = classify(r, None)
    assert c["routing_new"] == 0
    assert c["incomplete"]["track_dangling"] >= 1
    assert is_clean(c)
    s = summarise("fx", r)
    assert len(s.splitlines()) <= 20
    assert s.startswith("DRC fx: routing_new=0")


def test_classify_preexisting(fixture_board, tmp_path):
    _add_clearance_track(fixture_board.pcb)
    base = run_drc(fixture_board.pcb, tmp_path / "base.json")
    assert classify(base, None)["routing_new"] == 1
    c = classify(base, base)
    assert c["routing_preexisting"] == 1
    assert c["routing_new"] == 0
    assert c["by_type_new"].get("clearance", 0) == 0
    assert is_clean(c)


def test_classify_new_clearance(fixture_board, tmp_path):
    _add_clearance_track(fixture_board.pcb)
    r = run_drc(fixture_board.pcb, tmp_path / "d.json")
    c = classify(r, None)
    assert c["routing_new"] == 1
    assert c["by_type_new"]["clearance"] == 1
    assert not is_clean(c)
    s = summarise("fx", r)
    assert "clearance: 1 (+0)" in s
    assert len(s.splitlines()) <= 20


def test_run_drc_bad_path_raises(tmp_path):
    with pytest.raises(RuntimeError):
        run_drc(tmp_path / "nope.kicad_pcb", tmp_path / "d.json")
