"""Router hook that calls the engine's smoother (`topor_engine.smooth`) on hand copper."""
from __future__ import annotations

import json
import math
import re


def smooth_router(model: dict, options: dict) -> dict:
    """Run the smoother; forces existing_copper="smooth".

    Returns {"items": [...], "remove_uuids": [...], "report": <engine report dict>}.
    """
    import topor_engine

    opts = {**options, "existing_copper": "smooth"}
    out = json.loads(topor_engine.smooth(json.dumps(model), json.dumps(opts)))
    return {
        "items": list(out.get("items") or []),
        "remove_uuids": list(out.get("replaced") or []),
        "report": out,
    }


_NET_RE = re.compile(r"\[([^\]]*)\]")


def _dist_to_prim(p: tuple[float, float], prim: dict) -> float:
    """Centreline distance (nm) from `p` to a seg, or to an arc's start-mid-end polyline."""
    pts = [prim["a"], prim["b"]] if prim["kind"] == "seg" else [prim["start"], prim["mid"], prim["end"]]
    best = math.inf
    for a, b in zip(pts, pts[1:]):
        dx, dy = b[0] - a[0], b[1] - a[1]
        l2 = dx * dx + dy * dy
        t = 0.0 if l2 == 0 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2))
        best = min(best, math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy))
    return best


def drc_pins(violations, created: dict, wires: list[dict], net_names: dict,
             radius_nm: float) -> dict:
    """Hand items to pin (kept uuid -> reason) for new routing DRC violations.

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
