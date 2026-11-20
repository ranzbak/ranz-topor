#!/usr/bin/env python3
"""Refit the TopoR loop's cost counter weights (engine/src/topo/escape.rs `pub mod cost`).

usage: python3 tools/escape_cost_fit.py PROF.json [PROF.json ...] [--check]

Input: profile JSONs of deterministic runs with layering escapes on (`TOPO_PROF=1`, engine example
`topo_route ... --prof FILE`, or the `profile` object of a plugin/harness result.json), each with the
`work` rows the loop writes (`prof::work`: kind escape|relay, wall_s, evals, local_wires, measures,
alive, passes, cost). Run them at the scorecard thread count (RAYON_NUM_THREADS=4) on a quiet machine.

Model (wall ms, non-negative least squares, each run weighted equally):
  escape = a * alive + e * evals + p * passes + m * passes * measures
  relay  = l * local_wires + le * evals + r * passes
Printed in the counter's unit, one local wire (`l` ms): GLOBAL_ALIVE = a / l, GLOBAL_EVAL = e / l,
GLOBAL_PASS = p / l, MEASURE_PASS = m / l, LOCAL_EVAL = le / l, RELAY_PASS = r / l; then per run the
escape-vs-relay real/predicted ratio f (a share s becomes f s / (f s + 1 - s) in time).
--check: no fit; the ratios of the weights compiled in now (the rows' own `cost` column).
"""
from __future__ import annotations

import json
import sys

ESC = ("alive", "evals", "passes", "pm")
REL = ("local_wires", "evals", "passes")


def rows(path: str) -> list[dict]:
    p = json.load(open(path))
    p = p.get("profile", p)
    out = []
    for r in p.get("work", []):
        r = dict(r)
        r["pm"] = r["passes"] * r["measures"]
        out.append(r)
    return out


def nnls(xs: list[list[float]], ys: list[float]) -> list[float]:
    """Least squares with every coefficient >= 0 (active set: drop the most negative, refit)."""
    n = len(xs[0])
    free = list(range(n))
    while True:
        k = len(free)
        a = [[sum(x[free[i]] * x[free[j]] for x in xs) for j in range(k)] for i in range(k)]
        b = [sum(x[free[i]] * y for x, y in zip(xs, ys)) for i in range(k)]
        for i in range(k):  # Gauss-Jordan with partial pivoting
            piv = max(range(i, k), key=lambda r: abs(a[r][i]))
            a[i], a[piv], b[i], b[piv] = a[piv], a[i], b[piv], b[i]
            if abs(a[i][i]) < 1e-300:
                continue
            for r in range(k):
                if r != i:
                    f = a[r][i] / a[i][i]
                    a[r] = [u - f * v for u, v in zip(a[r], a[i])]
                    b[r] -= f * b[i]
        sol = [b[i] / a[i][i] if abs(a[i][i]) > 1e-300 else 0.0 for i in range(k)]
        neg = [i for i in range(k) if sol[i] < 0]
        if not neg:
            c = [0.0] * n
            for i, j in enumerate(free):
                c[j] = sol[i]
            return c
        free.pop(min(neg, key=lambda i: sol[i]))


def main(argv: list[str]) -> int:
    check = "--check" in argv
    files = [a for a in argv if a != "--check"]
    if not files:
        print(__doc__)
        return 2
    data = {f: rows(f) for f in files}
    for f, rs in data.items():
        if not any(r["kind"] == "escape" for r in rs) or not any(r["kind"] == "relay" for r in rs):
            print(f"{f}: no escape or no relay rows (escape = \"layering\", TOPO_PROF=1?)", file=sys.stderr)
            return 1
    if check:
        pe = lambda r: r["cost"]  # noqa: E731
        pr = pe
    else:
        xe, ye, xr, yr = [], [], [], []
        for rs in data.values():
            e = [r for r in rs if r["kind"] == "escape"]
            r_ = [r for r in rs if r["kind"] == "relay"]
            we = 1.0 / (len(e) ** 0.5 * sum(r["wall_s"] for r in e) / len(e))
            wr = 1.0 / (len(r_) ** 0.5 * sum(r["wall_s"] for r in r_) / len(r_))
            xe += [[r[k] * we for k in ESC] for r in e]
            ye += [r["wall_s"] * we for r in e]
            xr += [[r[k] * wr for k in REL] for r in r_]
            yr += [r["wall_s"] * wr for r in r_]
        ce, cr = nnls(xe, ye), nnls(xr, yr)
        unit = cr[0] if cr[0] > 0 else 1e-3
        print(f"unit = one local wire = {1000 * unit:.3f} ms")
        for name, v in [("GLOBAL_ALIVE", ce[0]), ("GLOBAL_EVAL", ce[1]), ("GLOBAL_PASS", ce[2]), ("MEASURE_PASS", ce[3]),
                        ("LOCAL_EVAL", cr[1]), ("RELAY_PASS", cr[2])]:
            print(f"pub const {name}: f64 = {v / unit:.4g};")
        pe = lambda r: sum(c * r[k] for c, k in zip(ce, ESC))  # noqa: E731
        pr = lambda r: sum(c * r[k] for c, k in zip(cr, REL))  # noqa: E731
    for f, rs in data.items():
        e = [r for r in rs if r["kind"] == "escape"]
        r_ = [r for r in rs if r["kind"] == "relay"]
        re_, rr = sum(r["wall_s"] for r in e) / sum(pe(r) for r in e), sum(r["wall_s"] for r in r_) / sum(pr(r) for r in r_)
        print(f"{f}: {len(e)} escapes, {len(r_)} re-lays, escape-vs-relay real/predicted f = {re_ / rr:.2f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
