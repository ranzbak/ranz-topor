"""Routing scope from the KiCad selection and the existing-copper / lock policies (46 row 22, 14, 8b).

Path used: the engine already resolves ``Options.scope`` (pads / nets / area, `unrouted_only`, lone-terminal and
MST rules in decompose.rs), so the scope is handed to it instead of filtering the model in Python. What the
engine does not know is the footprint-level ``between`` semantics (a lone selected pad would be routed to its
nearest partner anywhere), so footprints are expanded here to the pads whose net has at least two selected pads
(``between``) or to the endpoints of every spanning-tree connection that touches a selected pad (``touching``).
``do_not_touch`` nets are removed from the pad lists; on the whole board they are removed by listing every other
net in ``scope.nets`` (the engine has no ``exclude_nets`` yet; build_options also passes it when it exists).

Connection counts are the spanning-tree estimate used by ``run.connection_estimate`` (pads - 1 per net): they do
not know that existing copper may already join two pads.

Complexity: all functions are O(P + V + K + N + sum over touched nets of k^2) for P pads, V vias, K kept tracks,
N nets and k pads of a net that has both selected and unselected pads (capped at ``_MST_CAP`` pads, larger nets
use one connection per selected pad). Never an all-pairs loop over the board.
"""

from __future__ import annotations

import fnmatch
import math
from dataclasses import dataclass, field

from .config import Config
from .findings import Finding
from .ports import Selection

_MST_CAP = 200


@dataclass
class ScopePlan:
    """``engine_scope`` is the engine ``Scope`` dict (pads as ``REF.NUM``). ``nets`` = names of the nets that
    have a connection in scope (what ``rip_plan`` may rip); ``counts`` = {between, touching, whole}."""
    mode: str  # "whole" | "between" | "touching"
    engine_scope: dict
    nets: frozenset = frozenset()
    connections: int = 0
    counts: dict = field(default_factory=dict)
    findings: list = field(default_factory=list)
    footprints: tuple = ()


@dataclass
class RipPlan:
    """Items to delete on Accept and the ``existing_copper`` value the engine gets."""
    remove_uuids: list = field(default_factory=list)
    engine_existing_copper: str = "fixed"
    kept_locked: int = 0
    kept_do_not_touch: int = 0


def _empty_scope(unrouted_only: bool) -> dict:
    return {"nets": [], "netclasses": [], "pads": [], "vias": [], "footprints": [], "area": None,
            "unrouted_only": bool(unrouted_only)}


def do_not_touch_nets(model: dict, cfg: Config) -> set:
    """Net names matching a ``do_not_touch`` entry (exact or glob). O(N * patterns)."""
    pats = list(cfg.get("nets", "do_not_touch"))
    if not pats:
        return set()
    return {n["name"] for n in model.get("nets", [])
            if any(n["name"] == p or fnmatch.fnmatchcase(n["name"], p) for p in pats)}


def _in_poly(x: float, y: float, poly) -> bool:
    """Even-odd point in polygon (`poly` = [(x, y)] or [{x, y}]). O(len(poly))."""
    pts = [(p["x"], p["y"]) if isinstance(p, dict) else (p[0], p[1]) for p in poly]
    inside = False
    j = len(pts) - 1
    for i, (xi, yi) in enumerate(pts):
        xj, yj = pts[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def _xy(p) -> tuple:
    c = p["center"]
    return (c["x"], c["y"]) if isinstance(c, dict) else (c[0], c[1])


def _prim_edges(idx: list, pads: list) -> list:
    """Euclidean minimum spanning tree edges over pad indices `idx`. O(k^2)."""
    pts = [_xy(pads[i]) for i in idx]
    k = len(idx)
    best = [math.inf] * k
    parent = [-1] * k
    done = [False] * k
    best[0] = 0.0
    edges = []
    for _ in range(k):
        u = min((i for i in range(k) if not done[i]), key=lambda i: best[i])
        done[u] = True
        if parent[u] >= 0:
            edges.append((idx[parent[u]], idx[u]))
        for v in range(k):
            if not done[v]:
                d = math.hypot(pts[u][0] - pts[v][0], pts[u][1] - pts[v][1])
                if d < best[v]:
                    best[v], parent[v] = d, u
    return edges


def _compute(sel: Selection, model: dict, cfg: Config):
    """(plan pieces) shared by scope_from_selection and connections_by_scope. See module docstring."""
    pads = model.get("pads", [])
    names = {n["code"]: n["name"] for n in model.get("nets", [])}
    excluded = do_not_touch_nets(model, cfg)
    ignore = set(cfg.get("nets", "ignore_pads"))
    by_net: dict = {}
    for i, p in enumerate(pads):
        net = p.get("net")
        if not net or names.get(net) in excluded or f"{p['ref']}.{p['number']}" in ignore:
            continue
        by_net.setdefault(net, []).append(i)

    sel_pads: set = set()
    sel_fps = set(sel.footprints)
    sel_refs = {(r.ref, r.number) for r in sel.pads}
    for i, p in enumerate(pads):
        if p["ref"] in sel_fps or (p["ref"], p["number"]) in sel_refs:
            sel_pads.add(i)
    if sel.area:
        for i, p in enumerate(pads):
            if _in_poly(*_xy(p), sel.area):
                sel_pads.add(i)
    sel_nets: set = set()  # nets selected as a whole by a selected track / via
    if sel.tracks or sel.vias:
        tr, vi = set(sel.tracks), set(sel.vias)
        for k in model.get("kept", []):
            if k["uuid"] in tr and k.get("net"):
                sel_nets.add(k["net"])
        for v in model.get("vias", []):
            if v["uuid"] in vi and v.get("net"):
                sel_nets.add(v["net"])
        sel_nets -= {c for c in sel_nets if names.get(c) in excluded}

    counts = {"whole": sum(len(v) - 1 for v in by_net.values() if len(v) >= 2), "between": 0, "touching": 0}
    between_pads, touching_pads = [], []
    for net, idx in by_net.items():
        s = [i for i in idx if i in sel_pads]
        if len(s) >= 2:
            counts["between"] += len(s) - 1
            between_pads += s
        if not s:
            continue
        if len(idx) == len(s) or len(idx) > _MST_CAP:
            edges = [(a, b) for a, b in zip(s, s[1:])] if len(idx) == len(s) else [(a, a) for a in s]
        else:
            edges = [e for e in _prim_edges(idx, pads) if e[0] in sel_pads or e[1] in sel_pads]
        counts["touching"] += len(edges)
        touching_pads += sorted({i for e in edges for i in e})
    for c in sel_nets:
        n = max(len(by_net.get(c, ())) - 1, 0)
        counts["between"] += n
        counts["touching"] += n
    return pads, names, excluded, by_net, sel_pads, sel_nets, counts, between_pads, touching_pads


def connections_by_scope(sel: Selection, model: dict, cfg: Config) -> dict:
    """{"between", "touching", "whole"} -> connection count (spanning-tree estimate) for the pre-flight header.
    Selection without footprints / pads / area / tracks / vias reports between = touching = whole. O(see module)."""
    out = _compute(sel, model, cfg)[6]
    if sel.empty:
        out = {**out, "between": out["whole"], "touching": out["whole"]}
    return out


def scope_from_selection(sel: Selection, model: dict, cfg: Config, whole: bool = False) -> ScopePlan:
    """Plan for `sel` and ``cfg run.scope_mode`` (between | touching). Empty selection or ``whole`` (the dialog's
    "whole board" answer) -> whole board. Findings: SCOPE_EMPTY (warning) when a selection yields no connection.
    O(see module docstring)."""
    unrouted = bool(cfg.get("run", "unrouted_only"))
    (pads, names, excluded, by_net, sel_pads, sel_nets, counts, between_pads,
     touching_pads) = _compute(sel, model, cfg)
    if sel.empty or whole:
        scope = _empty_scope(unrouted)
        if excluded:  # engine has no exclude_nets: select every other net instead
            scope["nets"] = sorted(n["name"] for n in model.get("nets", []) if n["name"] not in excluded)
        nets = frozenset(n for n in names.values() if n not in excluded)
        return ScopePlan("whole", scope, nets, counts["whole"], counts)
    mode = cfg.get("run", "scope_mode")
    chosen = between_pads if mode == "between" else touching_pads
    scope = _empty_scope(unrouted)
    scope["pads"] = sorted({f"{pads[i]['ref']}.{pads[i]['number']}" for i in chosen})
    scope["nets"] = sorted(names[c] for c in sel_nets if c in names)
    if mode == "touching" and sel.area:
        scope["area"] = [{"x": p["x"], "y": p["y"]} if isinstance(p, dict) else {"x": p[0], "y": p[1]}
                         for p in sel.area]
    nets = {names[pads[i]["net"]] for i in chosen} | set(scope["nets"])
    n_conn = counts[mode]
    findings = []
    if n_conn == 0:
        findings.append(Finding(
            "SCOPE_EMPTY", "warning",
            f"The selection has no connection to route in mode '{mode}': " +
            ("no net has two selected pads." if mode == "between" else "no selected pad has a net with other pads."),
            "Select more footprints in the KiCad PCB editor, or set topor.toml [run] scope_mode = \"touching\", "
            "or choose the whole board in the pre-flight dialog."))
    return ScopePlan(mode, scope, frozenset(nets), n_conn, counts, findings, tuple(sel.footprints))


def rip_plan(model: dict, plan: ScopePlan, cfg: Config) -> RipPlan:
    """Copper to delete for ``run.existing_copper``: ``fixed`` nothing; ``rip`` the tracks and vias of the nets in
    `plan.nets`, except locked ones when ``respect_locked``, ``do_not_touch`` nets and free vias (a via that
    connects to nothing but copper); ``smooth`` nothing is removed and the engine gets ``smooth``.
    ``unrouted_only`` never removes anything. O(K + V)."""
    policy = cfg.get("run", "existing_copper")
    out = RipPlan(engine_existing_copper="smooth" if policy == "smooth" else "fixed")
    if policy != "rip" or cfg.get("run", "unrouted_only"):
        return out
    names = {n["code"]: n["name"] for n in model.get("nets", [])}
    excluded = do_not_touch_nets(model, cfg)
    keep_locked = cfg.get("run", "respect_locked")
    for kind, free_ok in (("kept", False), ("vias", True)):
        for it in model.get(kind, []):
            name = names.get(it.get("net"))
            if name is None:
                continue
            if name in excluded:
                out.kept_do_not_touch += 1
            elif name not in plan.nets:
                continue
            elif keep_locked and it.get("locked"):
                out.kept_locked += 1
            elif free_ok and it.get("free"):
                continue
            else:
                out.remove_uuids.append(it["uuid"])
    return out
