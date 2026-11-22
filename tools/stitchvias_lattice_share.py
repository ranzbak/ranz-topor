"""stitch-vias r5: the share of a run's stitching vias that stand on the board lattice or at a piece extremity
(user 2026-11-22: "I expected either for the placer to search for the corners, or to have a placement grid"),
from the run's notes (result.json route.warnings), plus the per-piece lines (class, layer, area, vias of wanted).

    python3 -m tools.stitchvias_lattice_share RESULT.json --lattice px,py,shift,ox,oy [--grid G] [--json]

Lattice in mm, KiCad frame: rows at oy + j·py, odd rows shifted by `shift`. --grid: also count the vias on the
via grid (origin ox, oy). Kinds: lattice, corner (the engine's "extremities at" note), other (on neither).
O(notes).
"""
from __future__ import annotations

import argparse
import json
import re

from tools.stitchvias_png import on_lattice, stitch_points


def share(notes: list[str], lattice, grid: float | None = None) -> dict:
    pts, corners = stitch_points(notes)
    kinds = {"lattice": 0, "corner": 0, "other": 0}
    others = []
    on_grid = 0
    for p in pts:
        if (round(p[0], 3), round(p[1], 3)) in corners:
            kinds["corner"] += 1
        elif on_lattice(p, lattice):
            kinds["lattice"] += 1
        else:
            kinds["other"] += 1
            others.append(p)
        if grid:
            ox, oy = lattice[3], lattice[4]
            if all(abs((v - o) / grid - round((v - o) / grid)) * grid < 1e-3 for v, o in ((p[0], ox), (p[1], oy))):
                on_grid += 1
    out = dict(kinds, vias=len(pts), share=round((kinds["lattice"] + kinds["corner"]) / max(1, len(pts)), 3),
               others=others)
    if grid:
        out["on_grid"] = on_grid
    out["placement_notes"] = [n for n in notes if " placement: " in n]
    out["pieces"] = [n[:160] for n in notes if re.match(r"stitch \((dropped island|thin neck)", n)]
    cut: dict = {}
    for n in notes:
        m = re.match(r"stitch via [\d.]+ mm at \([-\d.]+, [-\d.]+\) for (.*)$", n)
        if m:
            cut[m.group(1)] = cut.get(m.group(1), 0) + 1
    out["cut_pieces"] = [f"{k[:110]}: {v} via(s)" for k, v in cut.items()]
    out["no_site"] = [n[:160] for n in notes if n.startswith("no stitch site for")]
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("result")
    ap.add_argument("--lattice", required=True)
    ap.add_argument("--grid", type=float, default=None)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args(argv)
    lat = tuple(float(v) for v in a.lattice.split(","))
    res = share(json.load(open(a.result))["route"]["warnings"], lat, a.grid)
    if a.json:
        print(json.dumps(res, indent=1))
    else:
        print(f"vias {res['vias']}: lattice {res['lattice']}, corner {res['corner']}, other {res['other']} "
              f"-> share {res['share']}" + (f", on grid {res['on_grid']}" if a.grid else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
