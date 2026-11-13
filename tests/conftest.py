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


# ---------------------------------------------------------------------------
# Optional external resources.  A test module that needs KiCad's Python (pcbnew),
# wxPython or the SWIG-only tooling imports them at module level; where they are
# absent (CI runners, plain pip venvs) the whole module is reported as SKIPPED with
# the reason instead of a collection ERROR.  With the resources present nothing
# changes: the import succeeds and the report is untouched.
# ---------------------------------------------------------------------------
_OPTIONAL_MODULES = {
    "pcbnew": "KiCad's Python bindings (pcbnew) are not installed",
    "wx": "wxPython is not installed",
}


@pytest.hookimpl(hookwrapper=True)
def pytest_make_collect_report(collector):
    outcome = yield
    report = outcome.get_result()
    if not report.failed:
        return
    text = str(report.longrepr)
    for mod, why in _OPTIONAL_MODULES.items():
        if f"No module named '{mod}'" in text:
            report.outcome = "skipped"
            report.longrepr = (str(collector.path), 0, f"Skipped: {why}")
            return


# Resources that are only discovered inside a test (function-level `import pcbnew`, board
# snapshots resolved in a fixture or test body).  A failure that is exactly "the resource is
# absent" is reported as SKIPPED with that reason; every other failure, including a snapshot
# whose sha does not match its pin, stays a failure.
_SNAPSHOT_ABSENT = "(snapshot directory missing)"


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    if not report.failed or call.excinfo is None:
        return
    exc = call.excinfo.value
    why = None
    if isinstance(exc, ModuleNotFoundError) and exc.name in _OPTIONAL_MODULES:
        why = _OPTIONAL_MODULES[exc.name]
    elif (isinstance(exc, RuntimeError) and str(exc).startswith("snapshot of board ")
          and _SNAPSHOT_ABSENT in str(exc)):
        why = ("needs a pinned private board snapshot (scratch/boards, created by "
               "`python -m harness.snapshot` from ~/work/kicad/projects); not available here")
    if why:
        path, lineno, _ = item.location
        report.outcome = "skipped"
        report.longrepr = (str(item.path), lineno, f"Skipped: {why}")
