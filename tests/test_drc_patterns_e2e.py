"""DRC-cluster patterns (harness/drc_patterns.py) routed end to end, with the expected answer the user approved.

Marker `drcpat` (deselected by default, about 30 s per pattern): run with
    TOPOR_ENGINE_SITE=<wheel site> [TOPOR_VENV=<venv>] .venv/bin/python -m pytest tests/test_drc_patterns_e2e.py -m drcpat
Only approved patterns are here (round 3: via_in_fanin, K7); the others are PROPOSED (scratch gallery).
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pcbnew
import pytest

from harness import drc_patterns as dp
from harness.drc_patterns_png import _drc_new, judge, read_copper

ROOT = Path(__file__).resolve().parents[1]

pytestmark = pytest.mark.drcpat


def _route(name: str, tmp_path: Path, sets: tuple[str, ...] = ()) -> tuple[dict, Path]:
    # a fresh process: the canonical board (KiCad's KIIDs, so the saved footprint order, depend on the process history)
    subprocess.run([sys.executable, "-m", "harness.drc_patterns", str(tmp_path / "boards"), name], check=True, cwd=ROOT,
                   env={**os.environ, "PYTHONPATH": str(ROOT)}, capture_output=True, timeout=120)
    paths = {"problem": tmp_path / "boards" / name / "problem" / f"{name}.kicad_pcb"}
    out = tmp_path / "run"
    env = {**os.environ, "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "2"), "TOPOR_HAND_LABEL": "problem"}
    r = subprocess.run([str(ROOT / "tools" / "route_board.sh"), str(paths["problem"]), str(out), "--keep-copper", "--no-png", *[a for x in sets for a in ("--set", x)]],
                       env=env, capture_output=True, text=True, timeout=600)
    assert r.returncode == 0, r.stdout[-2000:] + r.stderr[-3000:]
    return json.loads((out / f"{name}--metrics.json").read_text()), out


@pytest.mark.xfail(strict=True, reason="main 06549dff: in the canonical footprint order the route is clean but off the "
                   "expected answer (detour x1.10); it matched in another file order (round 2): order-sensitive")
def test_via_in_fanin_matches_expected(tmp_path):
    """K7 (approved round 3): VX's via stays out of the SDA / SCL fan-in corridor (at or below y via_y_min, between its
    pin and the bundle), SDA / SCL run clean 45 deg, 1 via, no DRC, no detour beyond 5 %."""
    name = "via_in_fanin"
    pat = dp.via_in_fanin()
    m, out = _route(name, tmp_path)
    pcb = out / f"{name}-topor" / f"{name}.kicad_pcb"
    segs, vias = read_copper(pcb)
    r = judge(pat, segs, vias, m, _drc_new(out, name))
    assert r["verdict"] == "matches expected", json.dumps(r, default=str)
    assert r["free_angle_segments"] == 0, r
    (x, y, _d, net), = vias
    assert net == "VX" and y >= pat.expect["via_y_min"] - 1e-3, (x, y, net)
    assert any(t.GetClass() == "PCB_VIA" for t in pcbnew.LoadBoard(str(pcb)).GetTracks())


def _bundle_octilinear(name: str, tmp_path: Path, sets: tuple[str, ...] = ()) -> dict:
    pat = getattr(dp, name)()
    m, out = _route(name, tmp_path, sets)
    segs, vias = read_copper(out / f"{name}-topor" / f"{name}.kicad_pcb")
    r = judge(pat, segs, vias, m, _drc_new(out, name))
    assert r["free_angle_segments"] == 0, json.dumps(r, default=str)
    assert r["verdict"] == "matches expected", json.dumps(r, default=str)
    return r


def test_jst_fanout_45_close_octilinear_bundle(tmp_path):
    """K5 (accepted round 3, user: "triggers the free angle bug"): the six wires follow the kept 3V3 as one parallel
    45 deg / horizontal / 45 deg bundle on B.Cu: 0 free-angle segments, 0 vias, no DRC, within 5 % of the reference.
    Base 5571d1bd kept 2 free-angle legs (D1, D2): the draw order started in the middle of the bundle and each 45 deg
    form crossed the reserved bands of the undrawn neighbours; fixed by the sweep order (first-draw-45 P3,
    [drawing] octi_order = "sweep"; not the default, so set here)."""
    _bundle_octilinear("jst_fanout_45_close", tmp_path, ("drawing.octi_order=sweep",))
