"""Pair-via site proposal (user-approved 2026-11-13) end to end: a diff pair must change layer in a GND-walled
channel that holds its two-via disc only where it widens (harness.fixtures.make_pair_via_board). The plan decides
the site (the graded via-site price, items 1/2/4); the drawing realises it, KiCad DRC checks the copper. Without
room anywhere the via pair is still placed and reported (DIFF_PAIR_VIA_FORCED), never dropped.

The expected answer comes from the fixture geometry (channel 1.24 mm: band 1.0 mm fits, disc 1.0 + 2 x 0.2 mm
does not; the wide part x 5 .. 9 mm holds it), not from the tool. Needs an engine that reports pair vias (skipped
with an older one)."""
from __future__ import annotations

from pathlib import Path

import pytest

pcbnew = pytest.importorskip("pcbnew")

from harness.fixtures import PAIR_VIA_CLASSES, make_pair_via_board, write_project_file  # noqa: E402


def _route(tmp_path: Path, wide: bool):
    from harness.route_board import route_board
    pcb = make_pair_via_board(tmp_path / "pv" / "pairvia.kicad_pcb", wide=wide)
    write_project_file(pcb, rules={"min_via_diameter": 0.4, "min_through_hole_diameter": 0.3}, classes=PAIR_VIA_CLASSES)
    m = route_board(str(pcb), tmp_path / "out", png=False)
    res = next((tmp_path / "out").rglob("result.json"), None)
    import json
    r = json.loads(res.read_text()) if res else {}
    warns = [str(w) for w in (r.get("route") or {}).get("warnings") or r.get("warnings") or []]
    if not any(w.startswith("pair vias, plan vs drawn") for w in warns):
        pytest.skip("this engine does not report pair vias")
    out = next((tmp_path / "out").glob("*-topor/*.kicad_pcb"))
    b = pcbnew.LoadBoard(str(out))
    vias = [(t.GetNetname(), t.GetPosition().x / 1e6, t.GetPosition().y / 1e6) for t in b.GetTracks() if t.GetClass() == "PCB_VIA"]
    return m, warns, vias


def test_pair_via_goes_where_the_channel_holds_it(tmp_path):
    m, warns, vias = _route(tmp_path, wide=True)
    assert m["unconnected"] == 0, m
    assert len(vias) == 2 and {v[0] for v in vias} == {"USB+", "USB-"}, vias
    assert all(5.0 <= x <= 9.0 for _, x, _ in vias), vias
    assert m["routing_new"] == 0, m.get("drc_by_type_new")
    assert not any(w.startswith("pair via forced:") for w in warns), warns


def test_pair_via_without_room_is_placed_and_reported(tmp_path):
    m, warns, vias = _route(tmp_path, wide=False)
    assert m["unconnected"] == 0, m
    assert len(vias) == 2, vias
    assert sum(w.startswith("pair via forced: diff pair USB+ / USB-") for w in warns) == 1, warns
