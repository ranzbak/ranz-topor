#!/usr/bin/env python3
"""Simulate ordered speculative drawing from a TOPO_PDTRACE jsonl.

conflict(i, j) (j earlier in i's window): i's read cells meet j's write cells, or i read
wire j's state, or j failed (its via uncommit is not traced: treated as a write everywhere).
Policy P1 (batch of L units from one snapshot, commit in order, a unit invalid against the
writes committed since the snapshot is redone on the master at its turn):
  batch time = max(t of the batch) + sum(t of invalid units) + clone_ms per batch.
"""
import json, sys
from collections import defaultdict

BIG = (-(2**63), -(2**63))

def load(path):
    by = defaultdict(list)
    for line in open(path):
        j = json.loads(line)
        u = dict(w=j["w"], t=j["t"], ok=j["ok"], r=set(map(tuple, j["r"])), wr=set(map(tuple, j["wr"])), ww=set(j["ww"]), nq=j["nq"])
        u["fail"] = (not j["ok"]) or (2**64 - 1 in u["ww"]) or any(x >= 2**63 for x in u["ww"])
        by[j["tag"]].append(u)
    return by

def conflict(i, j):
    if j["fail"]:
        return True
    if BIG in i["r"] or BIG in j["wr"]:
        return True
    if j["w"] in i["ww"]:
        return True
    return not i["r"].isdisjoint(j["wr"])

def p1(units, L, clone_s=0.0):
    t = 0.0
    inval = 0
    for b in range(0, len(units), L):
        batch = units[b:b + L]
        t += max(u["t"] for u in batch) + (clone_s if L > 1 else 0.0)
        committed = []
        for u in batch:
            if any(conflict(u, c) for c in committed):
                t += u["t"]
                inval += 1
            committed.append(u)
    return t, inval

def main():
    path = sys.argv[1]
    clone_s = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0
    by = load(path)
    for tag, us in by.items():
        tot = sum(u["t"] for u in us)
        top = sorted((u["t"] for u in us), reverse=True)
        print(f"{tag}: {len(us)} units, {tot:.2f} s serial, max {top[0]:.3f} s, top5 share {sum(top[:5]) / max(tot, 1e-9):.1%}, fails {sum(u['fail'] for u in us)}, mean read cells {sum(len(u['r']) for u in us) / len(us):.0f}, write cells {sum(len(u['wr']) for u in us) / len(us):.0f}")
        # adjacent conflict rate
        adj = sum(conflict(us[i], us[i - 1]) for i in range(1, len(us)))
        print(f"   conflict with the unit just before: {adj}/{len(us) - 1}")
        for L in (2, 4, 8, 16):
            t, inv = p1(us, L, clone_s)
            print(f"   L={L:2d}: {t:.2f} s ({tot / max(t, 1e-9):.2f}x), invalid {inv} ({inv / len(us):.0%}); ideal {max(tot / L, top[0]):.2f} s")

main()
