"""The DRC-cluster pattern generator (harness/drc_patterns.py): every pattern builds, its reference keeps every
clearance (shapely; KiCad DRC when kicad-cli is there), kept copper is on the problem board, flipped footprints land
on B.Cu where the pattern says. The patterns themselves are PROPOSED: no router run here."""
import json
import math

import pcbnew
import pytest

from harness import drc_patterns as dp
from harness.paths import kicad_cli

GENERIC_NETS = {"DP", "DN", "S1", "S2", "A", "B", "C", "P3V3", "GND", "SDA", "SCL", "VX", "P", "N", "X", "SIG"}


@pytest.mark.parametrize("name", list(dp.PATTERNS))
def test_reference_keeps_clearance_and_names_are_generic(name):
    pat = dp.PATTERNS[name]()
    assert dp.clearance_report(pat) == []
    assert set(pat.nets) <= GENERIC_NETS
    pads = {f"{f.ref}.{p.num}": p.net for f in pat.fps for p in f.pads}
    for net, a, b in pat.connections:
        assert pads[a] == net and pads[b] == net, (net, a, b)
    assert all(p.net in pat.nets for f in pat.fps for p in f.pads if p.net)


def test_clearance_report_sees_a_slip():
    pat = dp.via_pair_cross()
    pat.ref_vias[0].pos = (5.0, 4.0)  # A's via moved into the crossing: B's B.Cu wire passes through it
    bad = dp.clearance_report(pat)
    assert any(b["a"].startswith("track B") or b["b"].startswith("track B") for b in bad)


@pytest.mark.parametrize("name", list(dp.PATTERNS))
def test_problem_board_has_only_kept_copper(name, tmp_path):
    pat = dp.PATTERNS[name]()
    prob = dp.write_board(pat, tmp_path / "p" / f"{name}.kicad_pcb", copper=False)
    ref = dp.write_board(pat, tmp_path / "r" / f"{name}.kicad_pcb", copper=True)
    nb_prob = len(pcbnew.LoadBoard(str(prob)).GetTracks())
    nb_ref = len(pcbnew.LoadBoard(str(ref)).GetTracks())
    kept = sum(len(t.pts) - 1 for t in pat.kept) + len(pat.kept_arcs)
    assert nb_prob == kept
    assert nb_ref == kept + sum(len(t.pts) - 1 for t in pat.ref_tracks) + len(pat.ref_vias)
    assert json.loads(prob.with_suffix(".kicad_pro").read_text())["board"]["design_settings"]["rules"]["min_clearance"] == dp.CL


def test_flipped_roundrect_lands_on_b_cu(tmp_path):
    pat = dp.roundrect_corner_pass()
    b = pcbnew.LoadBoard(str(dp.write_board(pat, tmp_path / "x.kicad_pcb", copper=False)))
    fp = next(f for f in b.GetFootprints() if f.GetReference() == "C1")
    assert fp.IsFlipped()
    got = {p.GetNumber(): p for p in fp.Pads()}
    for want in next(f for f in pat.fps if f.ref == "C1").pads:
        p = got[want.num]
        assert p.IsOnLayer(pcbnew.B_Cu) and not p.IsOnLayer(pcbnew.F_Cu)
        assert p.GetShape() == pcbnew.PAD_SHAPE_ROUNDRECT
        assert math.dist((p.GetPosition().x / 1e6, p.GetPosition().y / 1e6), want.pos) < 1e-6


def test_kept_arc_round_trip_and_polyline(tmp_path):
    pat = dp.kept_wide_gnd_arc()
    a = pat.kept_arcs[0]
    pts = dp.arc_points(a)
    assert math.dist(pts[0], a.start) < 1e-9 and math.dist(pts[-1], a.end) < 1e-9
    c = (12.9, 10.5)
    assert all(abs(math.dist(p, c) - 2.5) < 1e-4 for p in pts)  # mid is rounded to 0.1 um
    assert min(math.dist(p, a.mid) for p in pts) < 0.05  # the polyline passes the mid point (the short way round)
    b = pcbnew.LoadBoard(str(dp.write_board(pat, tmp_path / "k.kicad_pcb", copper=False)))
    arcs = [t for t in b.GetTracks() if t.GetClass() == "PCB_ARC"]
    assert len(arcs) == 1 and arcs[0].IsLocked() and abs(arcs[0].GetRadius() / 1e6 - 2.5) < 1e-3


def test_bottleneck_is_narrower_than_one_wire():
    pat = dp.kept_wide_gnd_arc()
    assert 0.15 < pat.expect["b_cu_room"] < dp.W + 2 * dp.CL


@pytest.mark.skipif(not kicad_cli().exists(), reason="no kicad-cli")
@pytest.mark.parametrize("name", list(dp.PATTERNS))
def test_reference_passes_kicad_drc(name, tmp_path):
    dp.generate(name, tmp_path, drc=True)
    doc = json.loads((tmp_path / name / "expected.json").read_text())
    assert doc["reference_drc"]["violations"] == 0, doc["reference_drc"]["detail"]
    assert doc["reference_drc"]["unconnected"] == 0
    assert doc["rules_assumed"] == {"authority": "board"}
