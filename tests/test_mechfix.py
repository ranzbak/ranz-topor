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


def _route(case: str, tmp_path: Path, *extra: str, tag: str = "") -> dict:
    src = getattr(mechfix, case)(tmp_path / "src")
    out = Path(os.environ["TOPOR_FX_OUT"]) / (case + tag) if os.environ.get("TOPOR_FX_OUT") else tmp_path / ("out" + tag)
    env = {**os.environ, "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "2"), "TOPOR_HAND_LABEL": "unrouted input"}
    r = subprocess.run([str(ROOT / "tools" / "route_board.sh"), str(src), str(out), "--timeout", "60", "--name", case, *extra],
                       env=env, capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout[-2000:] + r.stderr[-3000:]
    m = json.loads((out / f"{case}--metrics.json").read_text())
    m["_out"] = str(out)
    return m


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


def test_fx_pinned_crossing(tmp_path):
    """A and B, both held to F.Cu over the foreign B.Cu pad K, do not cross over K (D6a: they
    crossed there, "crossing cannot be layered", one dropped)."""
    _clean(_route("fx_pinned_crossing", tmp_path))


def test_fx_local_pour_far_pads(tmp_path):
    """Every pad of P is joined to the pour's copper: the far pads by traces, no via standing
    outside the pour (D6a: 4 stub vias in bare board, 4 unconnected)."""
    _clean(_route("fx_local_pour_far_pads", tmp_path))


def test_fx_same_layer_pour_stub(tmp_path):
    """P joins the B.Cu pour by a B.Cu track alone (no via, no signal connection to Q)."""
    m = _route("fx_same_layer_pour_stub", tmp_path)
    _clean(m)
    gnd = _conns(m, "GND")
    assert len(gnd) == 1 and gnd[0]["to"]["kind"] == "pour" and gnd[0]["vias"] == 0, gnd


def test_fx_coincident_pad_stub(tmp_path):
    """The pad pair joins the B.Cu pour by one stub and via (no signal connection to Q)."""
    m = _route("fx_coincident_pad_stub", tmp_path)
    _clean(m)
    gnd = _conns(m, "GND")
    assert len(gnd) == 1 and gnd[0]["to"]["kind"] == "pour" and gnd[0]["vias"] == 1, gnd


def test_fx_corner_wrap(tmp_path):
    """No out-and-back spike: no track is the exact reverse of another track of its net on its
    layer (D6b: SIG had one at (11.55, 16.63))."""
    m = _route("fx_corner_wrap", tmp_path)
    _clean(m)
    import pcbnew
    b = pcbnew.LoadBoard(str(next(Path(m["_out"]).glob("fx_corner_wrap-topor/*.kicad_pcb"))))
    segs = {}
    for t in b.GetTracks():
        if t.Type() != pcbnew.PCB_VIA_T:
            s, e = t.GetStart(), t.GetEnd()
            segs[(s.x, s.y, e.x, e.y, t.GetLayer(), t.GetNetCode())] = t
    spikes = [k for k in segs if (k[2], k[3], k[0], k[1], k[4], k[5]) in segs]
    assert not spikes, spikes


def test_fx_narrow_gap_forced(tmp_path):
    """Never drop (M-B1): the only way is a gap too narrow for the track. The connection is drawn
    through it anyway; KiCad's DRC sees it connected, with the clearance violations the engine
    reports (place, type, cause). With [optimize] allow_violations = false it stays unrouted and
    nothing breaks a rule (the old legal-only output)."""
    m = _route("fx_narrow_gap_forced", tmp_path)
    assert m["routed"] == m["connections"] == 1, m
    assert m["unconnected"] == 0, m
    assert m["routing_new"] >= 1 and set(m["drc_by_type_new"]) <= {"clearance"}, m
    viol = _result(m)["violations"]
    assert viol and all(v["kind"] == "clearance" and v["cause"] for v in viol), viol
    assert all(abs(v["at"][0] / 1e6 - 15.0) < 1.0 and abs(v["at"][1] / 1e6 - 10.0) < 1.0 for v in viol), viol
    # The least violating geometry: no copper overlap (a clearance violation, not a short); the
    # pick counts violations first (one against one wall pad beats one against each).
    assert all(0 < v["depth_nm"] < 200_000 for v in viol), viol
    legal = _route("fx_narrow_gap_forced", tmp_path, "--set", "optimize.allow_violations=false", tag="-legal")
    assert legal["routed"] == 0 and legal["routing_new"] == 0, legal
    assert not _result(legal)["violations"], _result(legal)["violations"]


def test_fx_terminal_clinch(tmp_path):
    """Crossing rules (M-B2): B no longer wraps A's terminal A1: A runs straight to A2 and B
    passes A1 on the south (before: A went around B's pad B2, detour 1.55, Luzin & Popov 2012
    Fig. 3a)."""
    m = _route("fx_terminal_clinch", tmp_path)
    _clean(m)
    assert m["vias"] == 0, m
    assert m["detour"]["max"] <= 1.35, m["detour"]


def test_fx_terminal_clinch_no_via(tmp_path):
    """The clinch where A cannot go around B and no via site anywhere near: legal copper, no
    violation (before: A crossed B on one layer, drawn through it as a short, never drop)."""
    m = _route("fx_terminal_clinch_no_via", tmp_path)
    _clean(m)
    assert not _result(m)["violations"], _result(m)["violations"]



def test_fx_neck_bottleneck(tmp_path):
    """Width in P2 (M-C): the plan necks the wide net from a pad end through the 0.7 mm gap, the
    drawing realises it: legal copper, one necked stretch reported (before M-C: drawn through the
    gap at full width as a clearance violation)."""
    m = _route("fx_neck_bottleneck", tmp_path)
    _clean(m)
    r = _result(m)
    assert not r["violations"], r["violations"]
    c = r["connections"][0]
    assert c["neck_nm"] > 0 and 200_000 <= c["neck_width_nm"] < 400_000, c
    assert m["detour"]["max"] <= 1.05, m["detour"]


@pytest.mark.xfail(strict=False, reason="open (M-C): wires from neighbouring 0.5 mm pins that turn to 45 degrees "
                   "right at the row's tips run 0.354 mm apart where 0.45 mm is needed; the planner's packer "
                   "compares passes edge by edge and does not see it (pin access / fanout angle); the pre-M-C "
                   "wheel routed this board legally with two vias by chance, the model fix alone not")
def test_fx_fine_pitch_exit(tmp_path):
    """Pad exits on fine pitch (M-C): every wire leaves its pad through the tip, no neighbour pad
    clipped, no forced copper. The P2 side of it (no exit through a side gap) is the engine test
    `topo_mc::a_wire_leaves_a_fine_pitch_pad_through_its_tip`."""
    m = _route("fx_fine_pitch_exit", tmp_path)
    _clean(m)
    assert not _result(m)["violations"], _result(m)["violations"]


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-m", "mechfix", "-q"]))
