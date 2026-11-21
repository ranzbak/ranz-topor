"""first-draw-45 r6 (TEMP): U6 pinrow_long_bundle PNG, self-contained (model JSON + track dumps, no harness).

    python tools/fd45_u6_png.py <model.json> <out.png> <title1> <tracks1.json> [<title2> <tracks2.json> ...]

One panel per track dump (FD45_DUMP of tests/topo_first_draw_45.rs: [net, layer, x0, y0, x1, y1] mm, KiCad frame),
free-angle segments outlined yellow, plus a last panel with the pattern's reference tracks (the formula of
harness drc_patterns.pinrow_long_bundle, copied). Pads from the model JSON.
"""
import json
import math
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Circle, Rectangle  # noqa: E402

W = 0.20
COL = {1: "#c0392b", 2: "#2471a3", 3: "#229954", 4: "#7d3c98", 5: "#d35400", 6: "#17a589", 7: "#b7950b"}


def free(a, b):
    dx, dy = abs(b[0] - a[0]) * 1000, abs(b[1] - a[1]) * 1000
    return dx > 1 and dy > 1 and abs(dx - dy) > 0.01 * max(dx, dy) + 1


def r_(x):
    return round(x, 4)


def reference():
    jx, jy = 5.0, 15.0
    rows = [(0, "S1"), (2, "S2"), (4, "S3"), (6, "S4"), (8, "S5")]
    tgt = {n: (r_(jx + 1.385 * i), jy) for i, n in rows}
    for i, n in [(1, "S6"), (7, "S7")]:
        tgt[n] = (r_(jx + 1.385 * i), r_(jy + 2.84))
    ux0, uy = r_(jx + 25.093), r_(jy - 9.706)
    order = ["S1", "S6", "S2", "S3", "S4", "S7", "S5"]
    ux = [r_(ux0 + 0.65 * i + (0.01 if i else 0.0)) for i in range(7)]
    yd = r_(uy + 1.01 + 0.2)
    out = []
    for i, n in enumerate(order):
        yh = r_(jy - 6.0 + 0.5 * i)
        d = ux[i] + yd
        tx, ty = tgt[n]
        if ty > jy:
            c = tx + jy - 0.6
            tail = [(r_(c - yh), yh), (tx, r_(jy - 0.6)), (tx, ty)]
        else:
            c = tx + ty
            tail = [(r_(c - yh), yh), (tx, ty)]
        out.append((int(n[1]), [(ux[i], uy), (ux[i], yd), (r_(d - yh), yh)] + tail))
    return out


def base(ax, model, title):
    ax.set_title(title, fontsize=9)
    ax.set_xlim(0, 36)
    ax.set_ylim(20, 0)
    ax.set_aspect("equal")
    ax.add_patch(Rectangle((0, 0), 36, 20, fill=False, lw=1.2))
    for p in model["pads"]:
        cx, cy = p["center"][0] / 1e6, p["center"][1] / 1e6
        sh = next(iter(p["shapes"].values()))
        w, h = sh.get("w", sh.get("d", 0)) / 1e6, sh.get("h", 0) / 1e6
        fc = "#c9b26b" if p.get("drill") else "#e8b4b4"
        if sh["kind"] == "circle":
            ax.add_patch(Circle((cx, cy), w / 2, fc=fc, ec="#555", lw=0.5))
        else:
            ax.add_patch(Rectangle((cx - w / 2, cy - h / 2), w, h, fc=fc, ec="#555", lw=0.5))
        if p.get("drill"):
            ax.add_patch(Circle((cx, cy), p["drill"]["w"] / 2e6, fc="white", ec="#555", lw=0.5))


def draw(ax, polys, mark=True):
    n_fa, mm = 0, 0.0
    for net, pts in polys:
        xs, ys = zip(*pts)
        ax.plot(xs, ys, color=COL.get(net, "#c0392b"), lw=W * 18, solid_capstyle="round", alpha=0.85)
        for p, q in zip(pts, pts[1:]):
            if mark and free(p, q):
                n_fa += 1
                mm += math.dist(p, q)
                ax.plot([p[0], q[0]], [p[1], q[1]], color="#f0c000", lw=W * 30, alpha=0.5, zorder=0)
    return n_fa, mm


def main(argv):
    model = json.load(open(argv[0]))
    out = argv[1]
    pairs = list(zip(argv[2::2], argv[3::2]))
    fig, axs = plt.subplots(len(pairs) + 1, 1, figsize=(11, 6.4 * (len(pairs) + 1)), dpi=110)
    for ax, (title, path) in zip(axs, pairs):
        raw = json.load(open(path))
        polys = [(n, [(x0, y0), (x1, y1)]) for n, _l, x0, y0, x1, y1 in raw]
        base(ax, model, "")
        n, mm = draw(ax, polys)
        ax.set_title(f"{title}: {n} free-angle segments, {mm:.1f} mm (yellow)", fontsize=9)
    base(axs[-1], model, "pattern reference (panel 3 of r2-u6-p4.png; bend depth not asserted)")
    draw(axs[-1], reference(), mark=False)
    fig.tight_layout()
    fig.savefig(out)
    print("wrote", out)


if __name__ == "__main__":
    main(sys.argv[1:])
