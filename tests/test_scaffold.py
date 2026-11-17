# tests/test_scaffold.py
import os, subprocess, sys, pathlib
import pytest
ROOT = pathlib.Path(__file__).resolve().parents[1]

def test_python_is_venv_with_pcbnew():
    import pcbnew
    assert sys.prefix != sys.base_prefix
    assert pcbnew.GetBuildVersion().startswith("10.0")

@pytest.mark.skipif(bool(os.environ.get("CI")),
                    reason="CI runs `cargo test --release` in its own rust job; here it would rebuild the engine")
def test_cargo_workspace_builds():
    # build-only (PK-DBG, user 2026-11-17 VM load): the engine and all its test binaries compile in release (the profile the
    # wheel and CI use); running them is the rust job's business (`cargo test --release`), not a pytest's. It used to run the
    # whole debug suite here (a second, slow copy).
    r = subprocess.run(["cargo", "test", "-p", "topor-engine", "--release", "--no-run", "--quiet"], cwd=ROOT, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr

def test_plugin_versions_match_engine():
    import json, re, tomllib
    eng = tomllib.loads((ROOT / "engine" / "Cargo.toml").read_text())["package"]["version"]
    plug = json.loads((ROOT / "plugin" / "plugin.json").read_text())["version"]
    shipped = re.search(r'^SHIPPED_VERSION = "([^"]+)"', (ROOT / "plugin" / "topor_plugin.py").read_text(), re.M)
    pin = re.search(r"^topor-engine==(\S+)$", (ROOT / "plugin" / "requirements.txt").read_text(), re.M)
    assert plug == shipped.group(1) == pin.group(1) == eng
