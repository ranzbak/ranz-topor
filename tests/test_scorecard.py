"""harness.scorecard: signal vs stitching via split and the median [min–max] spread."""
from harness.scorecard import _spread, via_split

BOARD = """(kicad_pcb (version 20250000)
  (segment (start 10 10) (end 20 10) (width 0.25) (layer "F.Cu") (net 1) (uuid "s1"))
  (via (at 20 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "v1"))
  (via (at 30 30) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (uuid "v2"))
  (via (at 10 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (uuid "v3"))
)
"""


def test_via_split_signal_needs_a_track_of_its_net_ending_on_it(tmp_path):
    p = tmp_path / "b.kicad_pcb"
    p.write_text(BOARD)
    # v1: net-1 track ends on it -> signal; v2: nothing ends on it -> stitching;
    # v3: a track end lies on it but of another net -> stitching
    assert via_split(p) == (1, 2)


def test_spread_is_median_with_range():
    assert _spread([1.0, 3.0, 2.0]) == "2.0 [1.0–3.0]"
    assert _spread([1.234], ".2f") == "1.23"
    assert _spread([None]) == "–"
