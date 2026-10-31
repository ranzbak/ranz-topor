"""The DRC re-route loop (plan T1.4) with the board writes and the DRC behind a callable. Pure; no pcbnew.

`route_with_drc_loop` is the loop that used to live inside `harness.bench.run_bench`: route, let
`drc_fn` apply the routed items to a board copy and judge it (kicad-cli DRC), derive DRC spots /
pins from new routing violations, and route again, capped by `loop_cap` and the whole-run budget
(M3-R8: later rounds get what is left). Zero-slack retry judge (R6) included.

Complexity: at most `loop_cap + 1` rounds (each: one route_fn + one drc_fn call, both external);
per round O(V * (C + W)) for V new violations over C created items and W wires (drc_spots /
drc_pins). The loop adds no search of its own.
"""
from __future__ import annotations

import math
import re
import time
from dataclasses import dataclass, field
from typing import Callable

from toporlib.drc import DrcReport, classify, new_routing

ENGINE_WALL_CLOCK_S = 60.0  # engine/src/model.rs Options::default().wall_clock_s
MIN_RERUN_BUDGET_S = 1.0  # a DRC-loop / zero-slack re-route needs at least this much budget left

_NET_RE = re.compile(r"\[([^\]]*)\]")


@dataclass
class DrcJudgement:
    """What `drc_fn` returns: the DRC of the board with the routed items applied, and the written
    items keyed by the uuid of each created board item (uuid -> engine item dict)."""
    report: DrcReport
    created: dict = field(default_factory=dict)


@dataclass
class LoopResult:
    """Outcome of the loop: the LAST round's router report / items / created / DRC, plus the loop
    state (rounds run, wires pinned, DRC spots, zero-slack verdict, budget flag, summed router time)."""
    router_report: dict | None
    items: list
    created: dict
    routed_drc: DrcReport
    rounds: int
    pinned: dict
    spots: list
    zero_slack: dict
    budget_exhausted: bool
    router_s: float


def _zero_slack_names(report: dict | None) -> list[str]:
    return [str(w.get("name")) for w in ((report or {}).get("zero_slack_wires") or [])]


def _dist_to_prim(p: tuple[float, float], prim: dict) -> float:
    """Centreline distance (nm) from `p` to a seg, or to an arc's start-mid-end polyline. O(1)."""
    pts = [prim["a"], prim["b"]] if prim["kind"] == "seg" else [prim["start"], prim["mid"], prim["end"]]
    best = math.inf
    for a, b in zip(pts, pts[1:]):
        dx, dy = b[0] - a[0], b[1] - a[1]
        l2 = dx * dx + dy * dy
        t = 0.0 if l2 == 0 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2))
        best = min(best, math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy))
    return best


def _layer_of(it: dict) -> int:
    if it.get("layer") is not None:
        return int(it["layer"])
    return int((it.get("layers") or [0])[0])


def drc_spots(violations, created: dict, model: dict, reach_nm: float,
              clearance_nm: float) -> list[dict]:
    """One DrcSpot dict per net of each new routing violation. O(V * (C + L)).

    A violation that names a written item (uuid in `created`) blames that item's net. Otherwise
    (zone-fill necks: KiCad names the via / pad and the zone, copper the engine cannot see) the
    blamed items are the written ones of another net whose centreline passes within `reach_nm`
    of a named position. The spot sits at the lowest non-zone item position (nm, KiCad frame);
    a via item yields one spot per copper layer; its radius is `reach_nm` for `connection_width`, else `clearance_nm` + the widest blamed item.
    """
    names = {n["code"]: n["name"] for n in model.get("nets", [])}
    spots: list[dict] = []
    for v in violations:
        if not v.items:
            continue
        hits = [created[i["uuid"]] for i in v.items if i.get("uuid") in created]
        if not hits:
            nets = {m for i in v.items for m in _NET_RE.findall(i.get("description", ""))}
            for i in v.items:
                p = (i["pos"]["x"] * 1e6, i["pos"]["y"] * 1e6)
                hits += [it for it in created.values()
                         if it.get("prim") is not None and names.get(it["net"]) not in nets
                         and _dist_to_prim(p, it["prim"]) <= reach_nm]
        if not hits:
            continue
        # KiCad's item order varies between runs: name the lowest non-zone position.
        pos = [i for i in v.items if not i.get("description", "").startswith("Zone")] or v.items
        x, y = min((i["pos"]["x"], i["pos"]["y"]) for i in pos)
        at = [round(x * 1e6), round(y * 1e6)]
        for net in sorted({h["net"] for h in hits}):
            mine = [h for h in hits if h["net"] == net]
            r = (reach_nm if v.type == "connection_width"
                 else clearance_nm + max(h.get("width") or h.get("d") or 0 for h in mine))
            if any(h.get("kind") == "via" for h in mine):  # through via: copper on every layer
                layers = sorted(int(l["id"]) for l in model.get("layers", [])
                                ) or [_layer_of(mine[0])]
            else:
                layers = [_layer_of(mine[0])]
            spots.extend({"layer": l, "at": at, "r": int(round(r)), "net": net}
                         for l in layers)
    return spots


def drc_pins(violations, created: dict, wires: list[dict], net_names: dict,
             radius_nm: float) -> dict:
    """Hand items to pin (kept uuid -> reason) for new routing DRC violations. O(V * (C + W)).

    `created`: uuid of each written item -> its engine item (with `wire`). A violation that
    names a written item pins that item's wire. Otherwise (zone-fill necks: KiCad names the via /
    pad and the zone, copper the engine cannot see) every written item of another net whose
    centreline passes within `radius_nm` of a named item's position pins its wire.
    """
    by_id = {w["id"]: w for w in wires if "id" in w}
    pins: dict = {}
    for v in violations:
        direct = {created[i["uuid"]].get("wire") for i in v.items if i.get("uuid") in created}
        hits = {w for w in direct if w is not None}
        if not direct:
            nets = {m for i in v.items for m in _NET_RE.findall(i.get("description", ""))}
            for i in v.items:
                p = (i["pos"]["x"] * 1e6, i["pos"]["y"] * 1e6)
                hits |= {it["wire"] for it in created.values()
                         if it.get("wire") is not None and it.get("prim") is not None
                         and net_names.get(it["net"]) not in nets
                         and _dist_to_prim(p, it["prim"]) <= radius_nm}
        if not v.items:
            continue
        # KiCad's item order varies between runs: name the lowest non-zone position.
        spots = [i for i in v.items if not i.get("description", "").startswith("Zone")] or v.items
        x, y = min((i["pos"]["x"], i["pos"]["y"]) for i in spots)
        reason = f"drc {v.type} at ({x:.3f}, {y:.3f}) mm"
        for w in sorted(hits):
            for u in by_id.get(w, {}).get("kept_uuids", []):
                pins.setdefault(u, reason)
    return pins


def route_with_drc_loop(
    model: dict,
    options: dict,
    *,
    route_fn: Callable[[dict, dict], "list[dict] | dict"],
    drc_fn: Callable[..., DrcJudgement],
    base_drc: DrcReport,
    orig_drc: DrcReport | None,
    budget_s: float,
    loop_cap: int,
    reach_nm: float = 1_000_000.0,
    clearance_nm: float = 0.0,
    min_rerun_budget_s: float = MIN_RERUN_BUDGET_S,
) -> LoopResult:
    """Route, judge by DRC, and re-route with DRC feedback.

    `route_fn(model, options)` returns a dict {"items", "remove_uuids", "report"} or a bare item
    list (report None: no feedback loop). `options` is passed through (callers put `threads` in);
    from round 2 on `wall_clock_s` is the remaining `budget_s`, and `pinned` / `drc_spots` /
    `zero_slack_retry` carry the feedback.

    `drc_fn(items)` applies the routed items to a board copy and returns a `DrcJudgement`. When
    the router also names hand copper to replace, it is called `drc_fn(items, remove_uuids=[...])`
    (keyword only when non-empty). Every call must start from the UNROUTED board state: the loop
    never asks for a restore, so a re-route after feedback simply starts from scratch in `drc_fn`.
    `base_drc` is the DRC of the ripped, unrouted board; `orig_drc` the original board's (kept for
    callers; the loop itself only needs `base_drc`). `reach_nm` / `clearance_nm` size the DRC
    spots (zone-fill reach, board clearance).
    """
    pinned: dict = dict(options.get("pinned") or {})
    spots: list = list(options.get("drc_spots") or [])
    zero_slack: dict = {"status": "not attempted", "wires": []}
    zs_off = options.get("zero_slack_retry") is False
    rounds, router_s = 0, 0.0
    budget_exhausted = False

    def out_of_budget() -> bool:
        return budget_s - router_s < min_rerun_budget_s

    while True:
        rounds += 1
        t_route = time.monotonic()
        opts = {**options,
                **({"wall_clock_s": max(0.0, budget_s - router_s)} if rounds > 1 else {}),
                **({"pinned": pinned} if pinned else {}),
                **({"drc_spots": spots} if spots else {}),
                **({"zero_slack_retry": False} if zs_off else {})}
        routed = route_fn(model, opts)
        router_s += time.monotonic() - t_route
        if isinstance(routed, dict):
            items = list(routed.get("items") or [])
            remove_uuids = list(routed.get("remove_uuids") or [])
            router_report = routed.get("report")
        else:
            items, remove_uuids, router_report = list(routed), [], None
        judged = drc_fn(items, remove_uuids=remove_uuids) if remove_uuids else drc_fn(items)
        routed_drc, created = judged.report, judged.created
        if router_report is None or rounds > loop_cap:
            break
        if "connections" in router_report:  # topor: DRC spots for the offending nets
            zs_names = _zero_slack_names(router_report)
            if zs_names and not zs_off:
                wires = [{"id": w.get("conn"), "net": w.get("name")}
                         for w in router_report.get("zero_slack_wires") or []]
                bad = (classify(routed_drc, base_drc, created)["routing_new"] > 0
                       or len(routed_drc.unconnected) > 0)
                if bad:
                    zero_slack = {"status": "rejected by DRC", "wires": wires}
                    if out_of_budget():  # no time for the re-run: keep this board, flagged
                        zero_slack["budget_exhausted"] = True
                        budget_exhausted = True
                        break
                    zs_off = True
                    continue
                zero_slack = {"status": "kept", "wires": wires}
            elif zero_slack["status"] == "not attempted" and not zs_off:
                zero_slack = {"status": str(router_report.get("zero_slack") or "not run"),
                              "wires": []}
            have = {(sp["layer"], tuple(sp["at"])) for sp in spots}
            fresh = []
            for sp in drc_spots(new_routing(routed_drc, base_drc, created), created, model,
                                reach_nm, float(clearance_nm)):
                if (sp["layer"], tuple(sp["at"])) not in have:
                    have.add((sp["layer"], tuple(sp["at"])))
                    fresh.append(sp)
            if not fresh:
                break
            if out_of_budget():
                budget_exhausted = True
                break
            spots.extend(fresh)
            continue
        names = {n["code"]: n["name"] for n in model["nets"]}
        more = {u: r for u, r in drc_pins(new_routing(routed_drc, base_drc, created), created,
                                          router_report.get("wires") or [], names, reach_nm).items()
                if u not in pinned}
        if not more:
            break
        if out_of_budget():
            budget_exhausted = True
            break
        pinned.update(more)
    return LoopResult(router_report=router_report, items=items, created=created,
                      routed_drc=routed_drc, rounds=rounds, pinned=pinned, spots=spots,
                      zero_slack=zero_slack, budget_exhausted=budget_exhausted, router_s=router_s)
