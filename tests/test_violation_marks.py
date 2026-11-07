"""User 2026-11-07: "Can you mark the violations with yellow in the images from now on?" KiCad's DRC JSON names
only each item's anchor (a track's start), so a marker at the anchors can sit far from the violation. With the
board's track geometry the marker sits between the closest points of the two items."""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "plugin"))
sys.path.insert(0, str(ROOT))

from toporlib.drc import violation_marks  # noqa: E402
from toporlib.pcbfile import track_lines  # noqa: E402

BOARD = """(kicad_pcb (version 20250000)
  (segment (start 0 0) (end 10 0) (width 0.2) (layer "F.Cu") (net 1) (uuid "t1"))
  (arc (start 0 5) (mid 5 7) (end 10 5) (width 0.2) (layer "F.Cu") (net 2) (uuid "a1"))
  (via (at 6 0.3) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (uuid "v1"))
)
"""


def _it(desc, x, y, uuid):
    return {"description": desc, "pos": {"x": x, "y": y}, "uuid": uuid}


def test_track_lines_reads_segments_arcs_and_vias():
    t = track_lines(BOARD)
    assert t["t1"] == [(0.0, 0.0, 10.0, 0.0)]
    assert t["a1"] == [(0.0, 5.0, 5.0, 7.0), (5.0, 7.0, 10.0, 5.0)]
    assert t["v1"] == [(6.0, 0.3, 6.0, 0.3)]


def test_marker_sits_at_the_violation_not_at_the_track_start():
    v = {"type": "clearance", "items": [_it("Track [A] on F.Cu", 0, 0, "t1"), _it("Via [B] on F.Cu - B.Cu", 6, 0.3, "v1")]}
    (x, y, typ), = violation_marks([v], track_lines(BOARD))
    assert typ == "clearance"
    assert abs(x - 6.0) < 1e-9 and abs(y - 0.15) < 1e-9  # halfway between (6, 0) on the track and the via


def test_zone_item_is_ignored_and_unknown_items_use_their_anchor():
    z = {"type": "clearance", "items": [_it("Zone [GND] on F.Cu", 177, 44, "z"), _it("Pad 1 [A] of U1 on F.Cu", 3, 4, "p")]}
    assert violation_marks([z], {}) == [(3.0, 4.0, "clearance")]
    assert violation_marks([{"type": "x", "items": [_it("Zone [GND]", 1, 1, "z")]}], {}) == []
