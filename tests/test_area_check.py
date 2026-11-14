"""Whole-area check and ERBS acceptance (M3 merge gate): synthetic copper and dumps, no engine or kicad-cli."""
import json
import math

import pytest

shapely = pytest.importorskip("shapely")

from shapely.geometry import box  # noqa: E402

from harness import area_check as AC  # noqa: E402
from harness import erbs_accept as EA  # noqa: E402

PCB = """(kicad_pcb (version 20260206)
 (layers (0 "F.Cu" signal) (2 "B.Cu" signal))
 (segment (start 0 0) (end 10 0) (width 0.2) (layer "F.Cu") (net "A") (uuid "a1"))
 (segment (start 0 0.3) (end 10 0.3) (width 0.2) (layer "F.Cu") (net "B") (uuid "b1"))
 (segment (start 0 5) (end 10 5) (width 0.2) (layer "F.Cu") (net "C") (uuid "c1"))
 (segment (start 10 5) (end 0 5.05) (width 0.2) (layer "F.Cu") (net "C") (uuid "c2"))
 (segment (start 0 8) (end 3 9) (width 0.2) (layer "F.Cu") (net "D") (uuid "d1"))
 (footprint "x" (layer "F.Cu") (uuid "f1") (at 20 20 90)
  (property "Reference" "R1")
  (pad "1" smd rect (at 1 0 90) (size 0.5 1) (layers "F.Cu") (net "A") (uuid "p1")))
)
"""


def _cu(tmp_path, clearance=0.2):
    f = tmp_path / "b.kicad_pcb"
    f.write_text(PCB)
    pro = tmp_path / "b.kicad_pro"
    pro.write_text(json.dumps({"net_settings": {"classes": [{"name": "Default", "clearance": clearance}]}}))
    return AC.Copper.from_pcb(f, pro)


def test_deficit_is_length_weighted(tmp_path):
    cu = _cu(tmp_path)
    ms = AC.measure_samples(cu)
    s = AC.region_stats(cu, ms, [], box(-1, -1, 11, 1))
    # gap 0.3 - 0.2 = 0.1 against 0.2: deficit 0.1 along 10 mm of each of the two tracks
    assert s["deficit_mm2"] == pytest.approx(2.0, rel=0.02)
    assert s["deficit_len_mm"] == pytest.approx(20.0, rel=0.02)
    assert s["worst_deficit_mm"] == pytest.approx(0.1, abs=1e-3)
    clean = AC.region_stats(cu, ms, [], box(-1, 7, 11, 10))
    assert clean["deficit_mm2"] == 0.0


def test_same_net_copper_laid_over_itself(tmp_path):
    cu = _cu(tmp_path)
    ms = AC.measure_samples(cu)
    s = AC.region_stats(cu, ms, [], box(-1, 4, 11, 6))
    # c2 runs back over c1: both covered over most of their length (the joint's neighbourhood is exempt)
    assert s["excess_len_mm"] > 15.0
    assert s["deficit_mm2"] == 0.0


def test_any_angle(tmp_path):
    cu = _cu(tmp_path)
    ms = AC.measure_samples(cu)
    s = AC.region_stats(cu, ms, [], box(-1, 7, 11, 10))
    assert s["anyangle"] == 1 and s["anyangle_len_mm"] == pytest.approx(math.hypot(3, 1), rel=0.02)
    assert AC.region_stats(cu, ms, [], box(-1, -1, 11, 1))["anyangle"] == 0


def test_pad_transform(tmp_path):
    cu = _cu(tmp_path)
    pad = next(o for o in cu.objs if o["kind"] == "pad")
    # footprint at (20, 20) rotated 90 deg: the local (1, 0) goes to (20, 19) (KiCad: y down, ccw on screen)
    assert pad["c"] == pytest.approx((20.0, 19.0))


def test_drc_place_between_items(tmp_path):
    cu = _cu(tmp_path)
    drc = {"violations": [{"type": "clearance", "items": [
        {"description": "Track [A] on F.Cu", "uuid": "a1", "pos": {"x": 0, "y": 0}},
        {"description": "Track [B] on F.Cu", "uuid": "b1", "pos": {"x": 0, "y": 0.3}}]}]}
    pl = AC.drc_places(cu, drc)
    assert len(pl) == 1 and pl[0][0] == "clearance" and 0.0 < pl[0][2] < 0.3
    ms = AC.measure_samples(cu)
    assert AC.region_stats(cu, ms, pl, box(-1, -1, 11, 1))["drc"] == 1
    assert AC.region_stats(cu, ms, pl, box(-1, 4, 11, 6))["drc"] == 0


def _dump(path, band, gates):
    return {"stretch": 2, "wire": 2, "net": 1, "band": band, "path": path, "gates": gates, "converged": True, "_layer": "F.Cu"}


def test_erbs_accept_straight_and_zigzag():
    band = [[0, 0], [10, 0]]
    ok = EA.stretch_row(_dump([[0, 0], [10, 0]], band, []))
    assert ok["ratio"] == 1.0 and ok["reversals"] == 0 and ok["bends"] == 0
    # a bend at a spoke tip whose root lies on the inner side: explained
    g = {"src": "cut", "a": [5, 0.5], "b": [5, 3], "p": [5, -1], "q": [5, 4]}
    r = EA.stretch_row(_dump([[0, 0], [5, 0.5], [10, 0]], band, [g]))
    assert r["bends"] == 1 and r["unexplained"] == 0
    # a corner in the middle of a portal, and a backward leg: the zigzag
    z = EA.stretch_row(_dump([[0, 0], [6, 1], [4, 1.2], [10, 0]], band, [g]))
    assert z["unexplained"] >= 2 and z["reversals"] == 1 and z["back_um"] > 1000
    assert z["ratio"] > 1.03


def test_erbs_accept_tip_at_its_cut_end():
    """A tip at its cut's end (a spoke of length 0, an open end): the spoke lies past the tip along the cut
    (from p to q for tip b), so a bend there turning toward that side is explained, one turning away is not."""
    band = [[0, 0], [10, 0]]
    g = {"src": "cut", "a": [5, 4], "b": [5, 0.5], "p": [5, 4], "q": [5, 0.5]}
    toward = EA.stretch_row(_dump([[0, 0], [5, 0.5], [10, 0]], band, [g]))
    assert toward["bends"] == 1 and toward["unexplained"] == 0
    away = EA.stretch_row(_dump([[0, 0], [5, 0.5], [10, 0]], band, [dict(g, p=[5, -3], a=[5, -3])]))
    assert away["unexplained"] == 1
