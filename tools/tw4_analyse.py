#!/usr/bin/env python3
"""TW4 EVAL phase-1 analysis (branch tw4-eval, docs/dev/design/tw4-eval.md). Read-only on fetched run dirs.

    python3 tools/tw4_analyse.py <run root> [--json out.json] [--conns]

Finds every ``*--compass.json`` under the root (score_runs layout <cfg>/<board>/<board>/s<k>/ or route_board layout
<cfg>/<board>/), with its result.json (``reports/*/*/result.json``) and, when present, the engine log with
``TOPO_ERBS_DEBUG`` lines (score_runs: ``<root of the run>/<board>-s<k>.log``).

Per run:
- drawn violations by compass class; predicted (plan) and loop-predicted errors;
- unseen share, two definitions: ``unseen_plan`` = pad_exit + drawing_loss + model_gap (the plan's 49/114 and 187/196:
  the loop-diversity script counted pad_exit + drawing_loss with the cut check off, where model gap is inside drawing
  loss), ``unseen_all`` = drawn minus the drawn in the classes the loop's measure predicts (planner_overfull, via_site,
  unlayerable, foreign_copper, forced_path);
- the drawing-loss sub-classes (first match): model gap (the cut test finds the bottleneck over-full), a via of
  another net (``against`` names a via: CM1 via room), the wrong side of a via (cause "passes a via of another net on
  the wrong side": the drawn via is not where the plan crossed), a sequential run (the violating run was not drawn
  on its ERBS band: no band / rejected / replaced; from the ERBS debug lines, or the erbs no_band_list), a static item
  (pad that is no neighbour pin, board edge), else ERBS band vs foreign track (packing in the joint drawing);
- with an allow_violations=false run of the same board (cfg ending in F): the open connections = the forced ones,
  classified like route-improve-plan-20261118 §1 (pair half > via site reason > legal in the forced run (near miss)
  > capacity classes > pad exit >= drawing loss (pad exit) > rest (drawing order)) against the forced run's compass;
- KiCad's view (with ``*--drc-routed.json`` and ``*--drc-hand.json`` beside the compass): each new routing item of KiCad
  (type in toporlib.drc.ROUTING_TYPES, (type, item uuids) not in the hand board's DRC) joined to the nearest drawn
  violation like the compass does (nets and layer it names, the compass join radius), then split into the unseen
  classes neck_loss, pad_exit, drawing_crowding (a drawing loss against another net's track where the plan said
  fits: the wire is drawn through an area its plan did not see as crowded; sub-classes "ERBS band vs foreign track"
  and "sequential run"), via_room (via of another net, wrong side of a via), static, model_gap, seen (the measure's
  classes), kicad_only, other; their counts and shares of KiCad's new routing items; ``pour_island`` = new
  unconnected items (not in the hand board) with a zone among their items (a pour piece cut off); per connection
  with a KiCad item in a drawing-loss class (drawing_crowding, via_room, static, other: the plan said fits; sorted by
  that count, ``crowding_conns``): the KiCad and drawn counts, planned / drawn / reference length, detour, vias planned / drawn,
  the plan's own items on it, its KiCad items of every class and the foreign nets KiCad names (tw3 r4: esp s3 IO21_TX,
  planned 0 errors, KiCad 33).
"""
from __future__ import annotations

import collections
import glob
import json
import math
import os
import re
import sys

PRED = ("planner_overfull", "via_site", "unlayerable", "foreign_copper", "forced_path")
UNSEEN_PLAN = ("pad_exit", "drawing_loss", "model_gap", "neck_loss")  # neck_loss: located necks (TW4 meter), was inside drawing_loss
DL = ("drawing_loss", "model_gap", "neck_loss")
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from toporlib.drc import ROUTING_TYPES  # noqa: E402  (the routing types of KiCad's DRC the compass counts)
CROWD = ("ERBS band vs foreign track (joint packing)", "sequential run")
DLK = ("drawing_crowding", "via_room", "static", "other")  # drawing losses where the plan said fits (per-connection view)
KSPLIT = ("neck_loss", "pad_exit", "drawing_crowding", "via_room", "static", "model_gap", "seen", "kicad_only", "other")
_NET = re.compile(r"\[([^\]]+)\]")
_LAYER = re.compile(r"\b([A-Za-z0-9_]+\.Cu)\b")


def load_runs(root):
    out = {}
    for f in sorted(glob.glob(f"{root}/**/*--compass.json", recursive=True)):
        rel = os.path.relpath(f, root).split(os.sep)
        d = os.path.dirname(f)
        cfg = rel[0]
        board = os.path.basename(f)[: -len("--compass.json")]
        seed = next((p for p in rel if re.fullmatch(r"s\d+", p)), "s0")
        res = glob.glob(f"{d}/reports/*/*/result.json")
        log = None
        if re.fullmatch(r"s\d+", os.path.basename(d)):
            cand = os.path.join(os.path.dirname(os.path.dirname(d)), f"{board}-{os.path.basename(d)}.log")
            log = cand if os.path.exists(cand) else None
        met = glob.glob(f"{d}/*--metrics.json")
        drc = {k: (json.load(open(g[0])) if (g := glob.glob(f"{d}/*--drc-{k}.json")) else None) for k in ("routed", "hand")}
        out[(cfg, board, seed)] = {"compass": json.load(open(f)), "result": json.load(open(res[0])) if res else None,
                                   "metrics": json.load(open(met[0])) if met else {}, "log": log, "dir": d, "drc": drc}
    return out


def erbs_runs(log):
    """ERBS debug: per net id the drawn runs (layer, src, replaced, points in mm, KiCad frame)."""
    runs = collections.defaultdict(list)
    if not log:
        return runs
    cur = None
    for line in open(log, errors="ignore"):
        m = re.match(r"ERBS draw wire (\d+) net (\d+) L(\d+) k \S+: src (-?\d+) len \S+ band \S+ replaced (\w+)", line)
        if m:
            cur = {"wire": int(m[1]), "net": int(m[2]), "layer": int(m[3]), "src": int(m[4]), "replaced": m[5] == "true"}
            continue
        if cur and line.strip().startswith("drawn "):
            pts = [tuple(map(float, p)) for p in re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", line)]
            cur["pts"] = pts
            runs[cur["net"]].append(cur)
            cur = None
    return runs


def seg_dist(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 == 0 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2))
    return math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy)


def run_at(runs, net, at_mm):
    best = None
    for r in runs.get(net, []):
        pts = r.get("pts") or []
        for a, b in zip(pts, pts[1:]):
            d = seg_dist(at_mm, a, b)
            if best is None or d < best[0]:
                best = (d, r)
    return best


def against_kind(a):
    a = a or ""
    if a.startswith("pad"):
        return "pad"
    if a.startswith("via"):
        return "via"
    if a.startswith("track"):
        return "track"
    if a.startswith("edge"):
        return "edge"
    return a.split(":")[0].split(" ")[0] or "?"


def dl_sub(row, v, runs, nobands, forced=frozenset()):
    """The drawing-loss sub-class of one drawn violation (module doc)."""
    if row["class"] == "model_gap":
        return "model gap (cut test over-full, per-edge fits)"
    if (v.get("name"), v.get("from"), v.get("to")) in forced:
        return "forced connection (never-drop copper; open without forcing)"
    ag = against_kind(v.get("against"))
    cause = v.get("cause") or ""
    if "wrong side" in cause:
        return "wrong side of a via (drawn via not where the plan crossed)"
    if ag == "via":
        return "via of another net (via room)"
    key = (v.get("name"), v.get("from"), v.get("to"))
    near = run_at(runs, v.get("net"), (v["at"][0] / 1e6, v["at"][1] / 1e6)) if runs else None
    if key in nobands:
        return "sequential run (no ERBS band)"
    if runs:
        if near is None or near[0] > 0.5:
            return "sequential run (no ERBS band)"
        if near[1]["src"] >= 0:
            return "sequential run (ERBS rejected" + (", replaced)" if near[1]["replaced"] else ")")
    if ag in ("pad", "edge"):
        return f"static {ag} (no neighbour pin)"
    return "ERBS band vs foreign track (joint packing)"


def _vkey(v):
    return (v.get("type"), tuple(sorted(str(i.get("uuid", "")) for i in v.get("items") or [])))


def kicad_new(drc):
    """KiCad's new routing items and new unconnected items of a run (against the hand board's DRC)."""
    routed, hand = drc.get("routed"), drc.get("hand")
    if not routed:
        return None, None
    seen = {_vkey(v) for v in (hand or {}).get("violations") or []}
    new = [v for v in routed.get("violations") or [] if v.get("type") in ROUTING_TYPES and _vkey(v) not in seen]
    ukey = lambda u: tuple(sorted(str(i.get("uuid", "")) for i in u.get("items") or []))
    useen = {ukey(u) for u in (hand or {}).get("unconnected_items") or []}
    unew = [u for u in routed.get("unconnected_items") or [] if ukey(u) not in useen]
    return new, unew


def kicad_split(r, eruns, nob, fset):
    """KiCad's new routing items split into the unseen classes (module doc), the per-connection crowding rows and
    the pour islands. None without the DRC files."""
    new, unew = kicad_new(r.get("drc") or {})
    if new is None:
        return None
    c = r["compass"]
    route = (r["result"] or {}).get("route") or {}
    viols = route.get("violations") or []
    rad = ((c.get("join") or {}).get("radius_mm") or 0.5) * 1e6
    rows = c.get("violations") or []
    cnt, per = collections.Counter(), collections.defaultdict(lambda: {"kicad": 0, "foreign_nets": set()})
    allk = collections.defaultdict(collections.Counter)  # per connection: its KiCad items by class
    for dv in new:
        pts = [(it["pos"]["x"] * 1e6, it["pos"]["y"] * 1e6) for it in dv.get("items") or [] if "pos" in it]
        descs = [it.get("description") or "" for it in dv.get("items") or []]
        nets = {n for d in descs for n in _NET.findall(d) if n != "<no net>"}
        layers = {l for d in descs for l in _LAYER.findall(d)}
        best = None
        for row in rows:
            if nets and row.get("name") not in nets:
                continue
            if layers and row.get("layer") not in layers and row.get("layer") != "all layers":
                continue
            at = (row["at_mm"][0] * 1e6, row["at_mm"][1] * 1e6)
            dd = min((math.hypot(px - at[0], py - at[1]) for px, py in pts), default=math.inf) - row["half_len_mm"] * 1e6
            if dd <= rad and (best is None or dd < best[0]):
                best = (dd, row)
        if best is None:
            cnt["kicad_only"] += 1
            continue
        row = best[1]
        cls = row["class"]
        if cls in ("neck_loss", "pad_exit", "model_gap"):
            k = cls
        elif cls in PRED:
            k = "seen"
        elif cls == "drawing_loss" and row["i"] < len(viols):
            sub = dl_sub(row, viols[row["i"]], eruns, nob, fset)
            k = ("drawing_crowding" if sub.startswith(CROWD) else "via_room" if "via" in sub
                 else "static" if sub.startswith("static") else "other")
        else:
            k = "other"
        cnt[k] += 1
        allk[(row.get("conn"), row.get("name"), row.get("from"), row.get("to"))][k] += 1
        if k in DLK:
            p = per[(row.get("conn"), row.get("name"), row.get("from"), row.get("to"))]
            p["kicad"] += 1
            p["foreign_nets"].update(nets - {row.get("name")})
    conn = {x.get("conn"): x for x in c.get("connections") or []}
    drawn = collections.Counter((x.get("conn"), x["class"]) for x in rows)
    planned = collections.Counter(cc for it in ((route.get("compass") or {}).get("items") or []) for cc in it.get("conns") or [])
    crowd = []
    for (cid, name, fr, to), p in sorted(per.items(), key=lambda kv: -kv[1]["kicad"]):
        x = conn.get(cid, {})
        crowd.append({"conn": [name, fr, to], "kicad_drawing_loss": p["kicad"], "kicad_all": dict(allk[(cid, name, fr, to)]), "drawn_drawing_loss": drawn[(cid, "drawing_loss")],
                      "plan_items": planned.get(cid, 0), "planned_mm": x.get("planned_mm"), "length_mm": x.get("length_mm"),
                      "ref_mm": x.get("ref_mm"), "detour": x.get("detour"), "detour_vs_plan": x.get("detour_vs_plan"),
                      "vias_planned": x.get("vias_planned"), "vias_drawn": x.get("vias_drawn"),
                      "foreign_nets": sorted(p["foreign_nets"])})
    isl = [u for u in unew if any((it.get("description") or "").startswith("Zone") for it in u.get("items") or [])]
    zones = collections.Counter(next(d for d in (it.get("description") or "" for it in u["items"]) if d.startswith("Zone"))
                                .split(",")[0] for u in isl)
    tot = len(new)
    dl = sum(cnt[k] for k in DLK)
    return {"kicad_new": tot, "by_class": {k: cnt[k] for k in KSPLIT if cnt[k]},
            "drawing_loss_all": dl, "drawing_loss_all_share": round(dl / tot, 3) if tot else None,
            "drawing_loss_conns": len(crowd), "drawing_loss_conns_plan_clean": sum(1 for x in crowd if not x["plan_items"]),
            "share": {k: round(cnt[k] / tot, 3) for k in KSPLIT if cnt[k]} if tot else {},
            "unconnected_new": len(unew), "pour_island": len(isl), "pour_island_zones": dict(zones), "crowding_conns": crowd}


def forced_set(runs, key):
    """(name, from, to) of the connections open in the <cfg>F run of `key` (a <cfg>K run): the forced ones."""
    cfg, board, seed = key
    f = runs.get((cfg[:-1] + "F", board, seed)) if cfg.endswith("K") else None
    if not f or not f["result"]:
        return frozenset()
    lab = lambda e: e.get("label") if isinstance(e, dict) else e
    return frozenset((x["name"], lab(x["from"]), lab(x["to"])) for x in f["result"]["route"]["connections"] if x.get("status") != "routed")


def loop_cycles(result):
    m = re.search(r"(\d+) cycles in ([\d.]+) s", json.dumps(result or {}))
    return (int(m[1]), float(m[2])) if m else (None, None)


def summarise(runs):
    rows, dlsub, dlsub_conns = {}, {}, {}
    for key, r in runs.items():
        cfg, board, seed = key
        c = r["compass"]
        by = {x["class"]: (x.get("drawn") or 0) for x in c.get("rows", [])}
        drawn = c.get("drawn_violations") or 0
        seen = sum(by.get(k, 0) for k in PRED)
        un_plan = sum(by.get(k, 0) for k in UNSEEN_PLAN)
        m = r["metrics"]
        route = (r["result"] or {}).get("route") or {}
        erbs = route.get("erbs") or {}
        rows[key] = {"drawn": drawn, "seen": seen, "unseen_plan": un_plan, "unseen_all": drawn - seen,
                     "share_plan": round(un_plan / drawn, 3) if drawn else None,
                     "share_all": round((drawn - seen) / drawn, 3) if drawn else None,
                     "predicted": c.get("predicted_violations"), "loop_predicted": c.get("loop_predicted"),
                     "by_class": {k: v for k, v in by.items() if v}, "kicad_new": m.get("routing_new"),
                     "unconnected": m.get("unconnected"), "routed": m.get("routed"), "connections": m.get("connections"),
                     "vias": m.get("vias"), "cpu_s": m.get("router_cpu_s"), "stop": m.get("stop_reason"),
                     "cycles": loop_cycles(r["result"])[0], "loop_s": loop_cycles(r["result"])[1], "forced_draw": erbs.get("forced"),
                     "conns_viol": sum(1 for x in route.get("connections", []) if x.get("violations"))}
        if not route:
            continue
        viols = route.get("violations") or []
        eruns = erbs_runs(r["log"])
        nob = {(x.get("net"), x.get("from"), x.get("to")) for x in erbs.get("no_band_list", [])}
        fset = forced_set(runs, key)
        cnt, conns = collections.Counter(), collections.defaultdict(set)
        for row in c.get("violations", []):
            if row["class"] not in DL:
                continue
            v = viols[row["i"]]
            s = dl_sub(row, v, eruns, nob, fset)
            cnt[s] += 1
            conns[s].add((v.get("name"), v.get("from"), v.get("to")))
        dlsub[key] = dict(cnt.most_common())
        dlsub_conns[key] = {k: sorted(map(list, v)) for k, v in conns.items()}
        rows[key]["erbs_debug"] = bool(eruns)
        ks = kicad_split(r, eruns, nob, fset)
        if ks is not None:
            rows[key]["kicad_split"] = ks
    return rows, dlsub, dlsub_conns


def forced_classes(runs):
    """Open connections of each <cfg>F run (allow_violations=false), classified against the <cfg>K run."""
    out = {}
    for (cfg, board, seed), r in runs.items():
        if not cfg.endswith("F") or not r["result"]:
            continue
        base = runs.get((cfg[:-1] + "K", board, seed))
        if not base or not base["result"]:
            continue
        fr, br = r["result"]["route"], base["result"]["route"]
        pairs = set()
        for dp in br.get("diff_pairs") or []:
            if isinstance(dp, dict):
                pairs.update(dp[k] for k in ("p_name", "n_name") if isinstance(dp.get(k), str))
        lab = lambda e: e.get("label") if isinstance(e, dict) else e
        ck = lambda x: (x["name"], lab(x["from"]), lab(x["to"]))
        bc = {ck(x): x for x in br["connections"]}
        bviol = collections.defaultdict(collections.Counter)
        for row in base["compass"].get("violations", []):
            bviol[(row["name"], row["from"], row["to"])][row["class"]] += 1
        cls = collections.Counter()
        per = []
        for x in fr["connections"]:
            if x.get("status") == "routed":
                continue
            k = ck(x)
            reason = x.get("reason") or ""
            b = bc.get(k, {})
            vc = bviol.get(k, collections.Counter())
            if x["name"] in pairs or "diff pair" in reason:
                c = "pair"
            elif "no site" in reason:
                c = "via site"
            elif not b.get("violations"):
                c = "near-miss legal"
            elif vc["planner_overfull"] + vc["model_gap"] + vc["overfull_nearby"]:
                c = "capacity"
            elif vc["pad_exit"] and vc["pad_exit"] >= vc["drawing_loss"]:
                c = "pad exit"
            else:
                c = "drawing order"
            cls[c] += 1
            per.append({"conn": list(k), "class": c, "reason": reason[:140], "base_viol": b.get("violations"),
                        "base_classes": dict(vc), "base_vias": b.get("vias")})
        out[(cfg, board, seed)] = {"open": sum(cls.values()), "by_class": dict(cls), "conns": per}
    return out


def main():
    root = sys.argv[1]
    runs = load_runs(root)
    rows, dlsub, dlsub_conns = summarise(runs)
    fc = forced_classes(runs)
    for k in sorted(rows):
        r = rows[k]
        print(f"{k[0]:8} {k[1]:20} {k[2]:3} drawn {r['drawn']:4} unseen plan {r['unseen_plan']:4} ({r['share_plan']}) all "
              f"{r['unseen_all']:4} ({r['share_all']}) pred {r['predicted']} loop {r['loop_predicted']} kicad {r['kicad_new']} "
              f"unc {r['unconnected']} vias {r['vias']} cyc {r['cycles']} stop {r['stop']} cpu {r['cpu_s']}")
        if dlsub.get(k):
            print("         drawing-loss sub-classes:", dlsub[k])
        if r.get("kicad_split"):
            ks = r["kicad_split"]
            print(f"         KiCad new {ks['kicad_new']}: {ks['by_class']} share {ks['share']}; pour_island "
                  f"{ks['pour_island']} of {ks['unconnected_new']} new unconnected {ks['pour_island_zones']}")
            for p in ks["crowding_conns"][:5] if "--conns" in sys.argv else []:
                print("           crowding", p)
    for k in sorted(fc):
        print("FORCED", k, fc[k]["open"], fc[k]["by_class"])
        if "--conns" in sys.argv:
            for p in fc[k]["conns"]:
                print("   ", p)
    if "--json" in sys.argv:
        j = {"runs": {"|".join(k): v for k, v in rows.items()}, "drawing_loss_sub": {"|".join(k): v for k, v in dlsub.items()},
             "drawing_loss_sub_conns": {"|".join(k): v for k, v in dlsub_conns.items()},
             "forced": {"|".join(k): v for k, v in fc.items()}}
        json.dump(j, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)


if __name__ == "__main__":
    main()
