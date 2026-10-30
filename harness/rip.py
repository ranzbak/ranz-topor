"""Rip existing copper (optionally scoped) and record per-net track widths."""
from __future__ import annotations

import statistics
from dataclasses import dataclass, field
from typing import Literal

import pcbnew


@dataclass
class NetWidths:
    min_nm: int
    median_nm: int
    max_nm: int
    count: int


@dataclass
class RipReport:
    removed_tracks: int = 0
    removed_arcs: int = 0
    removed_vias: int = 0
    kept_locked_tracks: int = 0
    kept_free_vias: int = 0
    kept_locked_vias: int = 0
    kept_rf_tracks: int = 0
    kept_rf_vias: int = 0
    nets_to_route: list[str] = field(default_factory=list)
    kept_nets: list[str] = field(default_factory=list)


def _is_arc(t) -> bool:
    return t.GetClass() == "PCB_ARC"


def _is_via(t) -> bool:
    return t.GetClass() == "PCB_VIA"


def record_widths(board) -> dict[str, NetWidths]:
    """Per-net width statistics over PCB_TRACK + PCB_ARC (vias excluded)."""
    by_net: dict[str, list[int]] = {}
    for t in board.GetTracks():
        if _is_via(t):
            continue
        by_net.setdefault(t.GetNetname(), []).append(int(t.GetWidth()))
    return {
        n: NetWidths(min(w), int(statistics.median(w)), max(w), len(w))
        for n, w in by_net.items()
    }


def _scope_nets(board, scope: dict) -> set[str]:
    nets: set[str] = set(scope.get("nets", ()))
    classes = set(scope.get("netclasses", ()))
    if classes:
        ns = board.GetDesignSettings().m_NetSettings
        for name in board.GetNetsByName().keys():
            name = str(name)
            if set(ns.GetEffectiveNetClass(name).GetName().split(",")) & classes:
                nets.add(name)
    pads = set(scope.get("pads", ()))
    fps = set(scope.get("footprints", ()))
    if pads or fps:
        for fp in board.GetFootprints():
            ref = fp.GetReference()
            for pad in fp.Pads():
                if ref in fps or f"{ref}.{pad.GetNumber()}" in pads:
                    if pad.GetNetname():
                        nets.add(pad.GetNetname())
    return nets


def _unconnected_nets(board, pads_by_net: dict[str, list]) -> set[str]:
    """Nets whose pads are not all in one copper cluster (connectivity engine)."""
    conn = board.GetConnectivity()
    conn.Build(board)
    out = set()
    for net, pads in pads_by_net.items():
        reach = {p.m_Uuid.AsString() for p in conn.GetConnectedPads(pads[0])}
        reach.add(pads[0].m_Uuid.AsString())
        if any(p.m_Uuid.AsString() not in reach for p in pads):
            out.add(net)
    return out


def rip_tracks(
    board,
    *,
    mode: Literal["all", "none", "scoped"] = "all",
    scope: dict | None = None,
    rf_nets: set[str] = frozenset(),
    keep_locked=True,
) -> RipReport:
    rep = RipReport()
    in_scope: set[str] | None = None
    if mode == "scoped":
        in_scope = _scope_nets(board, scope or {})
    rf = set(rf_nets)

    def selectable(net: str) -> bool:
        return mode != "none" and (in_scope is None or net in in_scope)

    to_remove = []
    vias = []
    by_net: dict[str, list] = {}  # retained tracks/arcs, for the transition-via test
    for t in board.GetTracks():
        if _is_via(t):
            vias.append(t)
            continue
        net = t.GetNetname()
        if keep_locked and t.IsLocked():
            rep.kept_locked_tracks += 1
        elif net in rf:
            rep.kept_rf_tracks += 1
        elif selectable(net):
            to_remove.append(t)
            if _is_arc(t):
                rep.removed_arcs += 1
            else:
                rep.removed_tracks += 1
            continue
        by_net.setdefault(net, []).append(t)

    for v in vias:
        net = v.GetNetname()
        if v.GetIsFree():
            rep.kept_free_vias += 1
        elif keep_locked and v.IsLocked():
            rep.kept_locked_vias += 1
        elif net in rf:
            rep.kept_rf_vias += 1
        elif selectable(net):
            pos = v.GetPosition()
            if any(t.HitTest(pos) for t in by_net.get(net, ())):
                continue  # still touched by retained copper
            to_remove.append(v)
            rep.removed_vias += 1

    for it in to_remove:
        board.Remove(it)

    pads_by_net: dict[str, list] = {}
    for fp in board.GetFootprints():
        for pad in fp.Pads():
            n = pad.GetNetname()
            if n:
                pads_by_net.setdefault(n, []).append(pad)
    pads_by_net = {n: p for n, p in pads_by_net.items() if len(p) >= 2}
    unconnected = _unconnected_nets(board, pads_by_net)
    multi = sorted(pads_by_net)
    known = {str(n) for n in board.GetNetsByName().keys()}
    rep.kept_nets = sorted(rf & known)
    rep.nets_to_route = [
        n for n in multi
        if n not in rf and n in unconnected and (in_scope is None or n in in_scope)
    ]
    return rep
