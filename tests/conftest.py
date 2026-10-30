import pathlib

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[1]


@pytest.fixture(scope="session")
def root():
    return ROOT


@pytest.fixture(scope="session")
def engine():
    """Import the topor_engine wheel; skip if it is not built into the venv."""
    try:
        import topor_engine
    except ImportError:
        pytest.skip("topor_engine wheel not built (run maturin develop in engine/)")
    return topor_engine


@pytest.fixture
def fixture_board(tmp_path):
    from harness.copy import ProjectCopy
    from harness.fixtures import make_fixture_board, write_project_file

    pcb = make_fixture_board(tmp_path / "fx" / "fixture.kicad_pcb")
    pro = write_project_file(pcb, rules={}, classes=[])
    return ProjectCopy(pcb=pcb, pro=pro, dru=None, src_hashes={})
