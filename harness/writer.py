"""Write engine output items (segments, arcs, vias, groups) into a board."""
from __future__ import annotations

from pathlib import Path

import pcbnew


def _v(p) -> "pcbnew.VECTOR2I":
    return pcbnew.VECTOR2I(int(p[0]), int(p[1]))


def _uuid(item) -> str:
    return item.m_Uuid.AsString()


def _find_group(board, name: str):
    for g in board.Groups():
        if g.GetName() == name:
            return g
    return None


def _group_for(board, groups: dict, name: str):
    g = groups.get(name)
    if g is None:
        g = _find_group(board, name)
        if g is None:
            g = pcbnew.PCB_GROUP(board)
            g.SetName(name)
            board.Add(g)
        groups[name] = g
    return g


def _make_item(board, it: dict):
    if it.get("kind") == "via":
        v = pcbnew.PCB_VIA(board)
        v.SetViaType(pcbnew.VIATYPE_THROUGH)
        v.SetPosition(_v(it["at"]))
        v.SetWidth(int(it["d"]))
        if it.get("drill"):
            v.SetDrill(int(it["drill"]))
        layers = it.get("layers") or [pcbnew.F_Cu, pcbnew.B_Cu]
        v.SetLayerPair(int(layers[0]), int(layers[-1]))
        v.SetNetCode(int(it["net"]))
        return v
    prim = it["prim"]
    if prim["kind"] == "arc":
        t = pcbnew.PCB_ARC(board)
        t.SetStart(_v(prim["start"]))
        t.SetMid(_v(prim["mid"]))
        t.SetEnd(_v(prim["end"]))
    else:
        t = pcbnew.PCB_TRACK(board)
        t.SetStart(_v(prim["a"]))
        t.SetEnd(_v(prim["b"]))
    t.SetWidth(int(it["width"]))
    t.SetLayer(int(it["layer"]))
    t.SetNetCode(int(it["net"]))
    return t


def _copper_key(it: dict) -> tuple | None:
    """Identity of a track/arc's copper (direction-free); None for vias."""
    prim = it.get("prim")
    if prim is None:
        return None
    if prim["kind"] == "arc":
        s, e = tuple(prim["start"]), tuple(prim["end"])
        geo = ("arc", min(s, e), tuple(prim["mid"]), max(s, e))
    else:
        a, b = tuple(prim["a"]), tuple(prim["b"])
        geo = ("seg", min(a, b), max(a, b))
    return (int(it["net"]), int(it["layer"]), int(it["width"])) + geo


def skip_unchanged(kept: list[dict], remove_uuids: list[str],
                   items: list[dict]) -> tuple[list[str], list[dict], int]:
    """Drop each (emitted item, removed hand item) pair with identical copper.

    A wire the smoother leaves straight re-emits its hand item unchanged; deleting and
    re-adding it would only change its uuid (DRC baselines key on uuids) and drop its
    teardrops. `kept` is the model's kept list. Returns `(remove_uuids, items, n_unchanged)`.
    """
    removing = set(remove_uuids)
    free: dict[tuple, list[str]] = {}
    for k in kept:
        if k["uuid"] in removing:
            free.setdefault(_copper_key(k), []).append(k["uuid"])
    keep: set[str] = set()
    out: list[dict] = []
    for it in items:
        same = free.get(_copper_key(it)) if it.get("prim") is not None else None
        if same:
            keep.add(same.pop(0))
        else:
            out.append(it)
    return [u for u in remove_uuids if u not in keep], out, len(keep)


def write_items(board, items: list[dict]) -> list[str]:
    """Add items to the board; return the uuids of created items in input order."""
    groups: dict = {}
    ids: list[str] = []
    for it in items:
        obj = _make_item(board, it)
        board.Add(obj)
        name = it.get("group")
        if name:
            _group_for(board, groups, name).AddItem(obj)
        ids.append(_uuid(obj))
    return ids


def save(board, path) -> Path:
    path = Path(path)
    pcbnew.SaveBoard(str(path), board)
    return path


def _is_via(t) -> bool:
    return t.Type() == pcbnew.PCB_VIA_T


def remove_items(board, uuids: list[str]) -> int:
    """Delete tracks/arcs/vias by uuid (board.Delete, never Remove); unknown uuids are ignored."""
    return remove_items_and_teardrops(board, uuids)[0]


def remove_items_and_teardrops(board, uuids: list[str]) -> tuple[int, int]:
    """Delete tracks/arcs/vias by uuid, plus teardrop zones attached to them.

    A teardrop zone (`zone.IsTeardropArea()`) goes when a removed item has the same net, the zone
    is on a layer the item is on, and the bounding boxes overlap or touch. Returns
    `(items_removed, teardrops_removed)`; unknown uuids are ignored. `board.Delete`, never Remove.
    """
    want = set(uuids)
    if not want:
        return 0, 0
    doomed = [t for t in board.GetTracks() if _uuid(t) in want]
    # snapshot (net, layers, bbox) before the deletes invalidate the items
    probes = []
    for t in doomed:
        bb = t.GetBoundingBox()
        layers = {int(l) for l in t.GetLayerSet().Seq()} if _is_via(t) else {int(t.GetLayer())}
        probes.append((int(t.GetNetCode()), layers, bb))
    tds = [z for z in board.Zones() if z.IsTeardropArea()]
    dead_zones = []
    for z in tds:
        zb = z.GetBoundingBox()
        zl = {int(l) for l in z.GetLayerSet().Seq()}
        zn = int(z.GetNetCode())
        if any(zn == n and zl & ls and zb.Intersects(bb) for n, ls, bb in probes):
            dead_zones.append(z)
    for t in doomed:
        board.Delete(t)
    for z in dead_zones:
        board.Delete(z)
    return len(doomed), len(dead_zones)
