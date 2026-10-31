"""B10: app wiring, headless mode, handshake, entry point, build layout.

No wx window is ever shown (Ruling C18): headless uses ScriptedUi. Report folders go to a tmp
TOPOR_REPORT_ROOT (Ruling C1). The real plugin dir is never written (Ruling C3).
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

topor_engine = pytest.importorskip("topor_engine")

from topor import app  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
REAL_PLUGIN_DIR = Path.home() / ".local/share/kicad/10.0/plugins/org.topor.router"
BOARD = "eprom-to-kernal"


@pytest.fixture(autouse=True)
def _report_root(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(tmp_path / "reports"))


def _snapshot(plugin_dir: Path) -> dict:
    if not plugin_dir.exists():
        return {}
    return {str(p.relative_to(plugin_dir)): (p.stat().st_mtime_ns, p.stat().st_size)
            for p in sorted(plugin_dir.rglob("*")) if p.is_file()}


class _Eng:
    def __init__(self, v):
        self._v = v

    def version(self):
        return self._v


def test_handshake_ok_and_mismatch():
    assert app.handshake("0.4.0", _Eng("0.4.0")) is None
    assert app.handshake(None, _Eng("9.9.9")) is None
    f = app.handshake("0.4.0", _Eng("0.3.0"))
    assert f.code == "ENGINE_VERSION" and f.severity == "error"
    assert "Restart the PCB Editor or use Recreate Plugin Environment" in f.fix


def test_main_engine_mismatch_exits_1(tmp_path, capsys):
    rc = app.main(["--headless", str(tmp_path / "x.kicad_pcb"), "--model", str(tmp_path / "m.json")],
                  shipped_version="0.0.1")
    assert rc == 1
    assert "Recreate Plugin Environment" in capsys.readouterr().err


def test_main_baseexception_exits_1_and_logs(tmp_path, capsys):
    class Boom(BaseException):
        pass

    class Eng:
        def version(self):
            raise Boom("pyo3 panic")

    rc = app.main(["--headless", "a.kicad_pcb", "--model", "m.json"], shipped_version="0.4.0", engine=Eng())
    assert rc == 1
    assert "Boom" in capsys.readouterr().err
    assert "pyo3 panic" in (tmp_path / "reports" / "topor-crash.log").read_text()


class RecordingUi:
    """Stands in for WxUi in run_gui (no window is ever shown): records every call."""

    def __init__(self):
        self.calls = []

    def __getattr__(self, name):
        def rec(*a, **kw):
            self.calls.append((name, a))
        return rec


def test_run_gui_kicad_api_connect_failure_shows_error_and_exits_1(tmp_path, monkeypatch, capsys):
    """Ruling C32 (gate review I-5): the KiCad API connection fails (API server off) -> an error dialog with
    KICAD_API_CONNECT, crash log entry, exit 1. The REAL kipy connect is used, on a socket that does not exist."""
    monkeypatch.setenv("KICAD_API_SOCKET", f"ipc://{tmp_path / 'no-kicad-here.sock'}")
    monkeypatch.setenv("KICAD_API_TOKEN", "test-token")
    ui = RecordingUi()
    rc = app.run_gui(None, _Eng("0.4.0"), ui=ui)
    assert rc == 1
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert len(errs) == 1 and errs[0].code == "KICAD_API_CONNECT" and errs[0].severity == "error"
    assert "Enable KiCad API" in errs[0].fix and "→" in errs[0].fix
    assert "KICAD_API_CONNECT" in (tmp_path / "reports" / "topor-crash.log").read_text()
    assert "KICAD_API_CONNECT" in capsys.readouterr().err


def test_run_gui_connect_seam_failure(tmp_path):
    def boom():
        raise ConnectionError("Failed to connect to KiCad: Connection refused")

    ui = RecordingUi()
    assert app.run_gui(None, _Eng("0.4.0"), ui=ui, connect=boom) == 1
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert [e.code for e in errs] == ["KICAD_API_CONNECT"] and "Connection refused" in errs[0].message


def test_main_headless_report_dir_unwritable_exits_1(tmp_path, monkeypatch, capsys):
    """Ruling C32: a report root that cannot be created -> REPORT_DIR_UNWRITABLE (ui.error), exit 1."""
    blocker = tmp_path / "file-not-dir"
    blocker.write_text("x")
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(blocker / "sub"))
    pcb = tmp_path / "b.kicad_pcb"
    pcb.write_text("(kicad_pcb (version 20260206) (generator pcbnew))\n")
    mp = tmp_path / "m.json"
    mp.write_text("{}")
    rc = app.main(["--headless", str(pcb), "--model", str(mp)], shipped_version=None, engine=_Eng("0.4.0"))
    assert rc == 1
    assert "REPORT_DIR_UNWRITABLE" in capsys.readouterr().err


def test_app_and_run_do_not_import_kipy():
    """MINOR 3: the KiCad connection lives behind topor.ipc (IpcBoard.connect); the run flow needs no kipy (the
    pure pour reach / dangling ends live in topor.pourreach, not in model_ipc)."""
    import ast

    for name in ("app.py", "run.py"):
        tree = ast.parse((ROOT / "plugin" / "topor" / name).read_text())
        mods = set()
        for n in ast.walk(tree):
            if isinstance(n, ast.Import):
                mods |= {a.name for a in n.names}
            elif isinstance(n, ast.ImportFrom):
                mods.add(("." * n.level) + (n.module or ""))
        assert not {m for m in mods if m.split(".")[0] == "kipy"}, (name, mods)
        if name == "run.py":
            assert ".model_ipc" not in mods and ".ipc" not in mods, mods


def test_headless_needs_model(capsys):
    assert app.main(["--headless", "a.kicad_pcb"]) == 2


def test_selection_from_json():
    s = app.selection_from_json('{"pads": [["U1", "1"]], "footprints": ["U2"], "area": [[0, 0], [5, 5]]}')
    assert s.pads[0].ref == "U1" and s.footprints == ["U2"] and s.area == [(0, 0), (5, 5)]
    assert app.selection_from_json(None).empty


@pytest.fixture(scope="module")
def eprom_files(tmp_path_factory):
    pytest.importorskip("pcbnew")
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.paths import board_pcb
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("proj")
    cp = copy_project(board_pcb(BOARD), d / BOARD)
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    model = export_model(b, rules, name=BOARD, dru_text=dru)
    # the shipped eprom is hand-routed: rip it so there is something to route (topor.toml next to the board)
    (cp.pcb.parent / "topor.toml").write_text('version = 1\n\n[run]\nexisting_copper = "rip"\n')
    mp = d / "model.json"
    mp.write_text(json.dumps(model))
    return cp.pcb, mp


def test_headless_run_on_fixture_accepts(eprom_files, capsys):
    pcb, model = eprom_files
    rc = app.main(["--headless", str(pcb), "--model", str(model), "--accept"], shipped_version=None)
    out = capsys.readouterr()
    assert rc == 0, out.err
    assert "topor: accepted routed 27/27" in out.out


def test_headless_reject_exits_0(eprom_files, capsys):
    pcb, model = eprom_files
    rc = app.main(["--headless", str(pcb), "--model", str(model)], shipped_version=None)
    out = capsys.readouterr()
    assert rc == 0, out.err
    assert "topor: rejected" in out.out


def test_python_dash_m_headless_usage_documented():
    assert "python -m topor.app --headless" in app.__doc__


def test_plugin_json_fields():
    pj = json.loads((ROOT / "plugin" / "plugin.json").read_text())
    assert pj["runtime"]["min_version"] == "3.11"
    a = pj["actions"][0]
    assert a["name"] == "Topor: route" and a["description"] == "Route the board or the selection with Topor"


def test_build_layout_in_scratch_kicad_home(tmp_path):
    real_before = _snapshot(REAL_PLUGIN_DIR)
    env = {**os.environ, "KICAD_DOCUMENTS_HOME": str(tmp_path / "kicad10"), "TOPOR_VENV": sys.prefix}
    r = subprocess.run(["bash", str(ROOT / "tools" / "build.sh")], cwd=ROOT, env=env, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr[-2000:]
    d = tmp_path / "kicad10" / "plugins" / "org.topor.router"
    for name in ("plugin.json", "topor_plugin.py", "requirements.txt"):
        assert (d / name).is_file(), name
    for name in ("topor", "toporlib", "docs/user", "wheels"):
        assert (d / name).is_dir(), name
    assert (d / "topor" / "app.py").is_file() and (d / "toporlib" / "drc.py").is_file()
    assert not (d / "harness").exists()
    assert not list(d.rglob("__pycache__"))
    assert list((d / "wheels").glob("topor_engine-0.4.0-*.whl"))
    assert "topor-engine==0.4.0" in (d / "requirements.txt").read_text()
    assert 'SHIPPED_VERSION = "0.4.0"' in (d / "topor_plugin.py").read_text()
    # the shipped entry point puts its own dir on sys.path: `import topor` / `import toporlib` work from anywhere
    probe = ("import sys, runpy; sys.path = [p for p in sys.path if p]; "
             "import topor_plugin, topor.app, toporlib.drc; print('ok', topor_plugin.SHIPPED_VERSION)")
    pr = subprocess.run([sys.executable, "-c", probe], cwd=tmp_path, capture_output=True, text=True,
                        env={**os.environ, "PYTHONPATH": str(d)})
    assert pr.returncode == 0 and "ok 0.4.0" in pr.stdout, pr.stderr
    # entry point alone (no PYTHONPATH) finds the packages
    env2 = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"}
    pr2 = subprocess.run([sys.executable, "-c",
                          f"import importlib.util as u; s=u.spec_from_file_location('topor_plugin', r'{d}/topor_plugin.py');"
                          "m=u.module_from_spec(s); s.loader.exec_module(m); import topor.app, toporlib.drc; print('ok')"],
                         cwd=tmp_path, capture_output=True, text=True, env=env2)
    assert pr2.returncode == 0 and "ok" in pr2.stdout, pr2.stderr
    # the user's real KiCad plugin dir is untouched (Ruling C3)
    assert _snapshot(REAL_PLUGIN_DIR) == real_before
