"""M4-R1: nothing under plugin/ or toporlib/ imports pcbnew, directly or transitively. ``harness`` (dev/test only,
imports pcbnew) is forbidden there too (F MINOR 2)."""
import ast
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCAN_DIRS = ("plugin", "toporlib")
FORBIDDEN = ("pcbnew", "harness")  # top-level module names the plugin runtime must never import


def _sources():
    for d in SCAN_DIRS:
        yield from sorted((ROOT / d).rglob("*.py"))


def _imports_pcbnew(tree: ast.AST) -> list[int]:
    """Line numbers of imports of a FORBIDDEN module (import, from-import, import_module / __import__)."""
    hits = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            if any(a.name.split(".")[0] in FORBIDDEN for a in node.names):
                hits.append(node.lineno)
        elif isinstance(node, ast.ImportFrom):
            if node.level == 0 and (node.module or "").split(".")[0] in FORBIDDEN:
                hits.append(node.lineno)
        elif isinstance(node, ast.Call):  # importlib.import_module("pcbnew") / __import__("pcbnew")
            f = node.func
            name = f.attr if isinstance(f, ast.Attribute) else getattr(f, "id", "")
            if name in ("import_module", "__import__") and node.args:
                a0 = node.args[0]
                if isinstance(a0, ast.Constant) and str(a0.value).split(".")[0] in FORBIDDEN:
                    hits.append(node.lineno)
    return hits


def test_scan_sees_the_sources():
    names = {p.name for p in _sources()}
    assert "topor_plugin.py" in names and "drcloop.py" in names


def test_no_source_imports_pcbnew():
    bad = []
    for p in _sources():
        for line in _imports_pcbnew(ast.parse(p.read_text(), filename=str(p))):
            bad.append(f"{p.relative_to(ROOT)}:{line}")
    assert not bad, f"pcbnew / harness imported (M4-R1): {bad}"


def test_scanner_detects_an_import(tmp_path):
    assert _imports_pcbnew(ast.parse("import pcbnew"))
    assert _imports_pcbnew(ast.parse("from pcbnew import BOARD"))
    assert _imports_pcbnew(ast.parse("import importlib\nimportlib.import_module('pcbnew')"))
    assert _imports_pcbnew(ast.parse("from harness.model import export_model"))
    assert _imports_pcbnew(ast.parse("import harness.rules"))
    assert _imports_pcbnew(ast.parse("import importlib\nimportlib.import_module('harness.drc')"))
    assert not _imports_pcbnew(ast.parse("import json"))
    assert not _imports_pcbnew(ast.parse("from . import harnessy"))


_PROBE = r"""
import importlib, importlib.util, pkgutil, sys
sys.modules['pcbnew'] = None            # any `import pcbnew` now raises ImportError
sys.modules['harness'] = None           # the dev/test harness imports pcbnew: never in the plugin runtime
import toporlib, topor
mods = ['toporlib'] + [m.name for m in pkgutil.walk_packages(toporlib.__path__, 'toporlib.')]
mods += ['topor'] + [m.name for m in pkgutil.walk_packages(topor.__path__, 'topor.')]
if importlib.util.find_spec('wx') is None:  # the wx dialogs need wxPython (KiCad's interpreter has it)
    mods = [m for m in mods if m != 'topor.ui_wx']
for m in mods:  # every submodule, also the lazily imported ones (model_ipc, docs_gen)
    importlib.import_module(m)
print('imported', ' '.join(mods))
"""


def test_modules_import_with_pcbnew_blocked():
    env = {**os.environ, "PYTHONPATH": os.pathsep.join([str(ROOT), str(ROOT / "plugin")])}
    r = subprocess.run([sys.executable, "-c", _PROBE], cwd=ROOT, env=env,
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    assert "toporlib.drcloop" in r.stdout and "toporlib.drc " in r.stdout + " "
    assert "topor.ports" in r.stdout and "topor.fake" in r.stdout and "topor.ipc" in r.stdout
    assert "toporlib.pcbfile" in r.stdout
    assert "topor.app" in r.stdout  # B10: the entry-point wiring imports with pcbnew blocked
    for m in ("topor.model_ipc", "topor.docs_gen", "topor.run", "topor.preflight", "topor.report"):
        assert f"{m} " in r.stdout + " ", m  # walked, not a fixed list (F MINOR 2)
