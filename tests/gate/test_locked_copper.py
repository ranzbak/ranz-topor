"""Locked copper (the user's locked edge GND trace on dcsgonebadge) survives rip, model export,
the full route and the writer untouched, and no routing violation pairs router copper with it."""
import json
from pathlib import Path

import pcbnew
import pytest

from harness.bench import run_bench
from harness.copy import copy_project
from harness.drc import ROUTING_TYPES
from harness.model import export_model, load_board
from harness.paths import BENCH_BOARDS
from harness.rip import record_widths, rip_tracks
from harness.router import route_router
from harness.rules import resolve_rules

BOARD = "dcsgonebadge"
N_LOCKED = 22  # B.Cu 16 tracks + 4 arcs, F.Cu 2 tracks (user's locked edge GND trace)


def _geom(t):
    """Layer-, net- and geometry-exact key of a track/arc (endpoint order insensitive)."""
    ends = tuple(sorted(((int(p.x), int(p.y)) for p in (t.GetStart(), t.GetEnd()))))
    mid = (int(t.GetMid().x), int(t.GetMid().y)) if t.GetClass() == "PCB_ARC" else None
    return (t.GetClass(), int(t.GetLayer()), str(t.GetNetname()), ends, mid, int(t.GetWidth()))


def _locked(board):
    return [t for t in board.GetTracks() if t.GetClass() != "PCB_VIA" and t.IsLocked()]


@pytest.fixture(scope="module")
def snap_copy(tmp_path_factory):
    return copy_project(BENCH_BOARDS[BOARD], tmp_path_factory.mktemp("locked") / "w")


def test_snapshot_has_22_locked_gnd_items(snap_copy):
    b = load_board(snap_copy)
    lk = _locked(b)
    assert len(lk) == N_LOCKED
    assert {str(t.GetNetname()) for t in lk} == {"GND"}
    by = {}
    for t in lk:
        by.setdefault((b.GetLayerName(t.GetLayer()), t.GetClass()), 0)
        by[(b.GetLayerName(t.GetLayer()), t.GetClass())] += 1
    assert by == {("B.Cu", "PCB_TRACK"): 16, ("B.Cu", "PCB_ARC"): 4, ("F.Cu", "PCB_TRACK"): 2}


def test_rip_all_keeps_locked_items(snap_copy, tmp_path):
    pc = copy_project(BENCH_BOARDS[BOARD], tmp_path / "w")
    b = load_board(pc)
    before = sorted(_geom(t) for t in _locked(b))
    rep = rip_tracks(b, mode="all")
    assert rep.kept_locked_tracks == N_LOCKED
    after = sorted(_geom(t) for t in _locked(b))
    assert after == before and len(after) == N_LOCKED


def test_model_passes_locked_items_as_kept_obstacles(tmp_path):
    pc = copy_project(BENCH_BOARDS[BOARD], tmp_path / "w")
    b = load_board(pc)
    rules = resolve_rules(b, dru_text=pc.dru.read_text() if pc.dru else None,
                          widths_before=record_widths(b))
    rip_tracks(b, mode="all")
    want = {t.m_Uuid.AsString(): t for t in _locked(b)}
    m = export_model(b, rules, name="dcs")
    nets = {n["code"]: n["name"] for n in m["nets"]}
    kept = {k["uuid"]: k for k in m["kept"]}
    assert len(m["kept"]) >= N_LOCKED
    assert set(want) <= set(kept)
    for uid, t in want.items():
        k = kept[uid]
        assert k["locked"] is True
        assert k["layer"] == int(t.GetLayer()) and k["width"] == int(t.GetWidth())
        assert nets[k["net"]] == "GND" == str(t.GetNetname())
        p = k["prim"]
        if t.GetClass() == "PCB_ARC":
            assert p["kind"] == "arc"
            assert (p["start"], p["mid"], p["end"]) == (
                [int(t.GetStart().x), int(t.GetStart().y)], [int(t.GetMid().x), int(t.GetMid().y)],
                [int(t.GetEnd().x), int(t.GetEnd().y)])
        else:
            assert p["kind"] == "seg"
            assert (p["a"], p["b"]) == ([int(t.GetStart().x), int(t.GetStart().y)],
                                        [int(t.GetEnd().x), int(t.GetEnd().y)])
    assert sum(1 for k in m["kept"] if k["locked"]) == N_LOCKED  # nothing else is locked


@pytest.mark.gate
def test_full_route_leaves_locked_copper_untouched_and_drc_clean_of_it(tmp_path):
    src = BENCH_BOARDS[BOARD]
    orig = load_board(copy_project(src, tmp_path / "orig"))
    want = sorted(_geom(t) for t in _locked(orig))
    orig_uuids = {t.m_Uuid.AsString() for t in orig.GetTracks()}
    locked_uuids = {t.m_Uuid.AsString() for t in _locked(orig)}
    assert len(want) == N_LOCKED
    del orig

    r = run_bench(BOARD, tmp_path / "run", router=route_router, rip="all", threads=4,
                  options={"wall_clock_s": 60, "partial_routes": False})
    out = tmp_path / "run"
    routed = pcbnew.LoadBoard(str(out / f"{BOARD}-work" / src.name))
    got = sorted(_geom(t) for t in _locked(routed))
    assert got == want  # (c) identical layer/net/geometry/width
    routed_new = {t.m_Uuid.AsString() for t in routed.GetTracks()} - orig_uuids

    # (d) no routing violation pairs a router-written item with a locked item
    base = {(v["type"], v["description"], tuple(sorted(str(i.get("uuid", "")) for i in v["items"])))
            for v in json.loads((out / f"{BOARD}--drc-orig.json").read_text())["violations"]}
    doc = json.loads((out / f"{BOARD}--drc-routed.json").read_text())
    pairs = []
    for v in doc["violations"]:
        if v["type"] not in ROUTING_TYPES:
            continue
        ids = [str(i.get("uuid", "")) for i in v["items"]]
        if (v["type"], v["description"], tuple(sorted(ids))) in base:
            continue
        if any(i in locked_uuids for i in ids) and any(i in routed_new for i in ids):
            pairs.append((v["type"], v["description"], [(i.get("description"), i.get("pos"))
                                                         for i in v["items"]]))
    print(f"LOCKED-INFO routed {r['route']['routed']}/{r['route']['connections']}, "
          f"locked-vs-router violations: {len(pairs)}")
    for p in pairs:
        print("LOCKED-VIOLATION", p)
    assert not pairs
