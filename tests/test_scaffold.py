# tests/test_scaffold.py
import subprocess, sys, pathlib
ROOT = pathlib.Path(__file__).resolve().parents[1]

def test_python_is_venv_with_pcbnew():
    import pcbnew
    assert sys.prefix != sys.base_prefix
    assert pcbnew.GetBuildVersion().startswith("10.0")

def test_cargo_workspace_builds():
    r = subprocess.run(["cargo", "test", "-p", "topor-engine", "--quiet"], cwd=ROOT, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
