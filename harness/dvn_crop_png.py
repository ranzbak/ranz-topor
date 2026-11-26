"""draw-via-neck: PNG of a qmtech crop (dvn_crop_q2 / q3) as three panels: hand reference | today | this branch, zoomed on
the user's spot. Necked copper (below the net class width) is outlined yellow and its length per focus net is in the panel
title; vias grey. Needs KiCad's pcbnew module (run where KiCad is installed: the VM).

usage: python -m harness.dvn_crop_png spec.json   (keys: ref, today, branch, out, view [x0,y0,x1,y1], focus [nets],
       nominal {net: mm}, marks [[x,y]], title, today_label, branch_label, expected)"""
import json
import math
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import pcbnew  # noqa: E402
from matplotlib.patches import Circle  # noqa: E402
from matplotlib.patches import Polygon as MPoly  # noqa: E402
from shapely.geometry import LineString  # noqa: E402

MM = 1e6
COL = {pcbnew.F_Cu: "#c83737", pcbnew.B_Cu: "#4a7fc1"}


def necked(b, nominal):
    """Necked length (mm) and narrowest width per net of `nominal`."""
    out = {n: [0.0, None] for n in nominal}
    for t in b.GetTracks():
        if t.GetClass() == "PCB_VIA" or t.GetNetname() not in nominal:
            continue
        w = t.GetWidth() / MM
        if w < nominal[t.GetNetname()] - 1e-4:
            s, e = t.GetStart(), t.GetEnd()
            o = out[t.GetNetname()]
            o[0] += math.hypot(e.x - s.x, e.y - s.y) / MM
            o[1] = w if o[1] is None else min(o[1], w)
    return out


def draw(ax, pcb, spec, title):
    b = pcbnew.LoadBoard(pcb)
    x0, y0, x1, y1 = spec["view"]
    focus, nominal = set(spec["focus"]), spec["nominal"]
    for L in (pcbnew.B_Cu, pcbnew.F_Cu):
        for fp in b.GetFootprints():
            for p in fp.Pads():
                if not p.IsOnLayer(L):
                    continue
                sp = p.GetEffectivePolygon(L, pcbnew.ERROR_INSIDE)
                for i in range(sp.OutlineCount()):
                    o = sp.Outline(i)
                    pts = [(o.CPoint(j).x / MM, o.CPoint(j).y / MM) for j in range(o.PointCount())]
                    hi = p.GetNetname() in focus
                    ax.add_patch(MPoly(pts, closed=True, fc=COL[L], ec="k" if hi else "none", lw=1.2 if hi else 0,
                                       alpha=0.55 if L == pcbnew.B_Cu else 0.85))
        for t in b.GetTracks():
            if t.GetClass() == "PCB_VIA" or t.GetLayer() != L:
                continue
            s, e = t.GetStart(), t.GetEnd()
            w = t.GetWidth() / MM
            g = LineString([(s.x / MM, s.y / MM), (e.x / MM, e.y / MM)]).buffer(w / 2)
            net = t.GetNetname()
            thin = net in nominal and w < nominal[net] - 1e-4
            ax.add_patch(MPoly(list(g.exterior.coords), closed=True, fc=COL[L],
                               ec="#ffd400" if thin else ("k" if net in focus else "none"), lw=2.2 if thin else 0.8,
                               alpha=0.95 if net in focus else 0.6, zorder=3 if net in focus else 2))
    for t in b.GetTracks():
        if t.GetClass() != "PCB_VIA":
            continue
        c = (t.GetPosition().x / MM, t.GetPosition().y / MM)
        ax.add_patch(Circle(c, t.GetWidth(pcbnew.F_Cu) / 2 / MM, fc="#9a9a9a", ec="k", lw=0.5, zorder=4))
        ax.add_patch(Circle(c, t.GetDrillValue() / 2 / MM, fc="white", ec="none", zorder=5))
    for (x, y) in spec.get("marks", []):
        ax.plot([x], [y], marker="x", ms=11, mew=2.2, color="#ff00ff", zorder=8)
    nk = necked(b, nominal)
    line = "; ".join(f"{n} necked {v[0]:.2f} mm" + (f" (min {v[1]:.3f})" if v[1] else "") for n, v in nk.items())
    ax.set_xlim(x0, x1)
    ax.set_ylim(y1, y0)
    ax.set_aspect("equal")
    ax.set_title(title + "\n" + line, fontsize=8)
    ax.grid(alpha=0.2)
    return nk


def main():
    spec = json.load(open(sys.argv[1]))
    fig, axs = plt.subplots(1, 3, figsize=(21, 7.8))
    draw(axs[0], spec["ref"], spec, "hand reference (crop of the hand layout: proves the room; not a target)")
    a = draw(axs[1], spec["today"], spec, spec["today_label"])
    b = draw(axs[2], spec["branch"], spec, spec["branch_label"])
    fig.suptitle(spec["title"] + "\nyellow outline = necked below the net class width; magenta x = the user's spot on the full "
                 "board; bold = focus nets", fontsize=10)
    fig.text(0.01, 0.01, "EXPECTED: " + spec["expected"], fontsize=9, wrap=True, va="bottom")
    fig.tight_layout(rect=(0, 0.06, 1, 0.92))
    fig.savefig(spec["out"], dpi=100)
    json.dump({"today": a, "branch": b}, open(spec["out"].rsplit(".", 1)[0] + ".json", "w"), indent=1)


if __name__ == "__main__":
    main()
