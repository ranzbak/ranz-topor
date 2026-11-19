#!/usr/bin/env python3
"""TW7 pattern PROPOSALS (design round, not in any suite): sketch boards, careless and expected states, as PNG + numbers.

Each pattern is a small 2-layer board (SMD pads on F.Cu, KiCad frame, mm, y down) with rules Default 0.25 mm / 0.20 mm
clearance, PWR 0.50 mm, via 0.6 / 0.3. Every state is a list of wires (net, width, layer, polyline) and vias; the
script computes each wire's length, the detour ratio (length / straight pad-to-pad), the price under the archive's
overall-best rule (length + 25 mm per via, `[optimize] mm_per_via`), and the smallest copper gap of every wire to the
other nets' copper and pads (must be >= 0.20 mm: the sketch checks its own legality).

  python3 tools/tw7_sketch.py --check            numbers only (no rendering), JSON on stdout
  python3 tools/tw7_sketch.py OUT_DIR            numbers + one PNG per pattern (render on the VM, AGENT_RULES)

Sources: Luzin 2015 (docs/literature/luzin2015-nurnberg-flexible-routing.pdf) pp. 7-10, figs. 15-21; Luzin, Popov &
Popov 2012 (luzin-popov2012-clinch-removal.pdf) pp. 116-118, figs. 1, 3, 4. Deterministic, O(segments^2) per state.
"""
from __future__ import annotations

import json
import math
import sys

CLEAR = 0.20
SIG, PWR = 0.25, 0.50
VIA_D = 0.6
MM_PER_VIA = 25.0


def pad(name, net, x, y, w=1.0, h=1.0):
    return {"name": name, "net": net, "x": x, "y": y, "w": w, "h": h}


def wire(net, width, pts, layer="F"):
    return {"net": net, "w": width, "pts": pts, "layer": layer}


def seglen(p):
    return sum(math.dist(a, b) for a, b in zip(p, p[1:]))


def _pt_seg(p, a, b):
    ax, ay = b[0] - a[0], b[1] - a[1]
    L2 = ax * ax + ay * ay
    t = 0.0 if L2 == 0 else max(0.0, min(1.0, ((p[0] - a[0]) * ax + (p[1] - a[1]) * ay) / L2))
    return math.hypot(p[0] - a[0] - t * ax, p[1] - a[1] - t * ay)


def _seg_seg(a, b, c, d):
    o = lambda p, q, r: (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    if o(c, d, a) * o(c, d, b) < 0 and o(a, b, c) * o(a, b, d) < 0:
        return 0.0
    return min(_pt_seg(a, c, d), _pt_seg(b, c, d), _pt_seg(c, a, b), _pt_seg(d, a, b))


def _seg_rect(a, b, r):
    """Distance of segment ab to the axis-aligned pad rectangle r (0 inside)."""
    x0, x1, y0, y1 = r["x"] - r["w"] / 2, r["x"] + r["w"] / 2, r["y"] - r["h"] / 2, r["y"] + r["h"] / 2
    inside = lambda p: x0 <= p[0] <= x1 and y0 <= p[1] <= y1
    if inside(a) or inside(b):
        return 0.0
    edges = [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))]
    return min(_seg_seg(a, b, c, d) for c, d in edges)


def check_state(pads, st):
    """Per wire: length, vias on it, smallest gap to other nets' wires (same layer) and pads (F.Cu)."""
    out = []
    for i, w in enumerate(st["wires"]):
        segs = list(zip(w["pts"], w["pts"][1:]))
        gap = math.inf
        pair = {}
        for j, o in enumerate(st["wires"]):
            if j == i or o["net"] == w["net"] or o["layer"] != w["layer"]:
                continue
            for a, b in segs:
                for c, d in zip(o["pts"], o["pts"][1:]):
                    g = _seg_seg(a, b, c, d) - (w["w"] + o["w"]) / 2
                    gap = min(gap, g)
                    pair[o["net"]] = min(pair.get(o["net"], math.inf), g)
        if w["layer"] == "F":
            for p in pads:
                if p["net"] != w["net"]:
                    for a, b in segs:
                        g = _seg_rect(a, b, p) - w["w"] / 2
                        gap = min(gap, g)
                        pair["pad " + p["name"]] = min(pair.get("pad " + p["name"], math.inf), g)
        for v in st.get("vias", []):
            if v["net"] != w["net"]:
                for a, b in segs:
                    gap = min(gap, _pt_seg(v["at"], a, b) - VIA_D / 2 - w["w"] / 2)
        out.append({"net": w["net"], "layer": w["layer"], "len": round(seglen(w["pts"]), 2), "min_gap": round(gap, 3),
                    "gaps": {k: round(v, 3) for k, v in sorted(pair.items())}})
    return out


def summarise(pads, st, straight):
    rows = check_state(pads, st)
    by = {}
    for r in rows:
        by.setdefault(r["net"], {"len": 0.0, "min_gap": math.inf, "gaps": {}})
        by[r["net"]]["len"] += r["len"]
        for k, v in r["gaps"].items():
            by[r["net"]]["gaps"][k] = min(by[r["net"]]["gaps"].get(k, math.inf), v)
        by[r["net"]]["min_gap"] = min(by[r["net"]]["min_gap"], r["min_gap"])
    vias = st.get("vias", [])
    nv = {}
    for v in vias:
        nv[v["net"]] = nv.get(v["net"], 0) + 1
    tot_len = sum(x["len"] for x in by.values())
    for n, x in by.items():
        x["len"] = round(x["len"], 2)
        x["vias"] = nv.get(n, 0)
        x["ratio"] = round(x["len"] / straight[n], 2) if straight.get(n) else None
    return {"nets": by, "length": round(tot_len, 2), "vias": len(vias),
            "price": round(tot_len + MM_PER_VIA * len(vias), 2), "legal": all(x["min_gap"] >= CLEAR - 1e-6 for x in by.values())}


def around(c, side, w, extra=0.0):
    """Corner points of a pad's clearance hull for a wire of width w: the pad grown by CLEAR + w/2."""
    g = CLEAR + w / 2 + extra
    x0, x1, y0, y1 = c["x"] - c["w"] / 2 - g, c["x"] + c["w"] / 2 + g, c["y"] - c["h"] / 2 - g, c["y"] + c["h"] / 2 + g
    return {"tl": (x0, y0), "tr": (x1, y0), "bl": (x0, y1), "br": (x1, y1)}[side]


def dip_rail(c, y_rail, off, w=PWR, x0=10.0, x1=50.0):
    """45 deg dip of a horizontal rail (y_rail, x0..x1) under pad c: the innermost rail's legs and bottom are at the
    clearance from the pad (legs tangent to the clearance circle at the pad's lower corners); `off` = perpendicular
    offset outward (a parallel rail). Lines x - y = cl (left leg), x + y = cr (right leg), y = yb (bottom)."""
    g = CLEAR + w / 2
    r2 = math.sqrt(2.0)
    lx, rx, by = c["x"] - c["w"] / 2, c["x"] + c["w"] / 2, c["y"] + c["h"] / 2
    cl = (lx - by) - g * r2 - off * r2
    cr = (rx + by) + g * r2 + off * r2
    yb = by + g + off
    return [(x0, y_rail), (cl + y_rail, y_rail), (cl + yb, yb), (cr - yb, yb), (cr - y_rail, y_rail), (x1, y_rail)]


# ---------------------------------------------------------------------------------------------- patterns
def p_detour_trade():
    """Fig. 21b -> 21c: the via-for-length detour (TW5 / TW4 r2 A/B nets SDRAM_D13, JS1_1, IO7 R10.1-U3.21)."""
    P = {k: pad(*v) for k, v in {
        "B1a": ("B1a", "B1", 10, 14), "B1b": ("B1b", "B1", 50, 14),
        "B2a": ("B2a", "B2", 10, 15.5), "B2b": ("B2b", "B2", 50, 15.5),
        "A1": ("A1", "A", 30, 5), "A2": ("A2", "A", 30, 19)}.items()}
    pads = list(P.values())
    straight = {"A": 14.0, "B1": 40.0, "B2": 40.0}
    rails = [wire("B1", PWR, [(10, 14), (50, 14)]), wire("B2", PWR, [(10, 15.5), (50, 15.5)])]
    # (a) via pair: A straight, its stretch under the rails on B.Cu (fig. 21a).
    va, vb = (30, 11.5), (30, 18.0)
    s_a = {"wires": rails + [wire("A", SIG, [(30, 5), va]), wire("A", SIG, [va, vb], "B"), wire("A", SIG, [vb, (30, 19)])],
           "vias": [{"net": "A", "at": va}, {"net": "A", "at": vb}]}
    # (b) careless: A round the rails' right-hand pads (one single re-lay; fig. 21b).
    tr, br = around(P["B1b"], "tr", SIG), around(P["B2b"], "br", SIG)
    s_b = {"wires": rails + [wire("A", SIG, [(30, 5), tr, br, (30, 19)])], "vias": []}
    # (c) expected: both rails dip round A2 at 45 deg, A straight (fig. 21c; Luzin-Popov 2013 step 8.1). B1 hugs A2's
    # pad at exactly the clearance (chamfered hull: the 45 deg legs tangent to the clearance circle at the pad's lower
    # corners); B2 is B1's true parallel offset at clearance (user 2026-11-19, rule "bundles tight-packed": width + 0.2 =
    # 0.7 mm centre pitch, 0.2 mm copper gap at the V; was the rails' 1.5 mm pitch, a 1.0 mm gap).
    s_c = {"wires": [wire("B1", PWR, dip_rail(P["A2"], 14.0, 0.0)), wire("B2", PWR, dip_rail(P["A2"], 15.5, 0.5 + CLEAR)),
                     wire("A", SIG, [(30, 5), (30, 19)])], "vias": []}
    return {
        "name": "detour_trade", "fig": "Luzin 2015 p. 9-10 fig. 21a-c; Luzin 2013 p. 104 fig. 21; Luzin-Popov 2013 p. 134 step 8.1",
        "board": [60, 30], "pads": pads, "straight": straight,
        "text": ("Two 0.5 mm rails B1, B2 (PWR, laid first: wider) cross the line of a 0.25 mm wire A. A's single re-lay "
                 "prefers the way round the rails' ends (b) to two vias (a): a via-for-length detour, ratio > 1.5. No single "
                 "re-lay of B1 or B2 helps (each would cross A's way round). Expected (c): A straight, B1 and B2 dip round A2 "
                 "together at 45 deg, B1 at the clearance from A2, B2 packed against B1 at the clearance (0.2 mm "
                 "copper gap at the V), 0 vias, +4.9 mm (B1) / +4.3 mm (B2). Today's pair_move does not fire: A has no via pair in state (b)."),
        "states": [("(a) via pair, fig. 21a", s_a), ("(b) careless: via-for-length detour, fig. 21b", s_b),
                   ("(c) expected: rails round A2, fig. 21c", s_c)],
    }


def p_terminal_clinch():
    """Fig. 18 / Luzin-Popov 2012 figs. 1, 3: a regular clinch at a terminal (sign 1)."""
    P = {k: pad(*v) for k, v in {"b1": ("b1", "N", 20, 10), "t1": ("t1", "N", 5, 10),
                                 "m1": ("m1", "M", 18.8, 2.5), "m2": ("m2", "M", 18.8, 17.5)}.items()}
    pads = list(P.values())
    straight = {"N": 15.0, "M": 15.0}
    m_str = wire("M", PWR, [(18.8, 2.5), (18.8, 17.5)])
    s_b = {"wires": [m_str, wire("N", SIG, [(20, 10), around(P["m1"], "tr", SIG), around(P["m1"], "tl", SIG), (5, 10)])], "vias": []}
    s_c = {"wires": [wire("M", PWR, [(18.8, 2.5), around(P["b1"], "tr", PWR), around(P["b1"], "br", PWR), (18.8, 17.5)]),
                     wire("N", SIG, [(20, 10), (5, 10)])], "vias": []}
    return {
        "name": "terminal_clinch", "fig": "Luzin 2015 p. 8 fig. 17, p. 9 fig. 18 (sign a); Luzin-Popov 2012 pp. 116-118 sign 1, figs. 1, 3",
        "board": [26, 20], "pads": pads, "straight": straight,
        "text": ("M (0.5 mm, laid first) runs straight past N's terminal b1, between b1 and N's target t1. N goes round M's "
                 "end m1 (b): crossing M would cost two vias. Expected (c): M bends round b1 on its far side, N straight. "
                 "The existing terminal_clinches detector sees this; today the whole crossing pass is judged at once."),
        "states": [("(b) careless: N round M's end", s_b), ("(c) expected: M round b1, N straight", s_c)],
    }


def p_u_clinch():
    """Fig. 19 / Luzin-Popov 2012 fig. 4: U-clinch, two equipotential pads with a foreign wire between them (sign 2)."""
    P = {k: pad(*v) for k, v in {"b1": ("b1", "N", 12, 11), "b2": ("b2", "N", 16, 11),
                                 "m1": ("m1", "M", 14, 2.5), "m2": ("m2", "M", 14, 19.5)}.items()}
    pads = list(P.values())
    straight = {"N": 4.0, "M": 17.0}
    m_str = wire("M", PWR, [(14, 2.5), (14, 19.5)])
    s_b = {"wires": [m_str, wire("N", SIG, [(12, 11), around(P["m1"], "tl", SIG), around(P["m1"], "tr", SIG), (16, 11)])], "vias": []}
    s_c = {"wires": [wire("M", PWR, [(14, 2.5), around(P["b1"], "tl", PWR), around(P["b1"], "bl", PWR), (14, 19.5)]),
                     wire("N", SIG, [(12, 11), (16, 11)])], "vias": []}
    return {
        "name": "u_clinch", "fig": "Luzin 2015 p. 8 (sign b), p. 9 fig. 19; Luzin-Popov 2012 pp. 116-118 sign 2, fig. 4, removal p. 118",
        "board": [28, 22], "pads": pads, "straight": straight,
        "text": ("N's two pads b1, b2 share a triangulation edge; M (0.5 mm, laid first) runs straight between them. N goes "
                 "round M's end (b). Expected (c): M round b1 (or b2, same price: both accepted), N along the edge. "
                 "Removal per Luzin-Popov 2012 p. 118: lay the interfering wire on the other side of b1, then N along the edge."),
        "states": [("(b) careless: N round M's end", s_b), ("(c) expected: M round b1, N along the edge", s_c)],
    }


PATTERNS = [p_detour_trade, p_terminal_clinch, p_u_clinch]


def numbers():
    out = {}
    for f in PATTERNS:
        p = f()
        out[p["name"]] = {"fig": p["fig"], "text": p["text"],
                          "states": {lab: summarise(p["pads"], st, p["straight"]) for lab, st in p["states"]}}
    return out


def render(out_dir):
    import os
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Circle, Polygon, Rectangle

    colours = {"A": "#1b9e77", "N": "#1b9e77", "B1": "#d95f02", "B2": "#7570b3", "M": "#d95f02"}
    os.makedirs(out_dir, exist_ok=True)
    nums = numbers()
    for f in PATTERNS:
        p = f()
        n = len(p["states"])
        fig, axes = plt.subplots(1, n, figsize=(5.2 * n, 5.2 * p["board"][1] / p["board"][0] + 1.6))
        axes = axes if n > 1 else [axes]
        for ax, (lab, st) in zip(axes, p["states"]):
            W, H = p["board"]
            ax.add_patch(Rectangle((0, 0), W, H, fill=False, lw=1.0, ec="black"))
            for q in p["pads"]:
                ax.add_patch(Rectangle((q["x"] - q["w"] / 2, q["y"] - q["h"] / 2), q["w"], q["h"], fc=colours.get(q["net"], "grey"),
                                       ec="black", lw=0.6, alpha=0.9))
                ax.add_patch(Rectangle((q["x"] - q["w"] / 2 - CLEAR, q["y"] - q["h"] / 2 - CLEAR), q["w"] + 2 * CLEAR,
                                       q["h"] + 2 * CLEAR, fill=False, ec="grey", lw=0.4, ls=":"))
                ax.text(q["x"], q["y"] - q["h"] / 2 - 0.35, q["name"], ha="center", va="bottom", fontsize=7)
            for w in st["wires"]:
                c = colours.get(w["net"], "grey")
                for a, b in zip(w["pts"], w["pts"][1:]):
                    dx, dy = b[0] - a[0], b[1] - a[1]
                    L = math.hypot(dx, dy) or 1.0
                    nx, ny = -dy / L * w["w"] / 2, dx / L * w["w"] / 2
                    poly = [(a[0] + nx, a[1] + ny), (b[0] + nx, b[1] + ny), (b[0] - nx, b[1] - ny), (a[0] - nx, a[1] - ny)]
                    if w["layer"] == "F":
                        ax.add_patch(Polygon(poly, fc=c, ec="black", lw=0.4))
                    else:
                        ax.add_patch(Polygon(poly, fc="white", ec=c, lw=0.8, ls="--", hatch="////"))
                for pt in w["pts"]:
                    ax.add_patch(Circle(pt, w["w"] / 2, fc=c if w["layer"] == "F" else "white", ec="none"))
            for v in st.get("vias", []):
                ax.add_patch(Circle(v["at"], VIA_D / 2, fc="#cccccc", ec="black", lw=0.6))
                ax.add_patch(Circle(v["at"], 0.15, fc="white", ec="black", lw=0.4))
            s = nums[p["name"]]["states"][lab]
            nets = ", ".join(f"{k} {x['len']} mm" + (f" (x{x['ratio']})" if x["ratio"] else "") + (f", {x['vias']} via" if x["vias"] else "")
                             for k, x in sorted(s["nets"].items()))
            ax.set_title(f"{lab}\n{nets}\nprice {s['price']} (mm + 25/via), legal {s['legal']}", fontsize=8)
            ax.set_xlim(-0.5, W + 0.5)
            ax.set_ylim(H + 0.5, -0.5)
            ax.set_aspect("equal")
            ax.tick_params(labelsize=6)
        fig.suptitle(f"{p['name']} (PROPOSED, not in the suite): {p['fig']}\nsolid = F.Cu, hatched dashed = B.Cu; dotted = pad "
                     f"clearance 0.20; colour = net", fontsize=8)
        fig.tight_layout()
        fig.savefig(os.path.join(out_dir, f"{p['name']}.png"), dpi=130)
        plt.close(fig)
    with open(os.path.join(out_dir, "numbers.json"), "w") as fh:
        json.dump(nums, fh, indent=1)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] != "--check":
        render(sys.argv[1])
    json.dump(numbers(), sys.stdout, indent=1)
