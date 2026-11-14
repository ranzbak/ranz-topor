"""ERBS acceptance (M3 merge gate, user 2026-11-14): length over band, reversals and bends of every ERBS, from the
engine's ERBS dumps (``TOPO_ERBS_DUMP=<dir>``: ``erbs-<layer>.json`` from the cut report, ``realise-L<n>.json`` from
the drawing's ``[drawing] realise = "erbs"``).

Per stretch:

- ``ratio`` = ERBS length / band length (the band is the zero-width taut path in the sheet: a lower bound).
- ``reversals``: the path sampled every 10 um and projected on the band (arc length of the nearest band point,
  searched forward from the last one); a backward leg is a run of falling projection, counted when it falls by more
  than 1 um; ``back_um`` the largest. A taut path has none.
- ``bends``: interior corners turning more than 0.01 degree. A bend is *explained* when it lies on a gate end (a
  spoke tip, an arc sample, a feature octagon vertex; within 0.5 um) and turns toward that end's root (the root on
  the inner side of the turn). ``unexplained`` counts the others (a corner in the middle of a portal: the zigzag).
- ``conv``: the solver's own convergence flag.

Pass marks (user 2026-11-14): (a) eprom-to-kernal stretch 2 (wire 2, F.Cu): ratio <= 1.03, 0 reversals, 0
unexplained bends, conv; (b) eprom + wiccon: p50 ratio <= 1.02, max <= 1.15, 0 reversals; esp aggregate (sum of
lengths / sum of bands) <= 1.10.

    python -m harness.erbs_accept DUMPDIR [--stretch N] [--mark a|b-small|b-esp] [--json out.json]

Exit 1 when a requested mark fails. O(path samples x band segments) per stretch.
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

TOL_TIP = 5e-4  # mm
MIN_TURN = math.radians(0.01)


def plen(p):
    return sum(math.dist(a, b) for a, b in zip(p, p[1:]))


def _proj(band, cum, x, j0):
    """Arc length of the nearest point of `band` to `x`, searching from segment j0 on (and a little back)."""
    best = (1e18, 0.0, j0)
    for j in range(max(0, j0 - 2), len(band) - 1):
        a, b = band[j], band[j + 1]
        dx, dy = b[0] - a[0], b[1] - a[1]
        l2 = dx * dx + dy * dy
        t = 0.0 if l2 == 0 else max(0.0, min(1.0, ((x[0] - a[0]) * dx + (x[1] - a[1]) * dy) / l2))
        q = (a[0] + dx * t, a[1] + dy * t)
        d = math.dist(q, x)
        if d < best[0] - 1e-12:
            best = (d, cum[j] + math.sqrt(l2) * t, j)
    return best[1], best[2]


def reversals(path, band, step=0.01, eps=1e-3):
    """(count of backward legs falling more than eps mm, largest fall in um)."""
    if len(band) < 2 or len(path) < 2:
        return 0, 0.0
    cum = [0.0]
    for a, b in zip(band, band[1:]):
        cum.append(cum[-1] + math.dist(a, b))
    pts = []
    for a, b in zip(path, path[1:]):
        n = max(1, int(math.ceil(math.dist(a, b) / step)))
        pts += [(a[0] + (b[0] - a[0]) * k / n, a[1] + (b[1] - a[1]) * k / n) for k in range(n)]
    pts.append(path[-1])
    j = 0
    s_prev, _ = _proj(band, cum, pts[0], 0)
    peak = s_prev
    legs, worst, falling = 0, 0.0, False
    for x in pts[1:]:
        s, j = _proj(band, cum, x, j)
        if s < peak - eps:
            if not falling:
                legs += 1
                falling = True
            worst = max(worst, peak - s)
        if s >= peak:
            peak = s
            falling = False
        s_prev = s
    return legs, worst * 1e3


def bends(path, gates):
    """(bends, unexplained, list of unexplained corners)."""
    ends = []
    for g in gates:
        ends.append((g["a"], g["p"]))
        if g.get("src") == "cut":
            ends.append((g["b"], g["q"]))
    n = un = 0
    bad = []
    for k in range(1, len(path) - 1):
        u, x, v = path[k - 1], path[k], path[k + 1]
        din = (x[0] - u[0], x[1] - u[1])
        dout = (v[0] - x[0], v[1] - x[1])
        li, lo = math.hypot(*din), math.hypot(*dout)
        if li == 0 or lo == 0:
            continue
        turn = math.atan2(din[0] * dout[1] - din[1] * dout[0], din[0] * dout[0] + din[1] * dout[1])
        if abs(turn) < MIN_TURN:
            continue
        n += 1
        inner = (dout[0] / lo - din[0] / li, dout[1] / lo - din[1] / li)
        ok = False
        for tip, root in ends:
            if math.dist(tip, x) <= TOL_TIP and ((root[0] - x[0]) * inner[0] + (root[1] - x[1]) * inner[1]) > 0:
                ok = True
                break
        if not ok:
            un += 1
            bad.append([round(x[0], 4), round(x[1], 4), round(math.degrees(turn), 2)])
    return n, un, bad


def load(dumpdir: Path):
    out = []
    for f in sorted(Path(dumpdir).glob("*.json")):
        if not (f.name.startswith("erbs-") or f.name.startswith("realise-")):
            continue
        d = json.loads(f.read_text())
        for e in d.get("erbs", []):
            e["_layer"] = f.stem.split("-", 1)[1]
            out.append(e)
    return out


def stretch_row(e) -> dict:
    band, path = e["band"], e["path"]
    bl, L = plen(band), plen(path)
    rv, back = reversals(path, band)
    nb, un, bad = bends(path, e.get("gates", []))
    return dict(layer=e["_layer"], stretch=e["stretch"], wire=e["wire"], net=e.get("net"), len=round(L, 4), band=round(bl, 4),
                ratio=round(L / bl, 4) if bl > 0 else 1.0, reversals=rv, back_um=round(back, 2), bends=nb, unexplained=un,
                conv=e.get("converged", True), bad_bends=bad[:10])


def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return None
    k = (len(xs) - 1) * q
    i = int(math.floor(k))
    return xs[i] if i + 1 >= len(xs) else xs[i] + (xs[i + 1] - xs[i]) * (k - i)


def summary(rows) -> dict:
    rs = [r["ratio"] for r in rows if r["band"] > 0.05]
    return dict(stretches=len(rows), p50=round(pct(rs, 0.5) or 1.0, 4), p90=round(pct(rs, 0.9) or 1.0, 4), max=round(max(rs, default=1.0), 4),
                aggregate=round(sum(r["len"] for r in rows) / max(1e-9, sum(r["band"] for r in rows)), 4),
                reversals=sum(r["reversals"] for r in rows), with_reversals=sum(1 for r in rows if r["reversals"]),
                unexplained=sum(r["unexplained"] for r in rows), not_conv=sum(1 for r in rows if not r["conv"]),
                worst=sorted(rows, key=lambda r: -r["ratio"])[:5])


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("dumpdir")
    ap.add_argument("--stretch", type=int, action="append", default=[])
    ap.add_argument("--layer", default=None)
    ap.add_argument("--mark", choices=["a", "b-small", "b-esp"], action="append", default=[])
    ap.add_argument("--json")
    a = ap.parse_args(argv)
    rows = [stretch_row(e) for e in load(Path(a.dumpdir)) if not a.layer or e["_layer"] == a.layer]
    rep = dict(summary=summary(rows), stretches=[r for r in rows if not a.stretch or r["stretch"] in a.stretch])
    fails = []
    for m in a.mark:
        if m == "a":
            st = [r for r in rows if r["stretch"] == 2 and r["wire"] == 2 and r["layer"].startswith(("F.Cu", "L0"))]
            if not st:
                fails.append("a: stretch 2 / wire 2 on F.Cu not in the dump")
            for r in st:
                if r["ratio"] > 1.03:
                    fails.append(f"a: ratio {r['ratio']} > 1.03 (len {r['len']} band {r['band']})")
                if r["reversals"]:
                    fails.append(f"a: {r['reversals']} reversals (worst {r['back_um']} um)")
                if r["unexplained"]:
                    fails.append(f"a: {r['unexplained']} bends not at a tip turning toward it")
                if not r["conv"]:
                    fails.append("a: not converged")
        elif m == "b-small":
            s = rep["summary"]
            if s["p50"] > 1.02:
                fails.append(f"b: p50 {s['p50']} > 1.02")
            if s["max"] > 1.15:
                fails.append(f"b: max {s['max']} > 1.15")
            if s["reversals"]:
                fails.append(f"b: {s['reversals']} reversals in {s['with_reversals']} stretches")
        elif m == "b-esp":
            if rep["summary"]["aggregate"] > 1.10:
                fails.append(f"b: aggregate {rep['summary']['aggregate']} > 1.10")
    rep["fails"] = fails
    txt = json.dumps(rep, indent=1)
    if a.json:
        Path(a.json).write_text(txt)
    print(txt)
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
