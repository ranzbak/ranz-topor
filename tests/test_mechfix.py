"""Mechanism fixtures (harness/mechfix.py) routed end to end: strip, topo core, KiCad DRC with refill.

Marker `mechfix` (deselected by default, seconds per case): run with
    TOPOR_ENGINE_SITE=<own wheel site> .venv/bin/python -m pytest tests/test_mechfix.py -m mechfix
`TOPOR_FX_OUT=<dir>` keeps each case's run (and its compare PNG) under <dir>/<case>/.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from harness import mechfix

ROOT = Path(__file__).resolve().parents[1]

pytestmark = pytest.mark.mechfix


def _route(case: str, tmp_path: Path) -> dict:
    src = getattr(mechfix, case)(tmp_path / "src")
    out = Path(os.environ["TOPOR_FX_OUT"]) / case if os.environ.get("TOPOR_FX_OUT") else tmp_path / "out"
    env = {**os.environ, "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "2"), "TOPOR_HAND_LABEL": "unrouted input"}
    r = subprocess.run([str(ROOT / "tools" / "route_board.sh"), str(src), str(out), "--timeout", "60", "--name", case],
                       env=env, capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout[-2000:] + r.stderr[-3000:]
    return json.loads((out / f"{case}--metrics.json").read_text())


def _clean(m: dict) -> None:
    assert m["routed"] == m["connections"], m
    assert m["unconnected"] == 0, m
    assert m["routing_new"] == 0, m


def _result(m: dict) -> dict:
    return json.loads((Path(m["report_dir"]) / "result.json").read_text())["route"]


def _first_round(m: dict) -> None:
    """Round 1 drew every connection: no rescue by the topology feedback (a later round's
    lowered rooms reroute around what the planner did not see, usually as a detour)."""
    picked = [v for v in _result(m)["variants"] if v.get("picked")]
    r1 = [v for v in _result(m)["variants"] if v["label"].startswith("round 1")]
    assert r1 and min(v["errors"] for v in r1) == 0, (picked, r1)


def test_fx_plane_stub_squeeze(tmp_path):
    """The GND stub via keeps out of SIG's corridor and the planner sees it: SIG runs straight,
    drawn in round 1 (HEAD 2bfab0b: via in the corridor, round 1 fails, round 2 detours 1.066)."""
    m = _route("fx_plane_stub_squeeze", tmp_path)
    _clean(m)
    _first_round(m)
    assert m["detour"]["max"] <= 1.02, m["detour"]



def _conns(m: dict, net: str) -> list:
    return [c for c in _result(m)["connections"] if c["name"] == net]


def test_fx_two_sided_overlap(tmp_path):
    """SIG goes on to U from the near pad R (under the front pad M of net K), not from the far
    pad L, and the whole net is about the chain L -> R -> U long (HEAD 2bfab0b: from L, 40.3 mm)."""
    m = _route("fx_two_sided_overlap", tmp_path)
    _clean(m)
    sig = _conns(m, "SIG")
    to_u = [c for c in sig if "U1.1" in (c["from"]["label"], c["to"]["label"])]
    assert len(to_u) == 1 and "R1.1" in (to_u[0]["from"]["label"], to_u[0]["to"]["label"]), to_u
    chain = 6.84 + ((35.25 - 15.37) ** 2 + (17.14 - 3.55) ** 2) ** 0.5
    # 1.15, not 1.0: the drawing's taut path still bends at Steiner points of the sleeve P2 chose
    # (D6a: 34.4 mm, 1.11 x the chain; a separate mechanism).
    assert sum(c["length_nm"] for c in sig) / 1e6 <= 1.15 * chain, [(c["from"]["label"], c["to"]["label"], c["length_nm"]) for c in sig]

def test_fx_copper_graphic_corridor(tmp_path):
    """The planner sees the copper graphic: SIG is drawn in round 1 (HEAD 2bfab0b: planned
    through it, the drawing rejects the run)."""
    m = _route("fx_copper_graphic_corridor", tmp_path)
    _clean(m)
    _first_round(m)

def test_fx_keepout_no_via_site(tmp_path):
    """The crossing's vias keep out of the no-via keepout; everything drawn in round 1."""
    m = _route("fx_keepout_no_via_site", tmp_path)
    _clean(m)
    _first_round(m)
def test_fx_keepout_no_via_inner(tmp_path):
    """Through vias keep out of a no-via keepout on In1.Cu alone (4 layers)."""
    _clean(_route("fx_keepout_no_via_inner", tmp_path))


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-m", "mechfix", "-q"]))
