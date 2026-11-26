#!/usr/bin/env python3
"""DRAW-ABSORB evidence: classify every new routed-copper KiCad DRC item of a score_runs run by the drawing
mechanism that produced it and by whether a fault-absorbing drawing could have avoided it.

    python3 tools/draw_absorb_classify.py <score_runs board dir .../<board>/s0> [--json OUT] [--md OUT]

Reads (all written by harness.score_runs / route_board): reports/**/result.json (engine violations + causes +
drawing counters), <board>--compass.json (class per drawn violation), <board>--drc-routed.json and
work/*/drc-orig.json (KiCad DRC routed / unrouted input). Read-only, no KiCad, no engine.

Join (same rule as toporlib.compass.build): a KiCad item takes the nearest drawn engine violation of a net it
names, on a layer it names, within compass_join_mm (0.5 mm) of its item positions minus half the violation's
length. Every drawn engine violation belongs to a wire written by never drop (allow_violations): the engine only
writes copper with a violation through the forced paths (core.rs viol_events ~10746; draw.rs force paths).

Mechanism (the copper's origin, from the joined violation's engine cause = the wire's last legal-attempt reason):
  forced_via     "via k of n: no site in its interval" -> the via was placed by force (draw.rs force_via ~2412)
  wrong_side     "passes a via of another net on the wrong side" -> a legal attempt went round a drawn via on the
                 side the topology does not allow, so the wire was forced (draw.rs ~3565)
  forced_run     "drawing: clearance gap ..." or the default "no legal geometry in the drawing" -> a forced run
                 (relaxed exact / funnel / attempt, draw.rs ~3868-4070)
  unjoined       no engine violation near: sub = what the item touches (ERBS fallback net / checker gap)

What the forced copper hit ("against" of the joined violation): movable = a track/via of another wire drawn by
the router (a neighbour that a re-taut drawing could move); static = pad, board item, dp bridge, keepout.

Absorbable verdict (a hypothesis per item, read with the compass class):
  yes      compass drawing_loss / overfull_nearby (the plan says the wire's own cuts fit) and the hit is movable
  via      compass via_site: room for a via disc; needs moving neighbours around the planned site (via-plan design)
  plan     planner_overfull / model_gap / neck_loss / pad_exit with a static hit: the plan has no room or no method
           step there; a drawing can only spread the shortfall
  partial  drawing_loss / overfull_nearby / neck_loss / pad_exit with a movable hit: room may exist next door
  checker  unjoined (KiCad only) or ERBS-fallback copper the engine believes legal
O(K x V) for K KiCad items, V drawn violations.
"""
import argparse
import collections
import glob
import json
import math
import re
import sys
from pathlib import Path

MM = 1e6
NET = re.compile(r"\[([^\]]+)\]")
LAYER = re.compile(r"\b([A-Za-z0-9_]+\.Cu)\b")
ROUTING = {"clearance", "copper_edge_clearance", "hole_clearance", "hole_to_hole", "track_width", "tracks_crossing",
           "shorting_items", "items_not_allowed", "annular_width", "via_diameter", "connection_width"}


def key(v):
    return (v["type"], tuple(sorted(i.get("uuid", "") for i in v.get("items", []))))


def kind_of(desc):
    d = desc.lower()
    for k, w in (("via", "via"), ("track", "track"), ("arc", "track"), ("pad", "pad"), ("zone", "zone")):
        if d.startswith(k) or f" {k} " in d[:20]:
            return w
    return "other"


def mechanism(cause):
    if "no site in its interval" in cause:
        return "forced_via"
    if "wrong side" in cause:
        return "wrong_side"
    return "forced_run"


def hit_kind(against):
    a = against or ""
    if a.startswith("track of") or a.startswith("via of") or a.startswith("arc of"):
        return "movable"
    return "static"


def verdict(cls, hit):
    if cls == "via_site":
        return "via"
    if cls in ("drawing_loss", "overfull_nearby"):
        return "yes" if hit == "movable" else "partial"
    if cls in ("neck_loss", "pad_exit"):
        return "partial" if hit == "movable" else "plan"
    return "plan"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("run")
    ap.add_argument("--json")
    ap.add_argument("--md")
    ap.add_argument("--join-mm", type=float, default=0.5)
    a = ap.parse_args(argv)
    run = Path(a.run)
    res = json.loads(Path(sorted(glob.glob(str(run / "reports" / "**" / "result.json"), recursive=True))[-1]).read_text())
    route = res["route"]
    comp = json.loads(next(run.glob("*--compass.json")).read_text())
    routed = json.loads(next(run.glob("*--drc-routed.json")).read_text())
    base = json.loads(Path(sorted(glob.glob(str(run / "work" / "*" / "drc-orig.json")))[0]).read_text())
    seen = {key(v) for v in base["violations"]}
    new = [v for v in routed["violations"] if v["type"] in ROUTING and key(v) not in seen]
    viols = route["violations"]
    drawn = comp["violations"]  # same order as route.violations, with the compass class
    no_band = {r.get("net") for r in route.get("erbs", {}).get("no_band_list", [])}
    rows = []
    for k, dv in enumerate(new):
        pts = [(i["pos"]["x"] * MM, i["pos"]["y"] * MM) for i in dv.get("items", []) if "pos" in i]
        descs = [i.get("description", "") for i in dv.get("items", [])]
        nets = {n for d in descs for n in NET.findall(d) if n != "<no net>"}
        layers = {l for d in descs for l in LAYER.findall(d)}
        best = None
        for r in drawn:
            if nets and r.get("name") not in nets:
                continue
            if layers and r.get("layer") not in layers and r.get("layer") != "all layers":
                continue
            at = (r["at_mm"][0] * MM, r["at_mm"][1] * MM)
            d = min((math.hypot(px - at[0], py - at[1]) for px, py in pts), default=math.inf) - r["half_len_mm"] * MM
            if d <= a.join_mm * MM and (best is None or d < best[0]):
                best = (d, r["i"])
        kinds = "-".join(sorted(kind_of(d) for d in descs))
        row = {"id": k, "type": dv["type"], "kinds": kinds, "nets": sorted(nets), "layers": sorted(layers),
               "at_mm": [round(pts[0][0] / MM, 3), round(pts[0][1] / MM, 3)] if pts else None}
        if best is None:
            sub = "erbs_no_band_net" if nets & no_band else "engine_silent"
            row.update(mechanism="unjoined", sub=sub, cls="kicad_only", hit="-", verdict="checker", conn=None)
        else:
            v, c = viols[best[1]], drawn[best[1]]
            cause = v.get("cause", "")
            hit = hit_kind(v.get("against"))
            row.update(mechanism=mechanism(cause), sub=re.sub(r"-?[0-9.]+", "#", cause)[:70], cls=c["class"],
                       hit=hit, verdict=verdict(c["class"], hit), conn=v.get("conn"), wire=v.get("name"),
                       against=v.get("against"))
        rows.append(row)
    C = collections.Counter
    summary = {
        "board": run.parent.name, "drc_new": len(new),
        "by_mechanism": dict(C(r["mechanism"] for r in rows).most_common()),
        "by_class": dict(C(r["cls"] for r in rows).most_common()),
        "by_verdict": dict(C(r["verdict"] for r in rows).most_common()),
        "by_mechanism_verdict": {f"{m}/{v}": n for (m, v), n in C((r["mechanism"], r["verdict"]) for r in rows).most_common()},
        "by_mechanism_hit": {f"{m}/{h}": n for (m, h), n in C((r["mechanism"], r["hit"]) for r in rows).most_common()},
        "by_class_hit": {f"{m}/{h}": n for (m, h), n in C((r["cls"], r["hit"]) for r in rows).most_common()},
        "connections_by_verdict": {v: len({r["conn"] for r in rows if r["verdict"] == v and r["conn"] is not None})
                                   for v in ("yes", "via", "partial", "plan")},
        "engine_violations": len(viols),
        "engine_by_class_hit": dict(C(f"{d['class']}/{hit_kind(v.get('against'))}" for v, d in zip(viols, drawn)).most_common()),
        "erbs": {k: v for k, v in route.get("erbs", {}).items() if not isinstance(v, list)},
        "unconnected_new": res.get("drc", {}).get("incomplete", {}).get("unconnected_new"),
    }
    out = {"summary": summary, "rows": rows}
    if a.json:
        Path(a.json).write_text(json.dumps(out, indent=1))
    if a.md:
        L = [f"# draw-absorb classification: {summary['board']}", "", "```", json.dumps(summary, indent=1), "```", "",
             "| id | type | kinds | mech | class | hit | verdict | wire | against | at |", "|---|---|---|---|---|---|---|---|---|---|"]
        for r in rows:
            L.append(f"| {r['id']} | {r['type']} | {r['kinds']} | {r['mechanism']} | {r['cls']} | {r['hit']} | {r['verdict']} | "
                     f"{r.get('wire', '-')} | {r.get('against', '-')} | {r['at_mm']} |")
        Path(a.md).write_text("\n".join(L) + "\n")
    json.dump(summary, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    sys.exit(main())
