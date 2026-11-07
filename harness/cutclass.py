"""Inflated sketch U1 (docs/dev/inflated-sketch.md §6.1): classify every violation of a run with the run's per-layer
cut report (`[capacity] cut_report`).

    .venv/bin/python -m harness.cutclass <run> <cut_report_dir> [--out DIR] [--png 3] [--label NAME]

<run> is a route_board output directory (its reports/**/result.json and <name>-topor/<name>.kicad_pcb) or a
result.json. The cut report of the picked run and cycle (`run K picked`, the picked variant "round C") is
<cut_report_dir>/run<K>/cycle<C>.json, its "final" part being the plan handed to the drawing.

Each violation (result.json `route.violations`: layer, `at`, conn) gets one class, the first that matches:

- **planner: via site**: its class is `via_site` (a via the plan placed without a site).
- **no static room**: its class is `static` (the drawing found no room against the static board alone).
- **planner: over-full cut**: a cut on its layer within two pitches (w + s of its wire) of `at` that its wire
  crosses or wraps has slack < 0. Sub-classes: *static* (the wire alone overflows the cut), *neck not planned* (the
  cut fits at the neck floor widths), *model gap* (no over-full edge-layer of the per-edge model within reach: only
  the per-layer cut test sees it), else *seen by both*.
- **planner: over-full cut nearby**: an over-full cut within reach that the wire does not cross.
- **drawing loss**: the plan has a stretch of the wire on that layer within reach and every cut within reach has
  slack >= 0 (sub-class: the violation's own class, e.g. pad_exit).
- **drawing off the plan**: the wire's copper violates where the plan has no stretch of it (the planned stretch on
  that layer is further than two pitches away, or the plan has none on that layer): the drawing left the planned
  route (a forced fallback or a detour); the cut test says nothing about that place.
- **other**: stated (the connection is not in the plan, the layer is not measured).

Writes <out>/classes.json (per violation), <out>/classes.md (tables per class and per connection) and, with --png,
close-ups of the first N violations per class (violation ringed in yellow, over-full cuts red, cuts with room
green (only cuts the wire crosses), the planned bands dashed (the wire's blue), planned vias dashed circles (red: no site), the drawn copper orange).
"""
from __future__ import annotations

import argparse
import glob
import json
import math
import re
from collections import Counter, defaultdict
from pathlib import Path

OVER = -0.001  # mm: a cut with slack below this is over-full (1 um, the report's rounding)


def seg_dist(p, a, b) -> float:
    ax, ay = a
    bx, by = b
    px, py = p
    dx, dy = bx - ax, by - ay
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 <= 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / l2))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def find_result(run: Path) -> Path:
    if run.is_file():
        return run
    hits = sorted(glob.glob(str(run / "**" / "result.json"), recursive=True))
    if not hits:
        raise SystemExit(f"no result.json under {run}")
    return Path(hits[-1])


def picked(result: dict) -> tuple[int, int]:
    """(run K, cycle C) of the result the run returned."""
    ro = result["route"]
    k = 1
    for w in ro.get("warnings", []):
        m = re.search(r"seeded runs \(\[optimize\] runs\): run (\d+) picked", w)
        if m:
            k = int(m.group(1))
    c = None
    for v in ro.get("variants", []) or []:
        if v.get("picked"):
            m = re.match(r"round (\d+)", v.get("label", ""))
            if m:
                c = int(m.group(1))
    if c is None:
        for w in ro.get("warnings", []):
            m = re.match(rf"run {k} cycles .*?picked round (\d+)", w)
            if m:
                c = int(m.group(1))
    return k, c or 1


def classify(result: dict, rep: dict) -> list[dict]:
    fin = rep["final"]
    cuts_by_layer = {L["layer"]: L["cuts"] for L in fin["layers"]}
    st_by = defaultdict(list)
    for s in fin["stretches"]:
        st_by[(s["conn"], s["layer"])].append(s)
    pitch_of = {}
    for s in fin["stretches"]:
        pitch_of[s["conn"]] = max(pitch_of.get(s["conn"], 0.0), s["w"] + s.get("s", 0.2))
    edge_over = defaultdict(list)
    for e in fin.get("edge_over", []):
        edge_over[e["layer"]].append((e["a"], e["b"]))
    out = []
    for i, v in enumerate(result["route"]["violations"]):
        at = (v["at"][0] / 1e6, v["at"][1] / 1e6)
        conn, layer = v.get("conn", -1), v.get("layer", "")
        r = 2.0 * pitch_of.get(conn, 0.4)
        row = {"i": i, "conn": conn, "name": v.get("name", ""), "from": v.get("from", ""), "to": v.get("to", ""),
               "layer": layer, "at_mm": [round(at[0], 3), round(at[1], 3)], "kind": v.get("kind"),
               "vclass": v.get("class"), "against": v.get("against"), "radius_mm": round(r, 3)}
        near = [c for c in cuts_by_layer.get(layer, []) if seg_dist(at, c["p"], c["q"]) <= r]
        mine = [c for c in near if any(it[0] == conn for it in c["items"])]
        over_mine = [c for c in mine if c["slack"] < OVER]
        over_near = [c for c in near if c["slack"] < OVER]
        has_band = any(seg_dist(at, s["band"][j], s["band"][j + 1]) <= r
                       for s in st_by.get((conn, layer), []) for j in range(len(s["band"]) - 1))
        row["cuts_near"] = len(near)
        row["min_slack_near"] = min((c["slack"] for c in near), default=None)
        row["min_slack_mine"] = min((c["slack"] for c in mine), default=None)
        if v.get("class") == "via_site":
            cls, sub = "planner: via site", ""
        elif v.get("class") == "static":
            cls, sub = "no static room", ""
        elif over_mine:
            worst = min(over_mine, key=lambda c: c["slack"])
            row["cut"] = {k: worst[k] for k in ("p", "q", "cap", "need", "need_other", "need_floor", "slack", "kind")}
            alone = min((it[6] for it in worst["items"] if it[0] == conn), default=0.0)
            gap = not any(seg_dist(at, a, b) <= r for a, b in edge_over.get(layer, []))
            cls = "planner: over-full cut"
            if alone < OVER:
                sub = "static (the wire alone overflows the cut)"
            elif worst["cap"] - worst["need_floor"] >= OVER:
                sub = "neck not planned (fits at neck floors)"
            elif gap:
                sub = "model gap (per-edge model: fits)"
            else:
                sub = "seen by both models"
        elif over_near:
            worst = min(over_near, key=lambda c: c["slack"])
            row["cut"] = {k: worst[k] for k in ("p", "q", "cap", "need", "need_other", "need_floor", "slack", "kind")}
            cls, sub = "planner: over-full cut nearby", "the wire does not cross it"
        elif has_band:
            cls, sub = "drawing loss", v.get("class") or ""
        elif layer not in cuts_by_layer:
            cls, sub = "other", f"layer {layer} not measured (no signal layer of the plan)"
        elif not any(s for (c_, _), ss in st_by.items() if c_ == conn for s in ss):
            cls, sub = "other", "the connection has no stretch in the plan (dropped or not planned)"
        elif st_by.get((conn, layer)):
            d = min(seg_dist(at, s["band"][j], s["band"][j + 1]) for s in st_by[(conn, layer)] for j in range(len(s["band"]) - 1))
            cls = "drawing off the plan"
            sub = "planned stretch on this layer " + ("< 2 mm" if d < 2 else "2-5 mm" if d < 5 else ">= 5 mm") + " away"
        else:
            cls, sub = "drawing off the plan", "no planned stretch of the wire on this layer"
        row["class"], row["sub"] = cls, sub
        out.append(row)
    return out


ROUTING_DRC = ("clearance", "shorting_items", "tracks_crossing", "hole_clearance", "copper_edge_clearance",
               "track_dangling", "via_dangling")


def join_drc(rows: list[dict], drc: dict, radius: float = 0.5) -> dict:
    """KiCad's DRC of the routed board joined to the rows: per row the routing-type DRC items within `radius` mm of
    its `at` (any item position), and the DRC items no row is near."""
    items = []
    for v in drc.get("violations", []):
        if v.get("type") in ROUTING_DRC:
            items.append((v["type"], [(it["pos"]["x"], it["pos"]["y"]) for it in v.get("items", [])]))
    used = set()
    for r in rows:
        x, y = r["at_mm"]
        near = [i for i, (_, ps) in enumerate(items) if any(math.hypot(px - x, py - y) <= radius for px, py in ps)]
        r["drc_near"] = len(near)
        used.update(near)
    left = Counter(items[i][0] for i in range(len(items)) if i not in used)
    return {"drc_routing_items": len(items), "matched": len(used), "unmatched_by_type": dict(left)}


def tables(rows: list[dict], title: str, drc: dict | None = None) -> str:
    lines = [f"## {title}", "", f"{len(rows)} violations.", ""]
    if drc:
        lines += [f"KiCad DRC of the routed board: {drc['drc_routing_items']} routing items (clearance, short, crossing, "
                  f"hole, edge, dangling), {drc['matched']} within 0.5 mm of a classified violation; not near any: "
                  f"{drc['unmatched_by_type'] or 'none'}.", ""]
    lines += ["| Class | Sub-class | Count |", "|---|---|---|"]
    by = Counter((r["class"], r["sub"]) for r in rows)
    for (c, s), n in sorted(by.items(), key=lambda x: (x[0][0], -x[1])):
        lines.append(f"| {c} | {s} | {n} |")
    tot = Counter(r["class"] for r in rows)
    lines += ["", "| Class | Count |", "|---|---|"] + [f"| {c} | {n} |" for c, n in tot.most_common()]
    lines += ["", "Per connection:", "", "| conn | net | from -> to | classes |", "|---|---|---|---|"]
    per = defaultdict(list)
    for r in rows:
        per[r["conn"]].append(r)
    for conn, rs in sorted(per.items(), key=lambda x: -len(x[1])):
        cc = Counter(r["class"] + (f" / {r['sub']}" if r["sub"] else "") for r in rs)
        lines.append(f"| {conn} | {rs[0]['name']} | {rs[0]['from']} -> {rs[0]['to']} | "
                     + "; ".join(f"{k} x{n}" for k, n in cc.most_common()) + " |")
    return "\n".join(lines) + "\n"


def closeup(rows: list[dict], rep: dict, pcb: Path | None, out: Path, n: int, label: str) -> list[str]:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Circle, Polygon
    pads, tracks = {}, {}
    if pcb and pcb.exists():
        import pcbnew
        b = pcbnew.LoadBoard(str(pcb))
        for lname in {r["layer"] for r in rows}:
            lid = b.GetLayerID(lname)
            if lid < 0:
                continue
            ps = []
            for p in b.GetPads():
                if not p.IsOnLayer(lid):
                    continue
                try:
                    poly = p.GetEffectivePolygon(lid, pcbnew.ERROR_INSIDE)
                    ol = poly.Outline(0)
                    ps.append([(ol.CPoint(k).x / 1e6, ol.CPoint(k).y / 1e6) for k in range(ol.PointCount())])
                except Exception:  # noqa: BLE001
                    bb = p.GetBoundingBox()
                    x0, y0, x1, y1 = bb.GetLeft() / 1e6, bb.GetTop() / 1e6, bb.GetRight() / 1e6, bb.GetBottom() / 1e6
                    ps.append([(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
            pads[lname] = ps
            tracks[lname] = [((t.GetStart().x / 1e6, t.GetStart().y / 1e6), (t.GetEnd().x / 1e6, t.GetEnd().y / 1e6),
                              t.GetWidth() / 1e6, t.GetNetCode()) for t in b.GetTracks()
                             if t.GetLayer() == lid and t.GetClass() in ("PCB_TRACK", "PCB_ARC")]
            tracks[lname] += [((t.GetPosition().x / 1e6, t.GetPosition().y / 1e6), None, t.GetWidth(lid) / 1e6, t.GetNetCode())
                              for t in b.GetTracks() if t.GetClass() == "PCB_VIA"]
    fin = rep["final"]
    cuts_by_layer = {L["layer"]: L["cuts"] for L in fin["layers"]}
    files = []
    by = defaultdict(list)
    for r in rows:
        by[r["class"]].append(r)
    for cls, rs in sorted(by.items()):
        for r in rs[:n]:
            x, y = r["at_mm"]
            h = max(1.5, 4 * r["radius_mm"])
            fig, ax = plt.subplots(figsize=(6, 6), dpi=130)
            for poly in pads.get(r["layer"], []):
                if any(abs(px - x) < h and abs(py - y) < h for px, py in poly):
                    ax.add_patch(Polygon(poly, closed=True, fc="#c9b26b", ec="#8a7430", lw=0.5, alpha=0.8))
            for a, bb, w, net in tracks.get(r["layer"], []):
                if bb is None:
                    if abs(a[0] - x) < h and abs(a[1] - y) < h:
                        ax.add_patch(Circle(a, w / 2, fc="#999", ec="none", alpha=0.6))
                    continue
                if min(seg_dist((x, y), a, bb), 9) < h * 1.5:
                    ax.plot([a[0], bb[0]], [a[1], bb[1]], color="#e69138", lw=max(0.5, w * 72 * 6 / (2 * h) * 0.8),
                            alpha=0.55, solid_capstyle="round")
            for c in cuts_by_layer.get(r["layer"], []):
                # Over-full cuts near the spot (red), and the wire's own cuts with room (green); long cuts that
                # only pass through the window are left out (they hide the spot).
                if seg_dist((x, y), c["p"], c["q"]) >= h or math.dist(c["p"], c["q"]) > 2.5 * h:
                    continue
                over = c["slack"] < OVER
                if not over and (math.dist(c["p"], c["q"]) > h or not any(it[0] == r["conn"] for it in c["items"])):
                    continue
                ax.plot([c["p"][0], c["q"][0]], [c["p"][1], c["q"][1]], color="#d62728" if over else "#2ca02c",
                        lw=1.6 if over else 0.6, alpha=0.9 if over else 0.6)
            for s in fin["stretches"]:
                if s["layer"] != r["layer"] or len(s["band"]) < 2:
                    continue
                xs, ys = zip(*s["band"])
                if s["conn"] == r["conn"]:
                    ax.plot(xs, ys, color="#1f77b4", lw=1.4, ls="--")
                else:
                    ax.plot(xs, ys, color="#555", lw=0.4, ls=":")
            for v in fin.get("vias", []):
                if abs(v["at"][0] - x) < h and abs(v["at"][1] - y) < h:
                    ax.add_patch(Circle(tuple(v["at"]), v["d"] / 2, fc="none", ec="#1f77b4" if v["site"] else "#d62728",
                                        lw=1.0, ls="--"))
            ax.add_patch(Circle((x, y), 0.12 * h, fc="none", ec="#ffd400", lw=2.5))
            ax.set_xlim(x - h, x + h)
            ax.set_ylim(y + h, y - h)
            ax.set_aspect("equal")
            cut = r.get("cut")
            sl = f"; worst cut slack {cut['slack']:.3f} mm (cap {cut['cap']:.3f}, need {cut['need']:.3f})" if cut else ""
            ax.set_title(f"{label}: {r['class']}" + (f" / {r['sub']}" if r["sub"] else "")
                         + f"\n{r['name']} {r['from']}->{r['to']} {r['layer']} {r['kind']} ({r['vclass']}){sl}", fontsize=7)
            f = out / f"{label}-{re.sub('[^a-z0-9]+', '-', cls.lower()).strip('-')}-{r['i']}.png"
            fig.savefig(f)
            plt.close(fig)
            files.append(str(f.resolve()))
    return files


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("run", type=Path)
    ap.add_argument("cuts", type=Path, help="the cut_report directory of the run")
    ap.add_argument("--out", type=Path, default=None)
    ap.add_argument("--png", type=int, default=0, help="close-ups per class")
    ap.add_argument("--label", default=None)
    ap.add_argument("--pcb", type=Path, default=None, help="routed board for the close-ups (default: <run>/*-topor/*.kicad_pcb)")
    ap.add_argument("--drc", type=Path, default=None, help="KiCad DRC JSON of the routed board (default: <run>/*--drc-routed.json)")
    a = ap.parse_args(argv)
    rp = find_result(a.run)
    result = json.loads(rp.read_text())
    if "route" not in result:  # a bare RouteResult (examples/topo_route --out)
        result = {"route": result}
    k, c = picked(result)
    cf = a.cuts / f"run{k}" / f"cycle{c}.json"
    rep = json.loads(cf.read_text())
    rows = classify(result, rep)
    drc = None
    dp = a.drc
    if dp is None and a.run.is_dir():
        hits = sorted(glob.glob(str(a.run / "*--drc-routed.json")))
        dp = Path(hits[0]) if hits else None
    if dp is not None and dp.exists():
        drc = join_drc(rows, json.loads(dp.read_text()))
    out = a.out or (a.cuts / "classes")
    out.mkdir(parents=True, exist_ok=True)
    label = a.label or rp.parent.name
    (out / "classes.json").write_text(json.dumps({"result": str(rp), "cut_report": str(cf), "run": k, "cycle": c,
                                                  "drc": drc, "rows": rows}, indent=1))
    md = tables(rows, f"{label}: run {k}, cycle {c} ({cf})", drc)
    files = []
    if a.png:
        pcb = a.pcb
        if pcb is None and a.run.is_dir():
            hits = sorted(glob.glob(str(a.run / "*-topor" / "*.kicad_pcb")))
            pcb = Path(hits[0]) if hits else None
        files = closeup(rows, rep, pcb, out, a.png, label)
        md += "\nClose-ups:\n\n" + "\n".join(f"- {f}" for f in files) + "\n"
    (out / "classes.md").write_text(md)
    print(md)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
