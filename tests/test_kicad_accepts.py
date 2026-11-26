"""harness.kicad_accepts: the pad_centre project and what KiCad's real Cleanup Tracks & Vias did to it.

Recorded from the real dialog (draw-via-neck r7, 2026-11-26: tools/gui_e2e.sh --existing --cleanup --display :6,
every box on, KiCad 10.0.8; the replay harness.cleanup_check removed exactly the same): of the nine cases only the
JOG's in-pad leg is removed (track_in_pad). The T joins inside the pad (0.17 / 0.40 / 0.75 mm off its centre, on an
axis host and on a 45-degree host) and the off-centre ends without a host stay: by the user's test ("if the track is
gone after the cleanup, it was not good") KiCad accepts them. The replay keeps this result as a regression check."""
from pathlib import Path

import pytest

pcbnew = pytest.importorskip("pcbnew")

from harness import cleanup_check, kicad_accepts  # noqa: E402

FIX = Path(__file__).parent / "fixtures" / "kicad_accepts" / "pad_centre.kicad_pcb"

#: the real dialog's removals on FIX: net -> [(start mm, end mm)]
DIALOG_REMOVED = {"JOG": [((24.0, 12.0), (23.915, 12.085))]}  # cleanup_check reports 4 decimals


def _removed(pcb: Path) -> dict:
    out: dict = {}
    for i in cleanup_check.cleanup_items(pcb):
        a = i["at"]
        out.setdefault(i["net"], []).append(((round(a[0], 4), round(a[1], 4)), (round(a[2], 4), round(a[3], 4))))
        assert i["rule"] == "track_in_pad", i
    return out


def test_the_fixture_is_the_generators_board(tmp_path):
    """The kept project is what make_pad_centre writes (same tracks per net)."""
    new = kicad_accepts.make_pad_centre(tmp_path)
    assert kicad_accepts.tracks_by_net(new) == kicad_accepts.tracks_by_net(FIX)
    assert sorted(kicad_accepts.tracks_by_net(FIX)) == sorted(kicad_accepts.CASES)


def test_cleanup_removes_only_the_jog_kicad_deletes():
    """KiCad's Cleanup on the pad_centre project, every option on: only the 5.35 um jog goes (its polygon lies in
    the pad); the T joins in the pad, the off-centre ends and the jog 1 um further out stay."""
    assert _removed(FIX) == DIALOG_REMOVED
