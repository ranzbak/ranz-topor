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


_MMN = 1_000_000
_HULL = [[-1_905_000, -635_000], [635_000, -635_000], [635_000, 635_000], [-1_905_000, 635_000]]
_NOTCH_FILL = [[-2_500_000, -150_000], [-1_700_000, -150_000], [-1_700_000, 150_000], [-2_500_000, 150_000]]
_BIG_FILL = [[-_MMN, -_MMN], [_MMN, -_MMN], [_MMN, _MMN], [-_MMN, _MMN]]


def _fill(ring):
    return [{"layer": 0, "outer": ring, "holes": []}]


def _pad(shape, **kw):
    return {"ref": "U4", "number": "29", "net": 1, "center": [0, 0], "rotation_deg": 0.0, "layers": [0],
            "shapes": {"0": shape}, **kw}


def test_pour_reach_zone_connection_none_hull_notch_not_reached():
    from topor.pourreach import pour_reach
    # U4.29 (esp-rust-board): custom pad, hull bridges a V-notch that the fill enters; zone_connection none
    pad = _pad({"kind": "poly", "pts": _HULL}, zone_connection=0)
    assert pour_reach(1, [0], _fill(_NOTCH_FILL), [pad], []) == ([], [])


def test_pour_reach_zone_connection_none_rect_over_centre_not_reached():
    from topor.pourreach import pour_reach
    pad = _pad({"kind": "rect", "w": 1_270_000, "h": 1_270_000}, zone_connection=0)
    assert pour_reach(1, [0], _fill(_BIG_FILL), [pad], []) == ([], [])


def test_pour_reach_pour_pad_connection_none_applies_unless_pad_overrides():
    from topor.pourreach import pour_reach
    pad = _pad({"kind": "rect", "w": 1_270_000, "h": 1_270_000})
    assert pour_reach(1, [0], _fill(_BIG_FILL), [pad], [], 0) == ([], [])
    assert pour_reach(1, [0], _fill(_BIG_FILL), [{**pad, "zone_connection": 2}], [], 0) == (["U4.29"], [])


def test_pour_reach_connected_pads_still_reached():
    from topor.pourreach import pour_reach
    pad = _pad({"kind": "rect", "w": 1_270_000, "h": 1_270_000})
    for conn in (1, 2, 3):
        assert pour_reach(1, [0], _fill(_BIG_FILL), [pad], [], conn) == (["U4.29"], [])
        assert pour_reach(1, [0], _fill(_BIG_FILL), [{**pad, "zone_connection": conn}], [], 0) == (["U4.29"], [])
    assert pour_reach(1, [0], _fill(_BIG_FILL), [pad], []) == (["U4.29"], [])  # inherited, default thermal


def test_pad_sample_points_poly_skips_hull_edge_midpoints():
    from topor.pourreach import pad_sample_points
    pts = pad_sample_points(_pad({"kind": "poly", "pts": _HULL}), 0)
    assert len(pts) == 1 + len(_HULL)
