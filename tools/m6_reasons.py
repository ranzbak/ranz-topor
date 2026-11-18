#!/usr/bin/env python3
"""m6_reasons.py BASE TIP [--json]: per board, the connections whose status changed between two runs of the
M6 measurement (tools/m6_job.sh layout), with the tip's failure reason class and the base's violation count.

Classes (from route.connections[].reason of the failed side): via (no legal via site), clearance (no legal drawing:
a clearance / gap), graze (own pad / via, KiCad minimum connection width), junction (branch meets its trunk on another
layer / undrawn trunk), crossing (a crossing that cannot be layered), other. Also the never-drop counters of each run
(result warnings: wires / vias drawn by force, branches joined by force, P6 a / c counts, funnel fallback runs).
Matching base<->tip by net name + the two terminal labels (as ladder_diff). O(connections).
"""
import glob, json, os, re, sys
from collections import Counter, defaultdict


def runs(root):
    out = {}
    for mf in glob.glob(os.path.join(root, "**", "*--metrics.json"), recursive=True):
        d = os.path.dirname(mf)
        rs = sorted(glob.glob(os.path.join(d, "reports", "*", "*", "result.json")))
        if rs:
            out[os.path.basename(mf).split("--")[0]] = rs[-1]
    return out


def klass(reason):
    r = (reason or "").lower()
    if not r:
        return "-"
    if "via" in r and ("site" in r or "place" in r):
        return "via"
    if "grazes" in r:
        return "graze"
    if "branch" in r or "trunk" in r or "junction" in r:
        return "junction"
    if "cannot be layered" in r or "crossing of" in r:
        return "crossing"
    if "clearance" in r or "gap" in r or "drawing:" in r:
        return "clearance"
    return "other:" + r[:60]


def conns(path):
    r = json.load(open(path))
    route = r.get("route") or {}
    out = {}
    for c in route.get("connections") or []:
        key = (c.get("name"), *sorted(map(str, ((c.get("from") or {}).get("label"), (c.get("to") or {}).get("label")))))
        out[key] = c
    return out, route.get("warnings") or []


PATS = {
    "forced_wires": r"never drop \(\[optimize\] allow_violations\): \d+ connections written with \d+ rule violations .*?; (\d+) wires drawn by force",
    "forced_via_wires": r"(\d+) wires with a via placed by force",
    "joined": r"(\d+) joined to their trunk by force",
    "p6a": r"(\d+) crossing partners re-read from a via as placed",
    "p6c": r"(\d+) corners removed by the taut pull",
    "funnel_ok": r"by the S2a funnel\+repair fallback (\d+)",
    "exact_ok": r"runs accepted by the exact path (\d+)",
    "fb": r"feedback tried (\d+), rescued (\d+)",
}


def counters(ws):
    d = {}
    for w in ws:
        for k, p in PATS.items():
            m = re.search(p, w)
            if m and k not in d:
                d[k] = "/".join(m.groups())
    return d


def main():
    a = [x for x in sys.argv[1:] if not x.startswith("--")]
    base, tip = runs(a[0]), runs(a[1])
    js = {}
    for b in sorted(set(base) & set(tip)):
        cb, wb = conns(base[b])
        ct, wt = conns(tip[b])
        lost, gained = [], []
        for k in sorted(set(cb) | set(ct), key=str):
            x, y = cb.get(k), ct.get(k)
            if not x or not y:
                continue
            rx, ry = x.get("status") == "routed", y.get("status") == "routed"
            if rx and not ry:
                lost.append((k, klass(y.get("reason")), x.get("violations"), y.get("reason")))
            elif ry and not rx:
                gained.append((k, klass(x.get("reason")), y.get("violations"), x.get("reason")))
        cl = Counter(l[1] for l in lost)
        viol_lost = sum(l[2] or 0 for l in lost)
        js[b] = {"lost": len(lost), "gained": len(gained), "lost_by_class": dict(cl), "base_viol_of_lost": viol_lost,
                 "lost_clean_in_base": sum(1 for l in lost if not l[2]),
                 "base_counters": counters(wb), "tip_counters": counters(wt),
                 "lost_list": [[*l[0], l[1], l[2]] for l in lost], "gained_list": [[*g[0], g[1], g[2]] for g in gained]}
    if "--json" in sys.argv:
        print(json.dumps(js, indent=1))
        return
    for b, v in js.items():
        if not (v["lost"] or v["gained"]) and v["base_counters"] == v["tip_counters"]:
            continue
        print(f"{b}: lost {v['lost']} (classes {v['lost_by_class']}; {v['lost_clean_in_base']} of them clean in the base, "
              f"base violations on the lost {v['base_viol_of_lost']}), gained {v['gained']}")
        print(f"   counters base {v['base_counters']}\n   counters tip  {v['tip_counters']}")
        for l in v["lost_list"]:
            print("   - lost", l)
        for g in v["gained_list"]:
            print("   + gained", g)


if __name__ == "__main__":
    main()
