#!/usr/bin/env python3
"""pad-centre-main: attribute every KiCad Cleanup "shorting" removal of a routed board to the engine's violation
and its compass sub-class (planner crossing / via without site / pad exit over neighbour pin / drawing loss).

  python3 tools/pad_centre_main_attrib.py <cleanup.json> <result.json> [--board NAME] > out.json

cleanup.json = harness.cleanup_check --json (or score_runs' <board>--cleanup.json); result.json = the run's report
folder result.json. Each shorting item is matched to the engine violation (result.json compass.violations) of kind
short / clearance / hole_clearance whose net pair is the item's (net, other net) in either order, nearest to the
item's geometry; `dist_mm` says how near. O(items x violations)."""
import json, math, sys

SUB = {"planner_overfull": "planner crossing", "overfull_nearby": "planner crossing", "via_site": "via without site",
       "pad_exit": "pad exit over neighbour pin", "drawing_loss": "drawing loss"}


def seg_dist(p, a, b):
    ax, ay = a; bx, by = b; px, py = p
    dx, dy = bx - ax, by - ay
    u = 0.0 if dx == dy == 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(ax + u * dx - px, ay + u * dy - py)


def main(argv):
    cl, res = json.load(open(argv[0])), json.load(open(argv[1]))
    board = argv[argv.index("--board") + 1] if "--board" in argv else ""
    cv = res["compass"]["violations"]
    rv = res["route"]["violations"]
    out = []
    for it in cl["items"]:
        if it["rule"] != "shorting":
            continue
        other = it["detail"].split(" of net ")[-1]
        at = it["at"]
        geo = (lambda p: seg_dist(p, at[:2], at[2:])) if len(at) == 4 else (lambda p: math.hypot(p[0] - at[0], p[1] - at[1]))
        best = None
        for i, v in enumerate(cv):
            r = rv[i] if i < len(rv) else {}
            against = r.get("against", "")
            if v["name"] not in (it["net"], other):
                continue
            if v["name"] == it["net"] and other not in against and "pad:" not in against:
                continue
            if v["name"] == other and it["net"] not in against:
                continue
            d = geo(v["at_mm"])
            if best is None or d < best[0]:
                best = (d, v, r)
        e = {"board": board, "net": it["net"], "other_net": other, "kind": it["kind"], "layer": it["layer"], "at_mm": at,
             "touches": it["detail"]}
        if best:
            d, v, r = best
            e.update(dist_mm=round(d, 3), forced_conn=f"{v['name']} {v['from']} -> {v['to']}", engine_kind=r.get("kind"),
                     engine_at_mm=v["at_mm"], compass_class=v.get("class"), compass_sub=v.get("sub"), mech=v.get("mech"),
                     fclass=v.get("fclass"), subclass=SUB.get(v.get("class"), v.get("class")), cause=r.get("cause"),
                     victim=it["net"] != v["name"])
        out.append(e)
    json.dump(out, sys.stdout, indent=1)


if __name__ == "__main__":
    main(sys.argv[1:])
