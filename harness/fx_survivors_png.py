"""PNG of a proposed survivor pattern (harness/fx_survivors.py): expected route | topor sequential | topor erbs, same view, same scale.
Pads (nets coloured, no-net grey), rule areas (hatched), board edge, the clearance envelope of foreign pads (dotted, half width + clearance),
tracks per layer (F.Cu solid, B.Cu hatched), vias. Run on the VM next to the routing:

    python -m harness.fx_survivors_png BOARDS RUNS OUT_DIR [name ...]
BOARDS = the fx_survivors output dir, RUNS = dir with <name>-seq/ and <name>-erbs/ route_board outputs."""
from __future__ import annotations

import glob
import json
import math
import os
import sys
import textwrap
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Circle, Polygon, Rectangle  # noqa: E402
from shapely.geometry import LineString, box  # noqa: E402

from harness.fit_patterns import CLEAR, W_SIG  # noqa: E402
from harness.fx_survivors import PATTERNS  # noqa: E402

COL = ["#1f77b4", "#ff7f0e", "#2ca02c", "#9467bd"]
# view windows (x0, x1, y0, y1): the part of the board the pattern is about
VIEW = {"edge_tangent": (3.5, 8.5, 4.2, 6.6), "collinear_row": (2.5, 14.5, 3.6, 7.0), "diag_exit": (1.5, 10.5, 1.5, 10.5),
        "via_fold": (0.0, 8.0, 3.0, 9.0), "qfn_two_ends": (3.2, 9.8, 0.8, 10.2), "edge_pads": (-0.3, 3.0, 0.8, 10.2)}


def read_copper(pcb: Path):
    import pcbnew

    b = pcbnew.LoadBoard(str(pcb))
    segs, vias = [], []
    for t in b.GetTracks():
        cls = t.GetClass()
        if cls == "PCB_VIA":
            p = t.GetPosition()
            vias.append((p.x / 1e6, p.y / 1e6, t.GetWidth() / 1e6, t.GetNetname()))
        elif cls == "PCB_ARC":
            s, m, e = t.GetStart(), t.GetMid(), t.GetEnd()
            segs.append((t.GetLayerName(), t.GetNetname(), t.GetWidth() / 1e6, [(s.x / 1e6, s.y / 1e6), (m.x / 1e6, m.y / 1e6), (e.x / 1e6, e.y / 1e6)]))
        else:
            s, e = t.GetStart(), t.GetEnd()
            segs.append((t.GetLayerName(), t.GetNetname(), t.GetWidth() / 1e6, [(s.x / 1e6, s.y / 1e6), (e.x / 1e6, e.y / 1e6)]))
    return segs, vias


def _stats(segs, vias):
    return sum(sum(math.dist(a, b) for a, b in zip(p, p[1:])) for _, _, _, p in segs), len(vias)


def _panel(ax, pat, segs, vias, title, view, drc=None):
    W, H = pat.size
    ax.add_patch(Rectangle((0, 0), W, H, fc="#fbfbf6", ec="k", lw=1.2, zorder=0))
    e = 0.5
    ax.add_patch(Rectangle((e, e), W - 2 * e, H - 2 * e, fc="none", ec="#d62728", lw=0.5, ls=(0, (3, 3)), zorder=1))
    for k in pat.keepouts:
        if k.layers == ["B.Cu"] and len(pat.ref_vias) == 0 and pat.layers and all(set(kk.layers) == {"B.Cu"} for kk in pat.keepouts):
            continue
        ax.add_patch(Polygon(k.poly, closed=True, fc="none", ec="#777777", hatch="..", lw=0.6, zorder=1))
    names = {n: i for i, n in enumerate(pat.nets)}
    for f in pat.fps:
        for p in f.pads:
            col = COL[names[p.net] % 4] if p.net else "#9a9a9a"
            x, y = p.pos
            w, h = p.size
            back = p.layer == "B.Cu"
            g0 = box(x - w / 2, y - h / 2, x + w / 2, y + h / 2)
            if p.angle:
                from shapely.affinity import rotate

                g0 = rotate(g0, -p.angle, origin=p.pos)
            ax.add_patch(Polygon(list(g0.exterior.coords), closed=True, fc=col, ec="k", lw=0.5, alpha=0.7, zorder=2, hatch="////" if back else None))
            if not p.net:
                g = g0.buffer(W_SIG / 2 + CLEAR, join_style=1)
                ax.add_patch(Polygon(list(g.exterior.coords), closed=True, fc="none", ec="#999999", lw=0.5, ls=(0, (1, 2)), zorder=1))
    for layer, net, w, pts in segs:
        if len(pts) == 3:  # arc: three-point polyline (view only)
            pts = [pts[0], pts[1], pts[2]]
        g = LineString(pts).buffer(w / 2, cap_style=1, join_style=1, quad_segs=8)
        back = layer == "B.Cu"
        ax.add_patch(Polygon(list(g.exterior.coords), closed=True, fc=COL[names.get(net, 0) % 4], ec="k", lw=0.3,
                             alpha=0.55 if back else 0.9, hatch="////" if back else None, zorder=4 if not back else 3))
    for x, y, d, net in vias:
        ax.add_patch(Circle((x, y), d / 2, fc="#dddd66", ec="k", lw=0.6, zorder=5))
    ax.set_xlim(view[0], view[1])
    ax.set_ylim(view[3], view[2])
    ax.set_aspect("equal")
    ax.grid(True, lw=0.3, color="#dddddd")
    L, V = _stats(segs, vias)
    ax.set_title(f"{title}\n{L:.3f} mm, {V} via{'s' if V != 1 else ''}" + (f", {drc}" if drc else ""), fontsize=9)
    ax.tick_params(labelsize=7)


def _drc_text(run: Path):
    for f in glob.glob(str(run / "*--metrics.json")):
        m = json.load(open(f))
        return f"DRC new {m.get('routing_new', '?')}, unconnected {m.get('unconnected', '?')}"
    return None


def render(name: str, boards: Path, runs: Path, out: Path) -> Path:
    pat = PATTERNS[name]()
    doc = json.load(open(boards / name / "expected.json"))
    ref = [(t.layer, t.net, t.width, t.pts) for t in pat.ref_tracks]
    refv = [(v.pos[0], v.pos[1], v.dia, v.net) for v in pat.ref_vias]
    v = VIEW[name]
    pw = 5.6
    fig, axes = plt.subplots(1, 3, figsize=(3 * pw + 0.6, pw * (v[3] - v[2]) / (v[1] - v[0]) + 2.4))
    rd = doc.get("reference_drc")
    _panel(axes[0], pat, ref, refv, "expected (optimal path)", v, f"KiCad DRC {rd['violations']}/{rd['unconnected']}" if rd else None)
    for ax, mode in ((axes[1], "seq"), (axes[2], "erbs")):
        pcbs = glob.glob(str(runs / f"{name}-{mode}" / "*-topor" / "*.kicad_pcb"))
        if not pcbs:
            ax.text(0.5, 0.5, f"no result ({mode})", transform=ax.transAxes, ha="center")
            continue
        segs, vias = read_copper(Path(pcbs[0]))
        _panel(ax, pat, segs, vias, f"topor {os.environ.get('TOPOR_FX_REV', 'main 0a53849')}, {'sequential' if mode == 'seq' else 'realise = erbs'}", v,
               _drc_text(runs / f"{name}-{mode}"))
    fig.suptitle(f"{name}: {pat.title}   [{os.environ.get('TOPOR_FX_STATUS', 'PROPOSED, awaiting approval')}]   kills: {', '.join(doc.get('kills', []))}", fontsize=12)
    rules = ("Rules: clearance 0.20 mm, track 0.25 mm, copper to edge 0.50 mm (red dashed), octilinear; "
             + ("B.Cu closed (rule area, one layer). " if not pat.ref_vias else "two layers, B.Cu hatched, via 0.6/0.3, hatched grey = no-via rule area. ")
             + "Dotted grey = clearance envelope of a foreign pad (half width + clearance).")
    fig.text(0.01, 0.01, "\n".join(textwrap.wrap(rules + "  Expected: " + doc["expected"], 190)), fontsize=8, va="bottom")
    fig.tight_layout(rect=(0, 0.1, 1, 0.92))
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=110)
    plt.close(fig)
    return out


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    boards, runs, out = Path(argv[0]), Path(argv[1]), Path(argv[2])
    for name in (argv[3:] or list(PATTERNS)):
        print(render(name, boards, runs, out / f"{name}.png"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
