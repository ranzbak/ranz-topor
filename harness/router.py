"""Router hook that calls the engine's topological router (`topor_engine.route`) and the
DRC-spots feedback of the bench loop."""
from __future__ import annotations

import json
import re

from harness.smoother import _dist_to_prim

_NET_RE = re.compile(r"\[([^\]]*)\]")


def route_router(model: dict, options: dict) -> dict:
    """Run the router. Returns {"items": [...], "remove_uuids": [], "report": <RouteResult dict>}."""
    import topor_engine

    out = json.loads(topor_engine.route(json.dumps(model), json.dumps(options)))
    return {"items": list(out.get("items") or []), "remove_uuids": [], "report": out}


def _layer_of(it: dict) -> int:
    if it.get("layer") is not None:
        return int(it["layer"])
    return int((it.get("layers") or [0])[0])


def drc_spots(violations, created: dict, model: dict, reach_nm: float,
              clearance_nm: float) -> list[dict]:
    """One DrcSpot dict per net of each new routing violation.

    A violation that names a written item (uuid in `created`) blames that item's net. Otherwise
    (zone-fill necks: KiCad names the via / pad and the zone, copper the engine cannot see) the
    blamed items are the written ones of another net whose centreline passes within `reach_nm`
    of a named position. The spot sits at the lowest non-zone item position (nm, KiCad frame);
    its radius is `reach_nm` for `connection_width`, else `clearance_nm` + the widest blamed item.
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
            spots.append({"layer": _layer_of(mine[0]), "at": at, "r": int(round(r)), "net": net})
    return spots
