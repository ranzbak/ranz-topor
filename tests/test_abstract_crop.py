"""harness/abstract_crop.py on a tiny synthetic board with a known hand layout (C0).

Board 30 x 20 mm, window [2, 4] - [15, 16] mm:
- SIG: pad A1 (5, 10) -> track on F.Cu along y = 10 -> pad B1 (25, 10) outside the window: one boundary terminal
  where the track crosses the window shrunk by half the track width, x = 15 - 0.125.
- N2: pads C1 (8, 13) and D1 (8, 7), joined by a B.Cu track that leaves the window (via x = 20): two terminals, and
  the hand copper inside the window is two pieces, so N2 is split in two (one keeps the name).
"""
import json
from pathlib import Path

import pcbnew
import pytest

from harness import abstract_crop as ac
from harness.mechfix import Fx

MM = 1_000_000


def _track(f: Fx, net: str, layer, pts, width=0.25):
    for a, c in zip(pts[:-1], pts[1:]):
        t = pcbnew.PCB_TRACK(f.b)
        t.SetStart(pcbnew.VECTOR2I(int(a[0] * MM), int(a[1] * MM)))
        t.SetEnd(pcbnew.VECTOR2I(int(c[0] * MM), int(c[1] * MM)))
        t.SetWidth(int(width * MM))
        t.SetLayer(layer)
        t.SetNet(f.nets[net])
        f.b.Add(t)


def _board(d: Path) -> Path:
    f = Fx(30, 20, nets=("SIG", "N2"))
    f.pad("A1", "SIG", (5, 10))
    f.pad("B1", "SIG", (25, 10))
    f.pad("C1", "N2", (8, 13), layer=pcbnew.B_Cu)
    f.pad("D1", "N2", (8, 7), layer=pcbnew.B_Cu)
    _track(f, "SIG", pcbnew.F_Cu, [(5, 10), (25, 10)])
    _track(f, "N2", pcbnew.B_Cu, [(8, 13), (20, 13), (20, 7), (8, 7)])
    return f.save(d / "hand.kicad_pcb")


@pytest.fixture(scope="module")
def crop(tmp_path_factory):
    d = tmp_path_factory.mktemp("crop")
    src = _board(d / "src")
    meta = ac.crop(src, ac.Rect(2, 4, 15, 16), d / "out", "fx")
    meta["reference_drc"] = ac.reference_drc(meta)
    return meta


def test_clip_segment():
    r = ac.Rect(0, 0, 10, 10)
    assert ac.clip_segment((-5, 5), (5, 5), r) == (0.5, 1.0)
    assert ac.clip_segment((-5, -5), (-1, -1), r) is None
    t0, t1 = ac.clip_segment((5, -5), (5, 15), r)
    assert (t0, t1) == (0.25, 0.75)


def test_boundary_terminals_at_the_hand_track_crossings(crop):
    t = {(x["net"].split("__")[0], x["layer"], round(x["at_mm"][0], 4), round(x["at_mm"][1], 4)) for x in crop["terminals"]}
    assert t == {("SIG", "F.Cu", 14.875, 10.0), ("N2", "B.Cu", 14.875, 13.0), ("N2", "B.Cu", 14.875, 7.0)}
    assert all(x["width_nm"] == 250_000 for x in crop["terminals"])
    # the window holds every pad inside it, nothing outside; the outline is the window grown by the edge clearance
    assert crop["window_mm"] == [2.0, 4.0, 15.0, 16.0]
    o, e = crop["outline_mm"], crop["edge_clearance_mm"]
    assert o[0] == pytest.approx(2 - e, abs=1e-5) and o[2] == pytest.approx(15 + e, abs=1e-5)
    b = pcbnew.LoadBoard(crop["problem"])
    refs = sorted(fp.GetReference() for fp in b.GetFootprints())
    assert refs == ["A1", "C1", "D1", "XT1", "XT2", "XT3"]
    assert len(list(b.GetTracks())) == 0  # the problem has no copper
    for fp in b.GetFootprints():  # never moved
        if fp.GetReference() == "A1":
            assert (fp.GetPosition().x, fp.GetPosition().y) == (5 * MM, 10 * MM)


def test_net_split_by_the_hand_connectivity_inside_the_window(crop):
    assert len(crop["net_splits"]) == 1
    s = crop["net_splits"][0]
    assert s["net"] == "N2" and s["new"] == "N2__crop1" and len(s["pads"]) == 2


def test_reference_is_drc_clean_against_the_crop(crop):
    drc = crop["reference_drc"]
    assert (drc["routing_total"], drc["routing_new"], drc["unconnected"], drc["unconnected_new"]) == (0, 0, 0, 0), drc
    b = pcbnew.LoadBoard(crop["reference_pcb"])
    assert len(list(b.GetTracks())) == 3  # SIG one piece, N2 two pieces (the part outside is cut away)
    meta = json.loads((Path(crop["problem"]).parent.parent / "crop.json").read_text())
    assert meta["reference"]["tracks"] == 3


def test_projects_are_refused(tmp_path):
    from harness.paths import PROJECTS
    with pytest.raises(SystemExit):
        ac._refuse_projects(PROJECTS / "x" / "y.kicad_pcb")
