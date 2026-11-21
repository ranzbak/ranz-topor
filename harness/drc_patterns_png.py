"""PNG + summary of the PROPOSED DRC-cluster patterns (harness/drc_patterns.py): setup | expected | topor per config.

    python -m harness.drc_patterns_png RUNS OUT [name ...]

RUNS/<label>/<name>/ (one panel per label, e.g. main-default) = tools/drc_patterns_run.sh output (harness.route_board: <name>-topor/<name>.kicad_pcb, --metrics.json,
--drc-routed.json). Same view and scale in every panel; F.Cu red, B.Cu blue, kept copper hatched, PTH pads grey with the
drill, the copper-to-edge line dashed, KiCad DRC violations of the routed copy ringed yellow. OUT/<name>.png and
OUT/summary.json (per pattern and config: routed, DRC routing_new, unconnected, vias, new copper length, layers per net,
verdict)."""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Circle, Polygon, Rectangle  # noqa: E402

from harness.drc_patterns import EDGE, PARKED, PATTERNS, ROUND, STATUS, Pat, arc_points, outline, pad_geom, plen  # noqa: E402

COL = {"F.Cu": "#c8372f", "B.Cu": "#2f62c8"}
ROUTING = {"clearance", "shorting_items", "tracks_crossing", "hole_clearance", "copper_edge_clearance", "track_width",
           "via_diameter", "annular_width", "hole_to_hole", "solder_mask_bridge", "track_dangling", "via_dangling",
           "unconnected_items"}


def _poly(ax, g, **kw):
    if g.geom_type == "MultiPolygon":
        for p in g.geoms:
            _poly(ax, p, **kw)
        return
    ax.add_patch(Polygon(list(g.exterior.coords), closed=True, **kw))


def _base(ax, pat: Pat, title: str):
    Wd, Hd = pat.size
    ol = outline(pat)
    _poly(ax, ol, fc="#fbfbf6", ec="k", lw=1.2, zorder=0)
    _poly(ax, ol.buffer(-EDGE, quad_segs=32), fc="none", ec="#888", lw=0.6, ls=(0, (3, 3)), zorder=1)
    for f in pat.fps:
        for p in f.pads:
            g = pad_geom(p)
            if p.drill:
                _poly(ax, g, fc="#b8a24a", ec="k", lw=0.5, alpha=0.85, zorder=2)
                ax.add_patch(Circle(p.pos, p.drill / 2, fc="white", ec="k", lw=0.4, zorder=3))
            else:
                _poly(ax, g, fc=COL[p.layer], ec="k", lw=0.5, alpha=0.45 if p.net else 0.25, zorder=2)
            if p.net:
                ax.text(p.pos[0], p.pos[1], p.net, fontsize=5, ha="center", va="center", zorder=8, color="#111")
    for t in pat.kept:
        from shapely.geometry import LineString

        _poly(ax, LineString(t.pts).buffer(t.width / 2, quad_segs=16), fc=COL[t.layer], ec="k", lw=0.3, alpha=0.35,
              hatch="xx", zorder=2)
    for a in pat.kept_arcs:
        from shapely.geometry import LineString

        _poly(ax, LineString(arc_points(a)).buffer(a.width / 2, quad_segs=16), fc=COL[a.layer], ec="k", lw=0.3,
              alpha=0.35, hatch="xx", zorder=2)
    ax.set_xlim(-0.3, Wd + 0.3)
    ax.set_ylim(Hd + 0.3, -0.3)
    ax.set_aspect("equal")
    ax.set_title(title, fontsize=8)
    ax.tick_params(labelsize=6)


def _copper(ax, segs, vias):
    from shapely.geometry import LineString

    for layer, _net, w, pts in segs:
        if layer not in COL:
            continue
        _poly(ax, LineString(pts).buffer(w / 2, quad_segs=16), fc=COL[layer], ec="k", lw=0.3, alpha=0.75, zorder=4)
    for x, y, d, _net in vias:
        ax.add_patch(Circle((x, y), d / 2, fc="#999", ec="k", lw=0.5, zorder=5))
        ax.add_patch(Circle((x, y), d / 4, fc="white", ec="k", lw=0.3, zorder=6))


def _grid(ax, pat: Pat):
    """grid-snap: the pattern's grid (expect grid_mm) as light dots."""
    g = pat.expect.get("grid_mm")
    if not g:
        return
    Wd, Hd = pat.size
    xs = [i * g for i in range(int(Wd / g) + 1)]
    ys = [j * g for j in range(int(Hd / g) + 1)]
    ax.scatter([x for x in xs for _ in ys], [y for _ in xs for y in ys], s=0.15, c="#9a9a9a", zorder=1, linewidths=0)


def off_grid_bends(pat: Pat, segs) -> list[tuple[float, float]]:
    """grid-snap: bends (corners where two segments of a net meet at an angle) off the pattern's grid (> 1 um)."""
    g = pat.expect.get("grid_mm")
    if not g:
        return []
    ends = {}
    for _layer, net, _w, pts in segs:
        if len(pts) != 2 or math.dist(*pts) < 1e-4:
            continue
        for p, q in ((pts[0], pts[1]), (pts[1], pts[0])):
            ends.setdefault((net, round(p[0], 4), round(p[1], 4)), []).append((p, math.atan2(q[1] - p[1], q[0] - p[0])))
    out = []
    for v in ends.values():
        if len(v) == 2:
            d = abs((v[0][1] - v[1][1] + math.pi) % (2 * math.pi) - math.pi)
            p = v[0][0]
            if abs(d - math.pi) > math.radians(1) and any(abs(c / g - round(c / g)) * g > 0.001 for c in p):
                out.append((round(p[0], 4), round(p[1], 4)))
    return sorted(out)


def _marks(ax, pat: Pat):
    for m in pat.marks:
        ax.annotate(m["text"], xy=m["xy"], xytext=(m["xy"][0] + 0.8, m["xy"][1] - 0.8), fontsize=6, color="#555",
                    arrowprops={"arrowstyle": "->", "color": "#777", "lw": 0.6}, zorder=9)


def _rats(ax, pat: Pat):
    pos = {f"{f.ref}.{p.num}": p.pos for f in pat.fps for p in f.pads}
    for _n, a, b in pat.connections:
        (x1, y1), (x2, y2) = pos[a], pos[b]
        ax.plot([x1, x2], [y1, y2], color="#444", lw=0.6, ls=(0, (2, 2)), zorder=7)


def _is_kept(seg, pat: Pat) -> bool:
    layer, net, w, pts = seg
    for t in pat.kept:
        if t.layer == layer and t.net == net and abs(t.width - w) < 1e-6:
            for a, b in zip(t.pts, t.pts[1:]):
                if (math.dist(a, pts[0]) < 1e-3 and math.dist(b, pts[-1]) < 1e-3) or (math.dist(b, pts[0]) < 1e-3 and math.dist(a, pts[-1]) < 1e-3):
                    return True
    for a in pat.kept_arcs:
        if a.layer == layer and a.net == net and len(pts) == 3 and math.dist(a.start, pts[0]) < 1e-3:
            return True
    return False


def read_copper(pcb: Path):
    import pcbnew

    b = pcbnew.LoadBoard(str(pcb))
    segs, vias = [], []
    for t in b.GetTracks():
        cls = t.GetClass()
        if cls == "PCB_VIA":
            p = t.GetPosition()
            vias.append((p.x / 1e6, p.y / 1e6, t.GetWidth(pcbnew.F_Cu) / 1e6, t.GetNetname()))
        elif cls == "PCB_ARC":
            s, m, e = t.GetStart(), t.GetMid(), t.GetEnd()
            segs.append((t.GetLayerName(), t.GetNetname(), t.GetWidth() / 1e6,
                         [(s.x / 1e6, s.y / 1e6), (m.x / 1e6, m.y / 1e6), (e.x / 1e6, e.y / 1e6)]))
        else:
            s, e = t.GetStart(), t.GetEnd()
            segs.append((t.GetLayerName(), t.GetNetname(), t.GetWidth() / 1e6, [(s.x / 1e6, s.y / 1e6), (e.x / 1e6, e.y / 1e6)]))
    return segs, vias


def _drc_new(run: Path, name: str) -> list[dict]:
    """KiCad DRC violations of the routed copy that the problem board does not have (by type and position)."""
    def load(p):
        if not p.exists():
            return []
        d = json.loads(p.read_text())
        out = []
        for v in d.get("violations", []) + d.get("unconnected_items", []):
            if v["type"] not in ROUTING:
                continue
            xy = [round(sum(i["pos"]["x"] for i in v["items"]) / len(v["items"]), 3),
                  round(sum(i["pos"]["y"] for i in v["items"]) / len(v["items"]), 3)]
            out.append({"type": v["type"], "xy": xy, "text": v["description"]})
        return out
    base = {(v["type"], tuple(v["xy"])) for v in load(run / f"{name}--drc-hand.json")}
    return [v for v in load(run / f"{name}--drc-routed.json") if (v["type"], tuple(v["xy"])) not in base]


def _octi(pts) -> bool:
    for a, b in zip(pts, pts[1:]):
        dx, dy = abs(b[0] - a[0]), abs(b[1] - a[1])
        if dx > 1e-3 and dy > 1e-3 and abs(dx - dy) > 0.01 * max(dx, dy) + 1e-3:
            return False
    return True


def bends(segs) -> dict:
    """Per net: corners where two straight segments of the net meet at an angle (> 1 deg), T junctions not counted."""
    ends = {}
    for _layer, net, _w, pts in segs:
        if len(pts) != 2 or math.dist(*pts) < 1e-4:
            continue
        for p, q in ((pts[0], pts[1]), (pts[1], pts[0])):
            ends.setdefault((net, round(p[0], 3), round(p[1], 3)), []).append(math.atan2(q[1] - p[1], q[0] - p[0]))
    out = {}
    for (net, _x, _y), angs in ends.items():
        if len(angs) == 2:
            d = abs((angs[0] - angs[1] + math.pi) % (2 * math.pi) - math.pi)
            if abs(d - math.pi) > math.radians(1):
                out[net] = out.get(net, 0) + 1
    return dict(sorted(out.items()))


def off_centre_ends(pat: Pat, segs) -> list[tuple[float, float, float]]:
    """Track ends that end inside a pad of their net off its centre (pad-snap, A1b): ends no other segment end of the
    net meets (within 2 um), inside the pad's shape, more than 1 um from its centre: (x, y, distance mm)."""
    from shapely.geometry import Point

    ends = {}
    for _layer, net, _w, pts in segs:
        for q in (pts[0], pts[-1]):
            ends.setdefault((net, round(q[0], 3), round(q[1], 3)), []).append(q)
    out = []
    for (net, _x, _y), qs in ends.items():
        if len(qs) != 1:
            continue
        q = qs[0]
        for f in pat.fps:
            for pd in f.pads:
                if pd.net == net and pad_geom(pd).buffer(1e-6).contains(Point(q)):
                    d = math.dist(q, pd.pos)
                    if d > 0.001:
                        out.append((q[0], q[1], round(d, 3)))
    return sorted(out)


def judge(pat: Pat, segs, vias, m: dict, drc: list[dict]) -> dict:
    new = [s for s in segs if not _is_kept(s, pat)]
    ln = sum(plen(p) for _, _, _, p in new)
    layers = {}
    for layer, net, _, _ in new:
        layers.setdefault(net, set()).add(layer)
    e = pat.expect
    ref_len = sum(plen(t.pts) for t in pat.ref_tracks)
    r = {"routed": f"{m.get('routed')}/{m.get('connections')}", "drc_new": len([v for v in drc if v["type"] != "unconnected_items"]),
         "drc_types": sorted({v["type"] for v in drc}), "unconnected": m.get("unconnected"), "vias": len(vias),
         "len": round(ln, 3), "ref_len": round(ref_len, 3), "layers": {k: sorted(v) for k, v in sorted(layers.items())},
         "free_angle_segments": sum(0 if _octi(p) else 1 for _, _, _, p in new if len(p) == 2), "valid": m.get("valid"),
         "bends": bends(new), "detour": round(ln / ref_len, 3) if ref_len else None}
    bad = r["drc_new"] > 0 or (r["unconnected"] or 0) > 0 or m.get("routed") != m.get("connections")
    same = r["vias"] == e.get("vias", r["vias"]) and ln <= 1.05 * ref_len + 1e-6
    if "layers" in e:
        same = same and all(sorted(e["layers"][n]) == r["layers"].get(n) for n in e["layers"])
    if e.get("octilinear"):
        same = same and r["free_angle_segments"] == 0
    if "bends" in e:
        same = same and all(r["bends"].get(n, 0) <= b for n, b in e["bends"].items())
    if e.get("grid_mm"):
        r["off_grid_bends"] = off_grid_bends(pat, new)
        same = same and len(r["off_grid_bends"]) <= e.get("grid_off_bends_max", 0)
    if e.get("pad_ends_at_centre"):
        r["off_centre_ends"] = len(off_centre_ends(pat, new))
        same = same and r["off_centre_ends"] == 0
    r["verdict"] = "REPRODUCES (DRC/unrouted)" if bad else ("matches expected" if same else "clean, differs from expected")
    return r


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    runs, out = Path(a[0]), Path(a[1])
    names = a[2:] or list(PATTERNS)
    out.mkdir(parents=True, exist_ok=True)
    summ = {}
    for name in names:
        pat = {**PATTERNS, **PARKED}[name]()
        Wd, Hd = pat.size
        cfgs = sorted(c.name for c in runs.iterdir() if (c / name / f"{name}-topor" / f"{name}.kicad_pcb").exists())
        n = 2 + len(cfgs)
        fig, ax = plt.subplots(1, n, figsize=(4.6 * n, 4.6 * Hd / Wd + 1.6))
        _base(ax[0], pat, "setup (problem board; dashed = connections to make)")
        for x in ax:
            _grid(x, pat)
        _rats(ax[0], pat)
        _marks(ax[0], pat)
        _base(ax[1], pat, f"expected (sketch, KiCad DRC 0): {pat.expect.get('vias', 0)} via(s), {sum(plen(t.pts) for t in pat.ref_tracks):.2f} mm")
        _copper(ax[1], [(t.layer, t.net, t.width, t.pts) for t in pat.ref_tracks], [(v.pos[0], v.pos[1], v.dia, v.net) for v in pat.ref_vias])
        _marks(ax[1], pat)
        summ[name] = {"cluster": pat.cluster, "title": pat.title, "round": ROUND.get(name, "unchanged")}
        for k, c in enumerate(cfgs, start=2):
            run = runs / c / name
            segs, vias = read_copper(run / f"{name}-topor" / f"{name}.kicad_pcb")
            m = json.loads((run / f"{name}--metrics.json").read_text())
            drc = _drc_new(run, name)
            r = judge(pat, segs, vias, m, drc)
            summ[name][c] = r
            types = ", ".join(t for t in r["drc_types"] if t != "unconnected_items")
            _base(ax[k], pat, f"topor {c}: {r['verdict']}\nDRC new {r['drc_new']}{' (' + types + ')' if types else ''}, "
                              f"unconnected {r['unconnected']}, vias {r['vias']}\n{r['len']:.2f} mm (x{r['detour']}), "
                              f"free-angle {r['free_angle_segments']}, bends {sum(r['bends'].values())}")
            _copper(ax[k], [s for s in segs if not _is_kept(s, pat)], vias)
            for v in drc:
                ax[k].add_patch(Circle(v["xy"], 0.45, fc="none", ec="#f2c200", lw=2.2, zorder=10))
            if pat.expect.get("pad_ends_at_centre"):
                for x, y, d in off_centre_ends(pat, [s for s in segs if not _is_kept(s, pat)]):
                    ax[k].add_patch(Circle((x, y), 0.3, fc="none", ec="#f2c200", lw=2.2, zorder=10))
                    ax[k].annotate(f"end {d:.2f} mm off centre", xy=(x, y), xytext=(x - 1.6, y + 1.0), fontsize=6,
                                   color="#7a5d00", arrowprops={"arrowstyle": "->", "color": "#b08a00", "lw": 0.6}, zorder=11)
                ax[k].set_title(ax[k].get_title() + f"\npad ends off centre: {r.get('off_centre_ends', 0)} (yellow)", fontsize=8)
            if pat.expect.get("grid_mm"):
                for x, y in r.get("off_grid_bends", []):
                    ax[k].plot([x], [y], marker="x", ms=5, mew=1.2, color="#e07000", zorder=11)
                ax[k].set_title(ax[k].get_title() + f"\nbends off the grid: {len(r.get('off_grid_bends', []))} (orange x)", fontsize=8)
        tag = f" [{ROUND[name]}]" if name in ROUND else ""
        fig.suptitle(f"{pat.cluster} {name}{tag}: {pat.title}\n[{"APPROVED (expected answer in tests/test_drc_patterns_e2e.py)" if "approved" in ROUND.get(name, "") else STATUS}]  F.Cu red, B.Cu blue, hatched = kept hand copper, "
                     "grey = PTH, dashed frame = copper-to-edge line, yellow ring = KiCad DRC of the routed copy", fontsize=9)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=120)
        plt.close(fig)
        print(name, json.dumps({c: summ[name][c]["verdict"] for c in cfgs}))
    (out / "summary.json").write_text(json.dumps(summ, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
