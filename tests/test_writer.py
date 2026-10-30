import math
import random

import pcbnew
import pytest

from harness.model import load_board
from harness.writer import save, write_items


def _items(sig):
    return [
        {"net": sig, "layer": 0, "width": 250_000,
         "prim": {"kind": "seg", "a": [20_000_000, 30_000_000], "b": [25_000_000, 30_000_000]}, "group": None},
        {"net": sig, "layer": 0, "width": 250_000,
         "prim": {"kind": "arc", "start": [25_000_000, 30_000_000], "mid": [27_000_000, 31_000_000],
                  "end": [29_000_000, 30_000_000]}, "group": None},
        {"kind": "via", "net": sig, "at": [29_000_000, 30_000_000], "d": 600_000, "drill": 300_000,
         "layers": [0, 2], "group": None},
    ]


def _by_uuid(board, uid):
    return [t for t in board.GetTracks() if t.m_Uuid.AsString() == uid]


def test_write_seg_arc_via_roundtrip(fixture_board, engine, tmp_path):
    b = load_board(fixture_board)
    sig = b.GetNetcodeFromNetname("SIG")
    ids = write_items(b, _items(sig))
    assert len(ids) == 3
    out = tmp_path / "w.kicad_pcb"
    save(b, out)
    b2 = pcbnew.LoadBoard(str(out))
    arcs = [t for t in b2.GetTracks() if t.GetClass() == "PCB_ARC" and t.m_Uuid.AsString() == ids[1]]
    assert len(arcs) == 1
    c = arcs[0].GetCenter()
    ex = engine.arc_center_nm(25_000_000, 30_000_000, 27_000_000, 31_000_000, 29_000_000, 30_000_000)
    assert abs(c.x - ex[0]) <= 100 and abs(c.y - ex[1]) <= 100
    assert arcs[0].GetStart() == pcbnew.VECTOR2I(25_000_000, 30_000_000)
    assert arcs[0].GetEnd() == pcbnew.VECTOR2I(29_000_000, 30_000_000)
    assert arcs[0].GetWidth() == 250_000 and arcs[0].GetNetCode() == sig
    seg = _by_uuid(b2, ids[0])[0]
    assert seg.GetEnd() == arcs[0].GetStart()
    assert seg.GetLayer() == pcbnew.F_Cu
    via = _by_uuid(b2, ids[2])[0]
    assert via.GetClass() == "PCB_VIA"
    assert via.GetViaType() == pcbnew.VIATYPE_THROUGH
    assert via.GetWidth(pcbnew.F_Cu) == 600_000 and via.GetDrillValue() == 300_000
    assert (via.TopLayer(), via.BottomLayer()) == (pcbnew.F_Cu, pcbnew.B_Cu)
    assert via.GetPosition() == pcbnew.VECTOR2I(29_000_000, 30_000_000)


def test_write_group(fixture_board, tmp_path):
    b = load_board(fixture_board)
    sig = b.GetNetcodeFromNetname("SIG")
    n_groups = len(list(b.Groups()))
    items = _items(sig)[:2]
    for it in items:
        it["group"] = "topor-partial"
    write_items(b, items)
    groups = [g for g in b.Groups() if g.GetName() == "topor-partial"]
    assert len(groups) == 1 and len(list(b.Groups())) == n_groups + 1
    assert len(list(groups[0].GetItems())) == 2
    # reused on a second call
    write_items(b, [dict(_items(sig)[0], group="topor-partial")])
    groups = [g for g in b.Groups() if g.GetName() == "topor-partial"]
    assert len(groups) == 1 and len(list(groups[0].GetItems())) == 3
    out = tmp_path / "g.kicad_pcb"
    save(b, out)
    b2 = pcbnew.LoadBoard(str(out))
    assert [len(list(g.GetItems())) for g in b2.Groups() if g.GetName() == "topor-partial"] == [3]


@pytest.mark.gate
def test_random_arcs_center_matches_engine(fixture_board, engine, tmp_path):
    rng = random.Random(11)
    b = load_board(fixture_board)
    sig = b.GetNetcodeFromNetname("SIG")
    items = []
    while len(items) < 200:
        r = rng.uniform(0.3e6, 20e6)
        sweep = math.radians(rng.uniform(5, 270))
        if 2 * r * math.sin(sweep / 4) ** 2 < 1000:  # sagitta >= 1 um
            continue
        a0 = rng.uniform(0, 2 * math.pi)
        cx, cy = 25e6, 20e6
        p = [[round(cx + r * math.cos(a0 + f * sweep)), round(cy + r * math.sin(a0 + f * sweep))]
             for f in (0, 0.5, 1)]
        items.append({"net": sig, "layer": 0, "width": 200_000,
                      "prim": {"kind": "arc", "start": p[0], "mid": p[1], "end": p[2]}, "group": None})
    ids = write_items(b, items)
    out = tmp_path / "r.kicad_pcb"
    save(b, out)
    b2 = pcbnew.LoadBoard(str(out))
    by = {t.m_Uuid.AsString(): t for t in b2.GetTracks() if t.GetClass() == "PCB_ARC"}
    for uid, it in zip(ids, items):
        pr = it["prim"]
        ex = engine.arc_center_nm(*pr["start"], *pr["mid"], *pr["end"])
        c = by[uid].GetCenter()
        assert abs(c.x - ex[0]) <= 100 and abs(c.y - ex[1]) <= 100


def _kept(uuid, a, b, width=250_000, net=3, layer=0):
    return {"uuid": uuid, "net": net, "layer": layer, "width": width,
            "prim": {"kind": "seg", "a": a, "b": b}, "locked": False}


def test_skip_unchanged_keeps_identical_hand_items():
    """T9c: an emitted item identical to the hand item it replaces is a no-op (uuid kept), so
    pre-existing DRC violations on unchanged copper keep their key."""
    from harness.writer import skip_unchanged
    kept = [_kept("A", [0, 0], [10, 0]), _kept("B", [10, 0], [20, 5]), _kept("C", [0, 9], [9, 9])]
    same_rev = {"net": 3, "layer": 0, "width": 250_000, "prim": {"kind": "seg", "a": [10, 0], "b": [0, 0]}}
    new = {"net": 3, "layer": 0, "width": 250_000, "prim": {"kind": "seg", "a": [10, 0], "b": [20, 6]}}
    wider = {"net": 3, "layer": 0, "width": 300_000, "prim": {"kind": "seg", "a": [0, 9], "b": [9, 9]}}
    rm, items, n = skip_unchanged(kept, ["A", "B", "C"], [same_rev, new, wider])
    assert (rm, items, n) == (["B", "C"], [new, wider], 1)
    # an item is only a no-op against a hand item that is being removed
    rm, items, n = skip_unchanged(kept, ["B"], [same_rev])
    assert (rm, items, n) == (["B"], [same_rev], 0)


def test_skip_unchanged_arc_and_via_passthrough():
    from harness.writer import skip_unchanged
    arc = {"kind": "arc", "start": [0, 0], "mid": [5, 2], "end": [10, 0]}
    kept = [{"uuid": "R", "net": 3, "layer": 2, "width": 200_000, "prim": arc, "locked": False}]
    same = {"net": 3, "layer": 2, "width": 200_000, "prim": dict(arc)}
    via = {"kind": "via", "net": 3, "at": [0, 0], "d": 600_000, "drill": 300_000, "layers": [0, 2]}
    assert skip_unchanged(kept, ["R"], [via, same]) == ([], [via], 1)
