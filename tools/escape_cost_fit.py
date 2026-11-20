#!/usr/bin/env python3
"""Refit the TopoR loop's cost counter weights (engine/src/topo/escape.rs `pub mod cost`).

usage: python3 tools/escape_cost_fit.py [BOARD:]PROF.json [...] [--check] [--loo]

Input: profile JSONs of deterministic runs with layering escapes on (`TOPO_PROF=1`, engine example
`topo_route ... --prof FILE`, or the `profile` object of a plugin/harness result.json), each with the
`work` rows the loop writes (`prof::work`: kind escape|relay, wall_s, evals, local_wires, measures,
alive, passes, cost). Run them at the scorecard thread count (RAYON_NUM_THREADS=4) on a quiet machine.

Model (wall ms, non-negative least squares, each run weighted equally):
  escape = ESCAPE_FIXED + GLOBAL_ALIVE * alive + GLOBAL_EVAL * evals + GLOBAL_PASS * passes
           + MEASURE_PASS * passes * measures
  relay  = RELAY_FIXED + LOCAL_WIRE * local_wires + LOCAL_WIRE_PASS * local_wires * passes
           + LOCAL_EVAL * evals + RELAY_PASS * passes
Printed in the counter's unit (1 ms at 4 threads); then per run the escape-vs-relay real/predicted
ratio f (a share s becomes f s / (f s + 1 - s) in time).
--check: no fit; the ratios of the weights compiled in now (the rows' own `cost` column).
--loo: also leave one board out: fit on the other boards, f of the left-out board (BOARD: prefixes
name the boards; without one a file is its own board).
"""
from __future__ import annotations

import json
import sys

ESC = ("one", "alive", "evals", "passes", "pm")
REL = ("one", "local_wires", "lwp", "evals", "passes")


def rows(path: str) -> list[dict]:
    p = json.load(open(path))
    p = p.get("profile", p)
    out = []
    for r in p.get("work", []):
        r = dict(r)
        r["pm"] = r["passes"] * r["measures"]
        r["one"] = 1.0
        r["lwp"] = r["local_wires"] * r["passes"]
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


def fit(data: dict[str, list[dict]]) -> tuple[list[float], list[float]]:
    """The escape and re-lay weights (wall s per count) over the runs `data`."""
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
    return nnls(xe, ye), nnls(xr, yr)


def ratio(rs: list[dict], pe, pr) -> float:
    """Escape-vs-re-lay real/predicted f over the rows `rs`."""
    e = [r for r in rs if r["kind"] == "escape"]
    r_ = [r for r in rs if r["kind"] == "relay"]
    re_ = sum(r["wall_s"] for r in e) / sum(pe(r) for r in e)
    rr = sum(r["wall_s"] for r in r_) / sum(pr(r) for r in r_)
    return re_ / rr


def lin(c: list[float], keys: tuple) -> object:
    return lambda r: sum(w * r[k] for w, k in zip(c, keys))


def main(argv: list[str]) -> int:
    check = "--check" in argv
    loo = "--loo" in argv
    args = [a for a in argv if a not in ("--check", "--loo")]
    if not args:
        print(__doc__)
        return 2
    board = {}
    data = {}
    for a in args:
        b, f = a.split(":", 1) if ":" in a else (a, a)
        data[f] = rows(f)
        board[f] = b
    for f, rs in data.items():
        if not any(r["kind"] == "escape" for r in rs) or not any(r["kind"] == "relay" for r in rs):
            print(f"{f}: no escape or no relay rows (escape = \"layering\", TOPO_PROF=1?)", file=sys.stderr)
            return 1
    if check:
        pe = pr = lambda r: r["cost"]  # noqa: E731
    else:
        ce, cr = fit(data)
        for name, v in [("ESCAPE_FIXED", ce[0]), ("GLOBAL_ALIVE", ce[1]), ("GLOBAL_EVAL", ce[2]), ("GLOBAL_PASS", ce[3]),
                        ("MEASURE_PASS", ce[4]), ("RELAY_FIXED", cr[0]), ("LOCAL_WIRE", cr[1]), ("LOCAL_WIRE_PASS", cr[2]),
                        ("LOCAL_EVAL", cr[3]), ("RELAY_PASS", cr[4])]:
            print(f"pub const {name}: f64 = {1000 * v:.4g};")
        pe, pr = lin(ce, ESC), lin(cr, REL)
    for f, rs in data.items():
        e = sum(r["kind"] == "escape" for r in rs)
        print(f"{f}: {e} escapes, {len(rs) - e} re-lays, escape-vs-relay real/predicted f = {ratio(rs, pe, pr):.2f}")
    if loo and not check:
        for b in sorted(set(board.values())):
            ce, cr = fit({f: rs for f, rs in data.items() if board[f] != b})
            out = [r for f, rs in data.items() if board[f] == b for r in rs]
            print(f"leave out {b}: f = {ratio(out, lin(ce, ESC), lin(cr, REL)):.2f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
