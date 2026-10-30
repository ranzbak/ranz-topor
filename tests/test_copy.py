import pcbnew
import pytest

from harness.copy import assert_unchanged, copy_project, sha256_tree
from harness.fixtures import make_fixture_board, write_project_file


def test_copy_project_leaves_source_untouched(tmp_path):
    src = tmp_path / "src"
    pcb = make_fixture_board(src / "fx.kicad_pcb")
    write_project_file(pcb, rules={}, classes=[])
    h = sha256_tree(src)
    pc = copy_project(pcb, tmp_path / "work")
    assert pc.pcb.exists() and pc.pro and pc.pro.exists()
    pcbnew.SaveBoard(str(pc.pcb), pcbnew.LoadBoard(str(pc.pcb)))
    assert_unchanged(src, h)


def test_copy_project_missing_pro_is_none(tmp_path):
    src = tmp_path / "src"
    pcb = make_fixture_board(src / "fx.kicad_pcb")
    pc = copy_project(pcb, tmp_path / "work")
    assert pc.pro is None and pc.dru is None and pc.pcb.exists()


def test_assert_unchanged_raises(tmp_path):
    (tmp_path / "a").write_text("x")
    h = sha256_tree(tmp_path)
    (tmp_path / "a").write_text("y")
    with pytest.raises(RuntimeError):
        assert_unchanged(tmp_path, h)
