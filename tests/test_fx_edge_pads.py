"""fx pattern edge_pads (harness/fx_survivors.py, approved by the user 2026-11-17) routed end to end in both drawing modes:
KiCad DRC 0 and the wire at the optimal x = 0.625 mm (copper-to-edge 0.5 + half the 0.25 mm track, measured from the
Edge.Cuts centre line as KiCad 10 does; the drawing's tolerance on top). main 0a53849: x = 0.600 (track ends at the pad
centres), 1 copper_edge_clearance each.

Marker `mechfix` (deselected by default, seconds per case):
    TOPOR_ENGINE_SITE=<own wheel site> .venv/bin/python -m pytest tests/test_fx_edge_pads.py -m mechfix
"""
import json
import os
import re
import subprocess
from pathlib import Path

import pytest

from harness import fx_survivors

ROOT = Path(__file__).resolve().parents[1]

pytestmark = pytest.mark.mechfix


@pytest.mark.parametrize("mode", ["seq", "erbs"])
def test_edge_pads_drc_clean_at_the_optimal_x(tmp_path, mode):
    src = fx_survivors.generate("edge_pads", tmp_path / "boards")["problem"]
    out = tmp_path / f"run-{mode}"
    extra = ["--set", "optimize.rounds=1"] + (["--set", "drawing.realise=erbs"] if mode == "erbs" else [])
    env = {**os.environ, "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "2"), "TOPOR_HAND_LABEL": "unrouted input"}
    r = subprocess.run([str(ROOT / "tools" / "route_board.sh"), str(src), str(out), "--no-png", "--timeout", "60", *extra],
                       env=env, capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout[-2000:] + r.stderr[-3000:]
    m = json.loads((out / "edge_pads--metrics.json").read_text())
    assert m["routed"] == m["connections"] == 1, m
    assert m["unconnected"] == 0 and m["routing_new"] == 0, m["drc_by_type_new"]
    pcb = (out / "edge_pads-topor" / "edge_pads.kicad_pcb").read_text()
    segs = re.findall(r"\(segment\s+\(start ([-\d.]+) ([-\d.]+)\)\s+\(end ([-\d.]+) ([-\d.]+)\)", pcb)
    assert len(segs) == 1, segs
    x0, y0, x1, y1 = map(float, segs[0])
    for x in (x0, x1):
        assert 0.625 <= x <= 0.6252, segs  # inside both pads (0.5 .. 0.7), never 0.600 (centre) or 0.650 (half stroke added)
    assert sorted((y0, y1)) == [2.0, 9.0], segs
