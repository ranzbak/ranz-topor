"""stitch-vias r5: classify the stitching vias of a routed board (vias not in the input board) as on the board
lattice, on the via grid only, or off the grid, and print the share.

    python3 tools/stitchvias_lattice_share.py <input.kicad_pcb> <routed.kicad_pcb> [--px 5.08 --py 5.08
        --shift 2.54 --grid 0.254 --ox 0 --oy 0] [--corners notes.txt] [--json]

Lattice: rows at oy + j·py, odd rows shifted by `shift`, columns at ox (+shift) + i·px (KiCad frame, mm).
--corners: a run log / report whose "at (x, y)" stitch notes tagged "corner" mark corner vias (r5 notes).
O(vias).
"""
from __future__ import annotations

import argparse
import json
import re


def vias(path):
    t = open(path).read()
    out = []
    for m in re.finditer(r"\(via\b(.*?)\(uuid", t, re.S):
        b = m.group(1)
        at = re.search(r"\(at ([-\d.]+) ([-\d.]+)\)", b)
        net = re.search(r"\(net (\d+|\"[^\"]*\")\)", b)
        if at:
            out.append((round(float(at.group(1)), 4), round(float(at.group(2)), 4), net.group(1) if net else "?"))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("inp")
    ap.add_argument("routed")
    ap.add_argument("--px", type=float, default=5.08)
    ap.add_argument("--py", type=float, default=5.08)
    ap.add_argument("--shift", type=float, default=2.54)
    ap.add_argument("--grid", type=float, default=0.254)
    ap.add_argument("--ox", type=float, default=0.0)
    ap.add_argument("--oy", type=float, default=0.0)
    ap.add_argument("--corners", default=None)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    before = {(x, y) for x, y, _ in vias(a.inp)}
    new = [(x, y, n) for x, y, n in vias(a.routed) if (x, y) not in before]
    corner = set()
    if a.corners:
        for m in re.finditer(r"corner[^\n]*?\(([-\d.]+), ([-\d.]+)\)", open(a.corners).read()):
            corner.add((round(float(m.group(1)), 3), round(-float(m.group(2)), 3)))
    tol = 1e-3

    def on(v, step, o):
        k = (v - o) / step
        return abs(k - round(k)) * step < tol

    cls = {"lattice": [], "corner": [], "grid": [], "off": []}
    for x, y, n in new:
        j = round((y - a.oy) / a.py)
        lat = on(y, a.py, a.oy) and on(x, a.px, a.ox + (a.shift if j % 2 else 0.0))
        if lat:
            cls["lattice"].append((x, y))
        elif (round(x, 3), round(y, 3)) in corner:
            cls["corner"].append((x, y))
        elif on(x, a.grid, a.ox) and on(y, a.grid, a.oy):
            cls["grid"].append((x, y))
        else:
            cls["off"].append((x, y))
    res = {k: len(v) for k, v in cls.items()}
    res["new"] = len(new)
    res["lattice_or_corner_share"] = round((res["lattice"] + res["corner"]) / max(1, len(new)), 3)
    res["positions"] = {k: v for k, v in cls.items() if k != "lattice"}
    print(json.dumps(res, indent=None if not a.json else 1))


if __name__ == "__main__":
    main()
