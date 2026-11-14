"""Locked copper (the designer's locked tracks on bornhack2026, a locked arc on the synthetic fixture board)
survives rip, model export, the full route and the writer untouched, and no routing violation pairs router
copper with it."""
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

BOARD = "bornhack2026"
N_LOCKED = 17  # B.Cu 17 tracks on 8 signal / power nets (the designer's locked copper in snapshot 21c5f478f1af)
LOCKED_NETS = {"/BUZZER", "Net-(C67-Pad2)", "Net-(D4-A)", "Net-(Q3-D)", "Net-(Q3-G)", "Net-(R14-Pad2)",
               "Net-(U10-EN)", "VIN"}


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


def test_snapshot_has_17_locked_items(snap_copy):
    b = load_board(snap_copy)
    lk = _locked(b)
    assert len(lk) == N_LOCKED
    assert {str(t.GetNetname()) for t in lk} == LOCKED_NETS
    by = {}
    for t in lk:
        by.setdefault((b.GetLayerName(t.GetLayer()), t.GetClass()), 0)
        by[(b.GetLayerName(t.GetLayer()), t.GetClass())] += 1
    assert by == {("B.Cu", "PCB_TRACK"): 17}


def test_rip_all_keeps_locked_items(snap_copy, tmp_path):
    pc = copy_project(BENCH_BOARDS[BOARD], tmp_path / "w")
    b = load_board(pc)
    before = sorted(_geom(t) for t in _locked(b))
    rep = rip_tracks(b, mode="all")
    assert rep.kept_locked_tracks == N_LOCKED
    after = sorted(_geom(t) for t in _locked(b))
    assert after == before and len(after) == N_LOCKED


def _assert_kept_obstacles(m, want, n_locked):
    """Every locked item of `want` (uuid -> track/arc) is a kept, locked obstacle of model `m` with its exact
    layer, width, net and geometry; nothing else is locked."""
    nets = {n["code"]: n["name"] for n in m["nets"]}
    kept = {k["uuid"]: k for k in m["kept"]}
    assert len(m["kept"]) >= n_locked
    assert set(want) <= set(kept)
    for uid, t in want.items():
        k = kept[uid]
        assert k["locked"] is True
        assert k["layer"] == int(t.GetLayer()) and k["width"] == int(t.GetWidth())
        assert nets[k["net"]] == str(t.GetNetname())
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
    assert sum(1 for k in m["kept"] if k["locked"]) == n_locked  # nothing else is locked


def test_model_passes_locked_items_as_kept_obstacles(tmp_path):
    pc = copy_project(BENCH_BOARDS[BOARD], tmp_path / "w")
    b = load_board(pc)
    rules = resolve_rules(b, dru_text=pc.dru.read_text() if pc.dru else None,
                          widths_before=record_widths(b))
    rip_tracks(b, mode="all")
    want = {t.m_Uuid.AsString(): t for t in _locked(b)}
    _assert_kept_obstacles(export_model(b, rules, name=BOARD), want, N_LOCKED)


def test_model_passes_a_locked_arc_as_kept_obstacle(tmp_path):
    """The arc branch (no public snapshot board has a locked arc): the fixture board's arc and track, both
    locked, survive rip and become kept obstacles with their exact start / mid / end."""
    from harness.fixtures import make_fixture_board

    pcb = make_fixture_board(tmp_path / "fx" / "fixture.kicad_pcb")
    b = pcbnew.LoadBoard(str(pcb))
    for t in b.GetTracks():
        if t.GetClass() == "PCB_ARC":
            t.SetLocked(True)
    rules = resolve_rules(b, dru_text=None, widths_before=record_widths(b))
    before = sorted(_geom(t) for t in _locked(b))
    assert [g[0] for g in before].count("PCB_ARC") == 1 and len(before) == 2
    rep = rip_tracks(b, mode="all")
    assert rep.kept_locked_tracks == 2 and sorted(_geom(t) for t in _locked(b)) == before
    want = {t.m_Uuid.AsString(): t for t in _locked(b)}
    _assert_kept_obstacles(export_model(b, rules, name="fixture"), want, 2)


def test_bench_rip_keeps_vias_touched_by_locked_copper(tmp_path):
    """The M0-gate bookkeeping on a synthetic board (no public snapshot has a via touched by locked copper):
    run_bench's rip keeps the free via and the SIG transition via whose B.Cu track is locked, removes only
    unlocked copper, and both vias are still on the written board (kept_free + removed + touched == vias)."""
    from harness.fixtures import make_fixture_board
    from harness.writer import save

    pcb = make_fixture_board(tmp_path / "fx" / "fixture.kicad_pcb", locked_track=False, arc=False,
                             transition_via=True)
    b = pcbnew.LoadBoard(str(pcb))
    for t in b.GetTracks():
        if t.GetClass() == "PCB_TRACK" and t.GetLayer() == pcbnew.B_Cu:
            t.SetLocked(True)
    assert len(_locked(b)) == 1
    save(b, pcb)
    del b

    r = run_bench("fx", tmp_path / "run", router=None, rip="all", pcb_override=pcb)
    vias = r["orig_all"]["vias"]
    kept, removed = r["kept"]["kept_free_vias"], r["removed"]["removed_vias"]
    assert (vias, kept, removed) == (2, 1, 0)
    assert kept + removed + 1 == vias  # the 1 via touched by the locked track is retained
    assert r["kept"]["kept_locked_tracks"] == 1
    assert r["removed"]["removed_tracks"] == 1  # the unlocked F.Cu half of the transition
    assert r["routed_all"]["vias"] == vias and r["routed_all"]["segments"] == 1


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
