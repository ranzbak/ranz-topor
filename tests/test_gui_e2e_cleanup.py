"""Offline parts of the GUI Cleanup Tracks & Vias step (harness/gui_e2e.py): option parsing and the dialog state
written into the test KiCad home. The live dialog run is tools/gui_e2e.sh --cleanup / --existing."""
import pytest

from harness import gui_e2e as g
from harness.cleanup_check import OPTIONS


def test_box_names_are_the_replay_options_plus_refill():
    assert set(g.CLEANUP_BOXES) == set(OPTIONS) | {"refill"}


def test_cleanup_options_all_and_list():
    assert g.cleanup_options("all") == {k: True for k in g.CLEANUP_BOXES}
    o = g.cleanup_options("refill, short,merge")
    assert [k for k, v in o.items() if v] == ["refill", "short", "merge"]
    with pytest.raises(SystemExit):
        g.cleanup_options("refill,bogus")


def test_control_state_maps_boxes_in_creation_order_and_clears_filters():
    opts = {k: k in ("refill", "dangling_vias", "in_pads") for k in g.CLEANUP_BOXES}
    st = g.cleanup_control_state(opts)
    # dialog_cleanup_tracks_and_vias_base.cpp creation order: refill, short, vias, dangling vias, merge, unconnected,
    # tracks in pads -> wxStaticBox_0wxCheckBox_0..6
    assert [st[f"wxStaticBox_0wxCheckBox_{i}"] for i in range(7)] == [True, False, False, True, False, False, True]
    assert not any(st[f"wxStaticBox_1wxCheckBox_{i}"] for i in range(4))


def test_copy_board_takes_project_files(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    for n in ("b.kicad_pcb", "b.kicad_pro", "b.kicad_dru", "fp-lib-table", "other.txt"):
        (src / n).write_text(n)
    out = g.copy_board(src / "b.kicad_pcb", tmp_path / "dst", stem="c")
    assert out == tmp_path / "dst" / "c.kicad_pcb"
    assert sorted(p.name for p in (tmp_path / "dst").iterdir()) == ["c.kicad_dru", "c.kicad_pcb", "c.kicad_pro",
                                                                    "fp-lib-table"]
