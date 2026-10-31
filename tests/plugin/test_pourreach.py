"""MINOR 3 of the M4b gate review: the pure pour-reach / dangling-end functions live in ``topor.pourreach`` (no
kipy), ``topor.model_ipc`` re-exports the same objects, and the run flow imports without kipy."""
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]

_BLOCK = """
import sys, importlib.abc
class Block(importlib.abc.MetaPathFinder):
    def find_spec(self, name, path=None, target=None):
        if name == "kipy" or name.startswith("kipy."):
            raise ImportError("kipy blocked by the test")
        return None
sys.meta_path.insert(0, Block())
import topor.pourreach, topor.run
from topor.pourreach import pour_reach, dangling_ends, pad_contains, pad_sample_points
assert not [m for m in sys.modules if m == "kipy" or m.startswith("kipy.")]
print("ok")
"""


def test_pourreach_and_run_import_without_kipy():
    env = {"PYTHONPATH": f"{ROOT / 'plugin'}:{ROOT}", "PATH": "/usr/bin:/bin"}
    r = subprocess.run([sys.executable, "-c", _BLOCK], cwd=ROOT, env=env, capture_output=True, text=True)
    assert r.returncode == 0 and "ok" in r.stdout, r.stderr[-2000:]


def test_model_ipc_reexports_the_pure_functions():
    pytest.importorskip("kipy")
    from topor import model_ipc, pourreach

    for name in ("pour_reach", "dangling_ends", "pad_contains", "pad_sample_points"):
        assert getattr(model_ipc, name) is getattr(pourreach, name), name


def test_pour_reach_probe_inside_fill():
    from topor.pourreach import pour_reach

    pad = {"ref": "R1", "number": "1", "net": 3, "layers": [0], "center": [0, 0], "rotation_deg": 0.0,
           "shapes": {"0": {"kind": "rect", "w": 1_000_000, "h": 1_000_000}}}
    other = {**pad, "ref": "R2", "center": [50_000_000, 0]}
    via = {"uuid": "v1", "net": 3, "at": [0, 0], "d": 600_000, "layers": [0, 2]}
    fill = [{"layer": 0, "outer": [[-2_000_000, -2_000_000], [2_000_000, -2_000_000], [2_000_000, 2_000_000],
                                   [-2_000_000, 2_000_000]], "holes": []}]
    assert pour_reach(3, [0], fill, [pad, other], [via]) == (["R1.1"], ["v1"])
    assert pour_reach(4, [0], fill, [pad], [via]) == ([], [])  # other net: nothing
